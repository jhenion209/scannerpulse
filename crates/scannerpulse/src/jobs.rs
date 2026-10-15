//! Long jobs: the web service or the command line creates them, and the host
//! that holds the processing lease runs them, one at a time, in steps. A stop
//! takes effect between steps; after a failover the new holder resumes a job
//! from what its last step recorded.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::postgres::PgConnection;

/// What a job does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Queues past calls for transcription: one talkgroup's, or every call
    /// of the talkgroups whose transcription is on (the backlog).
    Queue,
    /// Transcribes a set's labeled calls with a candidate and keeps its score.
    AsrRun,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Queue => "queue",
            Kind::AsrRun => "asr_run",
        }
    }

    pub fn parse(name: &str) -> Option<Kind> {
        match name {
            "queue" => Some(Kind::Queue),
            "asr_run" => Some(Kind::AsrRun),
            _ => None,
        }
    }
}

/// A job as its row holds it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub params: Value,
    pub state: String,
    pub done: i64,
    pub total: Option<i64>,
    pub message: String,
    pub started_by: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    /// The host that stepped it last.
    pub host: Option<String>,
    pub stop_requested: bool,
    /// The job's own state between steps.
    #[serde(skip)]
    pub work: Value,
}

const COLUMNS: &str = "id, kind, params, state, done, total, message, started_by, created_at, \
     started_at, finished_at, updated_at, host, stop_requested, work";

#[derive(Debug)]
pub enum JobError {
    /// The job cannot be created as asked; the text says why.
    Refused(String),
    NoSuch(i64),
    /// The job has ended already.
    Ended(i64),
    Database(sqlx::Error),
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobError::Refused(why) => f.write_str(why),
            JobError::NoSuch(id) => write!(f, "there is no job {id}"),
            JobError::Ended(id) => write!(f, "job {id} has ended already"),
            JobError::Database(error) => write!(f, "the database failed: {error}"),
        }
    }
}

impl std::error::Error for JobError {}

impl From<sqlx::Error> for JobError {
    fn from(error: sqlx::Error) -> JobError {
        JobError::Database(error)
    }
}

/// Adds a job, queued behind any others; returns its id. `params` must
/// already be checked by the kind's own function.
pub async fn create(
    db: impl sqlx::PgExecutor<'_>,
    kind: Kind,
    params: &Value,
    total: Option<i64>,
    started_by: &str,
) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO jobs (kind, params, total, started_by) VALUES ($1, $2, $3, $4) RETURNING id",
    )
    .bind(kind.name())
    .bind(params)
    .bind(total)
    .bind(started_by)
    .fetch_one(db)
    .await
}

/// The newest jobs first.
pub async fn list(db: impl sqlx::PgExecutor<'_>, limit: i64) -> sqlx::Result<Vec<Job>> {
    sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM jobs ORDER BY id DESC LIMIT $1"
    ))
    .bind(limit)
    .fetch_all(db)
    .await
}

pub async fn get(db: impl sqlx::PgExecutor<'_>, id: i64) -> sqlx::Result<Option<Job>> {
    sqlx::query_as(&format!("SELECT {COLUMNS} FROM jobs WHERE id = $1"))
        .bind(id)
        .fetch_optional(db)
        .await
}

/// What asking a job to stop did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// It had not started: it is stopped now.
    Now,
    /// It is running: it stops after its current step.
    AfterStep,
}

/// Asks a job to stop; `by` is who asked.
pub async fn stop(conn: &mut PgConnection, id: i64, by: &str) -> Result<Stopped, JobError> {
    let row: Option<String> = sqlx::query_scalar(
        "UPDATE jobs SET \
           stop_requested = true, \
           state = CASE state WHEN 'queued' THEN 'stopped' ELSE state END, \
           finished_at = CASE state WHEN 'queued' THEN now() ELSE finished_at END, \
           message = CASE state WHEN 'queued' THEN 'stopped by ' || $2 || ' before it started' \
                     ELSE message END \
         WHERE id = $1 AND state IN ('queued', 'running') \
         RETURNING state",
    )
    .bind(id)
    .bind(by)
    .fetch_optional(&mut *conn)
    .await?;
    match row.as_deref() {
        Some("stopped") => Ok(Stopped::Now),
        Some(_) => Ok(Stopped::AfterStep),
        None => match get(&mut *conn, id).await? {
            Some(_) => Err(JobError::Ended(id)),
            None => Err(JobError::NoSuch(id)),
        },
    }
}

/// The calls a queue job queues in one step.
pub const QUEUE_BATCH: i64 = 1000;

/// What a job's step may use.
pub struct Context {
    /// The calls a queue job queues in one step.
    pub batch: i64,
    /// Sends a candidate's requests.
    pub client: crate::asr::Client,
}

impl Context {
    pub fn new(api_key: Option<String>) -> Context {
        Context {
            batch: QUEUE_BATCH,
            client: crate::asr::Client::new(api_key),
        }
    }
}

/// Adds a job that queues `past`'s calls for transcription; returns its id
/// and the number of calls it will queue.
pub async fn start_queue(
    conn: &mut PgConnection,
    past: &crate::queue::Past,
    started_by: &str,
) -> Result<(i64, i64), JobError> {
    if let Some(talkgroup) = past.talkgroup_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM talkgroups WHERE id = $1)")
                .bind(talkgroup)
                .fetch_one(&mut *conn)
                .await?;
        if !exists {
            return Err(JobError::Refused(format!(
                "there is no talkgroup {talkgroup}"
            )));
        }
    }
    let count = crate::queue::count_past(&mut *conn, past).await?;
    let params = serde_json::to_value(past).expect("a Past is plain data");
    let id = create(&mut *conn, Kind::Queue, &params, Some(count), started_by).await?;
    Ok((id, count))
}

/// What one step of a job did.
enum Outcome {
    /// There is more to do: the counts and the state to resume from.
    More {
        done: i64,
        total: Option<i64>,
        work: Value,
        message: String,
    },
    Done(i64, String),
    Failed(String),
    /// A stop was asked for: the job ends where its last step left it.
    Stopped,
}

/// Runs one step of the job at the head of the line. Returns that job's id
/// and its state after the step; `None` when no job waits, or when another
/// host is stepping the head of the line (jobs run one at a time). The step
/// and the record of it are one transaction: a step that fails part way
/// changes nothing, and is tried again.
pub async fn step(
    pool: &sqlx::PgPool,
    host: &str,
    context: &Context,
) -> sqlx::Result<Option<(i64, String)>> {
    use sqlx::Connection;
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    let job: Option<Job> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM jobs \
         WHERE id = (SELECT min(id) FROM jobs WHERE state IN ('queued', 'running')) \
           AND state IN ('queued', 'running') \
         FOR UPDATE SKIP LOCKED"
    ))
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        return Ok(None);
    };
    let outcome = if job.stop_requested {
        Outcome::Stopped
    } else {
        match Kind::parse(&job.kind) {
            Some(Kind::Queue) => queue_step(&mut tx, &job, context.batch).await?,
            Some(Kind::AsrRun) => asr_step(&mut tx, &job, context).await?,
            None => Outcome::Failed(format!("no runner knows jobs of the kind {:?}", job.kind)),
        }
    };
    let (state, done, total, work, message) = match outcome {
        Outcome::Stopped => (
            "stopped",
            job.done,
            job.total,
            job.work.clone(),
            format!("stopped by request after {}", progress(job.done, job.total)),
        ),
        Outcome::Failed(why) => ("failed", job.done, job.total, job.work.clone(), why),
        Outcome::Done(done, message) => (
            "done",
            done,
            job.total.map(|_| done),
            job.work.clone(),
            message,
        ),
        Outcome::More {
            done,
            total,
            work,
            message,
        } => ("running", done, total, work, message),
    };
    sqlx::query(
        "UPDATE jobs SET state = $2, done = $3, total = $4, work = $5, message = $6, host = $7, \
           started_at = COALESCE(started_at, now()), updated_at = now(), \
           finished_at = CASE WHEN $2 = 'running' THEN NULL ELSE now() END \
         WHERE id = $1",
    )
    .bind(job.id)
    .bind(state)
    .bind(done)
    .bind(total)
    .bind(&work)
    .bind(&message)
    .bind(host)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some((job.id, state.to_string())))
}

fn progress(done: i64, total: Option<i64>) -> String {
    match total {
        Some(total) => format!("{done} of {total}"),
        None => done.to_string(),
    }
}

/// Queues the next `batch` calls, newest first, from where the last step
/// stopped (`work.before`).
async fn queue_step(conn: &mut PgConnection, job: &Job, batch: i64) -> sqlx::Result<Outcome> {
    let Ok(past) = serde_json::from_value::<crate::queue::Past>(job.params.clone()) else {
        return Ok(Outcome::Failed(format!(
            "the job's parameters are not a queue's: {}",
            job.params
        )));
    };
    let before = job
        .work
        .get("before")
        .and_then(|v| serde_json::from_value::<DateTime<Utc>>(v.clone()).ok());
    let (n, oldest) = crate::queue::queue_past(&mut *conn, &past, batch, before).await?;
    let done = job.done + i64::try_from(n).unwrap_or(0);
    if n == 0 {
        return Ok(Outcome::Done(done, format!("queued {done} calls")));
    }
    Ok(Outcome::More {
        done,
        total: job.total.map(|total| total.max(done)),
        work: serde_json::json!({ "before": oldest }),
        message: format!("queued {}", progress(done, job.total.map(|t| t.max(done)))),
    })
}

/// A candidate's run: the set, the server and the model, and the request's
/// prompt, language and time limit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrRun {
    pub set: String,
    pub server: String,
    pub model: String,
    pub prompt: Option<String>,
    pub language: Option<String>,
    pub timeout_secs: u64,
}

/// A candidate run's state between steps.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct AsrWork {
    candidate: crate::wer::Score,
    /// Production's first output, on the same calls the candidate answered.
    production: crate::wer::Score,
    skipped: u64,
    elapsed_ms: u64,
    /// Calls in a row without an answer.
    faults: usize,
}

/// Adds a job that runs a candidate over a set's labeled calls, as `asr run`
/// does; returns its id and the number of calls. The language and the time
/// limit are production's.
pub async fn start_asr_run(
    conn: &mut PgConnection,
    set: &str,
    server: &str,
    model: &str,
    prompt: Option<String>,
    started_by: &str,
) -> Result<(i64, i64), JobError> {
    let improvement = crate::settings::load_improvement(&mut *conn)
        .await
        .map_err(|error| JobError::Refused(error.to_string()))?;
    if !improvement.enabled {
        return Err(JobError::Refused(
            "the improvement module is off (improvement_module.enabled)".into(),
        ));
    }
    if !crate::settings::server_url_ok(server) {
        return Err(JobError::Refused(
            "the server must be an http:// or https:// URL without a user name, password, query or fragment; a key goes in SCANNERPULSE_TRANSCRIPTION_API_KEY".into(),
        ));
    }
    if model.trim().is_empty() {
        return Err(JobError::Refused("a model is needed".into()));
    }
    let labeled = crate::accuracy::labeled(&mut *conn, set).await?;
    if labeled.is_empty() {
        return Err(JobError::Refused(format!(
            "no call of the set {set} has a person's text yet"
        )));
    }
    let production = crate::settings::load_transcription(&mut *conn)
        .await
        .map_err(|error| JobError::Refused(error.to_string()))?;
    let run = AsrRun {
        set: set.to_string(),
        server: server.to_string(),
        model: model.trim().to_string(),
        prompt: prompt.filter(|p| !p.trim().is_empty()),
        language: production.as_ref().and_then(|p| p.language.clone()),
        timeout_secs: production
            .as_ref()
            .map_or(120, |p| p.request_timeout.as_secs()),
    };
    let params = serde_json::to_value(&run).expect("an AsrRun is plain data");
    let total = labeled.len() as i64;
    let id = create(&mut *conn, Kind::AsrRun, &params, Some(total), started_by).await?;
    Ok((id, total))
}

/// Transcribes the next labeled call with the candidate; after the last,
/// keeps the run's score.
async fn asr_step(conn: &mut PgConnection, job: &Job, context: &Context) -> sqlx::Result<Outcome> {
    use crate::accuracy::{self, Tried1};
    let Ok(run) = serde_json::from_value::<AsrRun>(job.params.clone()) else {
        return Ok(Outcome::Failed(format!(
            "the job's parameters are not a candidate run's: {}",
            job.params
        )));
    };
    let labeled = accuracy::labeled(&mut *conn, &run.set).await?;
    let mut work: AsrWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let index = usize::try_from(job.done).unwrap_or(usize::MAX);
    let total = labeled.len() as i64;
    let Some(call) = labeled.get(index) else {
        let (Some(wer), calls) = (work.candidate.wer(), work.candidate.calls) else {
            return Ok(Outcome::Failed(
                "no call was scored; nothing was kept".into(),
            ));
        };
        let seconds = work.elapsed_ms as f64 / 1000.0 / calls as f64;
        let id = accuracy::save_run(
            &mut *conn,
            &run.set,
            &accuracy::Run {
                run_at: Utc::now(),
                server: run.server.clone(),
                model: run.model.clone(),
                prompt: run.prompt.clone(),
                calls: calls as i32,
                wer: wer as f32,
                seconds_per_call: seconds as f32,
            },
        )
        .await?;
        return Ok(Outcome::Done(
            job.done,
            format!(
                "kept as run {id}: candidate {}; production on the same calls {}; {seconds:.2} s per call",
                work.candidate.summary(),
                work.production.summary()
            ),
        ));
    };
    let candidate = accuracy::Candidate {
        server: &run.server,
        model: &run.model,
        language: run.language.as_deref(),
        prompt: run.prompt.as_deref(),
        timeout: std::time::Duration::from_secs(run.timeout_secs),
    };
    match accuracy::try_one(&context.client, call, &candidate).await {
        Tried1::Text(text, took) => {
            work.faults = 0;
            work.elapsed_ms += u64::try_from(took.as_millis()).unwrap_or(u64::MAX);
            work.candidate.add(&call.said, &text);
            work.production
                .add(&call.said, call.first_output.as_deref().unwrap_or(""));
        }
        Tried1::NoAnswer(why) => {
            work.faults += 1;
            work.skipped += 1;
            if work.faults >= accuracy::STOP_AFTER_FAULTS {
                return Ok(Outcome::Failed(format!(
                    "{} calls in a row without an answer, the last: {why}; nothing was kept",
                    accuracy::STOP_AFTER_FAULTS
                )));
            }
        }
        Tried1::Unreadable(_) => work.skipped += 1,
    }
    let done = job.done + 1;
    Ok(Outcome::More {
        done,
        total: Some(total),
        work: serde_json::to_value(&work).expect("the work is plain data"),
        message: format!(
            "{} calls, {} skipped",
            progress(done, Some(total)),
            work.skipped
        ),
    })
}
