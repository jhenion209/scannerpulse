//! Scores against the text people saved for a set's calls: production's first
//! output, which needs no server, and a candidate model or prompt, which
//! transcribes the set again without saving any transcript.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;

use crate::asr::{AsrError, Client, Request};
use crate::wer::Score;

/// A labeled call of a set: the newest text a person saved for it, and the
/// machine's first output, which the label keeps.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Labeled {
    pub call_id: i64,
    pub started_at: DateTime<Utc>,
    pub said: String,
    pub first_output: Option<String>,
    pub root: String,
    pub rel_path: String,
}

impl Labeled {
    pub fn path(&self) -> PathBuf {
        PathBuf::from(&self.root).join(&self.rel_path)
    }
}

/// The set's labeled calls, oldest first. A later save of the same call wins.
pub async fn labeled(db: impl PgExecutor<'_>, set: &str) -> sqlx::Result<Vec<Labeled>> {
    sqlx::query_as(
        "SELECT * FROM ( \
           SELECT DISTINCT ON (l.call_id, l.started_at) l.call_id, l.started_at, \
                  l.text AS said, l.machine_text AS first_output, loc.root, c.rel_path \
           FROM eval_calls e \
           JOIN transcript_labels l ON l.call_id = e.call_id AND l.started_at = e.started_at \
           JOIN calls c ON c.id = e.call_id AND c.started_at = e.started_at \
           JOIN locations loc ON loc.id = c.location_id \
           WHERE e.set_name = $1 \
           ORDER BY l.call_id, l.started_at, l.id DESC) newest \
         ORDER BY started_at, call_id",
    )
    .bind(set)
    .fetch_all(db)
    .await
}

/// Production's score: each call's first machine output against its label.
pub fn production(labeled: &[Labeled]) -> Score {
    let mut score = Score::default();
    for call in labeled {
        score.add(&call.said, call.first_output.as_deref().unwrap_or(""));
    }
    score
}

/// One kept run of a candidate over a set.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Run {
    pub run_at: DateTime<Utc>,
    pub server: String,
    pub model: String,
    pub prompt: Option<String>,
    pub calls: i32,
    pub wer: f32,
    pub seconds_per_call: f32,
}

/// The set's runs, oldest first.
pub async fn runs(db: impl PgExecutor<'_>, set: &str) -> sqlx::Result<Vec<Run>> {
    sqlx::query_as(
        "SELECT run_at, server, model, prompt, calls, wer, seconds_per_call \
         FROM asr_runs WHERE set_name = $1 ORDER BY run_at, id",
    )
    .bind(set)
    .fetch_all(db)
    .await
}

/// Keeps a run; returns its id.
pub async fn save_run(db: impl PgExecutor<'_>, set: &str, run: &Run) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO asr_runs (run_at, set_name, server, model, prompt, calls, wer, \
                               seconds_per_call) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(run.run_at)
    .bind(set)
    .bind(&run.server)
    .bind(&run.model)
    .bind(&run.prompt)
    .bind(run.calls)
    .bind(run.wer)
    .bind(run.seconds_per_call)
    .fetch_one(db)
    .await
}

/// The candidate: a server, a model, and what else the request carries.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub server: &'a str,
    pub model: &'a str,
    pub language: Option<&'a str>,
    pub prompt: Option<&'a str>,
    pub timeout: Duration,
}

/// After this many calls in a row that the server did not answer, whether it
/// failed or refused them, the run stops: the server is down or the candidate
/// (its model, say) is wrong for every call. An answer starts the count over;
/// an unreadable file neither adds to it nor starts it over.
pub const STOP_AFTER_FAULTS: usize = 3;

/// What a candidate's run found.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tried {
    pub candidate: Score,
    /// Production's score on the same calls: the calls the candidate answered.
    pub production: Score,
    /// The calls that gave no answer, and why.
    pub skipped: Vec<(i64, String)>,
    /// The time the answered calls took.
    pub elapsed: Duration,
    /// Why the run stopped before the end.
    pub stopped: Option<String>,
}

impl Tried {
    pub fn seconds_per_call(&self) -> Option<f64> {
        (self.candidate.calls > 0).then(|| self.elapsed.as_secs_f64() / self.candidate.calls as f64)
    }
}

/// Transcribes each labeled call with the candidate, one call at a time, and
/// scores the answers. Nothing is saved. `each` hears the number of calls done.
pub async fn try_candidate(
    client: &Client,
    labeled: &[Labeled],
    candidate: &Candidate<'_>,
    mut each: impl FnMut(usize),
) -> Tried {
    let mut tried = Tried::default();
    let mut faults = 0;
    let request = Request {
        url: candidate.server,
        model: candidate.model,
        language: candidate.language,
        prompt: candidate.prompt,
        timeout: candidate.timeout,
    };
    for (done, call) in labeled.iter().enumerate() {
        let path = call.path();
        let started = Instant::now();
        let answer = match crate::transcriber::read_audio(&path).await {
            Err(why) => Err(AsrError::Audio(why)),
            Ok(audio) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("call.mp3")
                    .to_string();
                client.transcribe(&request, &name, audio).await
            }
        };
        match answer {
            Ok(text) => {
                faults = 0;
                tried.elapsed += started.elapsed();
                tried.candidate.add(&call.said, &text);
                tried
                    .production
                    .add(&call.said, call.first_output.as_deref().unwrap_or(""));
            }
            Err(AsrError::Server(why) | AsrError::Call(why)) => {
                faults += 1;
                tried.skipped.push((call.call_id, why.clone()));
                if faults >= STOP_AFTER_FAULTS {
                    tried.stopped = Some(format!(
                        "{STOP_AFTER_FAULTS} calls in a row without an answer, the last: {why}"
                    ));
                    break;
                }
            }
            Err(AsrError::Audio(why)) => {
                tried.skipped.push((call.call_id, why));
            }
        }
        each(done + 1);
    }
    tried
}
