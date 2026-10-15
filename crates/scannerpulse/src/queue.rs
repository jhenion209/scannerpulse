//! The transcription queue: the calls that wait for a transcript, newest
//! first, and the calls whose transcription failed for good. Every write of
//! the transcriber is made in a transaction that holds the processing lease.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::PgExecutor;
use sqlx::postgres::{PgConnection, PgPool};

use crate::servers::ServerState;
use crate::store::{self, RecordError};

/// When a call is tried again after its first, second, third and fourth
/// failure. The fifth failure is for good.
pub const RETRY_AFTER: [Duration; 4] = [
    Duration::from_secs(60),
    Duration::from_secs(600),
    Duration::from_secs(3600),
    Duration::from_secs(6 * 3600),
];

/// The wait after `failures` failures; `None` when the call has failed for
/// good.
pub fn retry_after(failures: i16) -> Option<Duration> {
    usize::try_from(failures)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|at| RETRY_AFTER.get(at).copied())
}

/// A queued call that is due, and where its file is.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Queued {
    pub call_id: i64,
    pub started_at: DateTime<Utc>,
    /// Failures counted so far.
    pub attempts: i16,
    pub root: String,
    pub rel_path: String,
}

impl Queued {
    pub fn path(&self) -> PathBuf {
        Path::new(&self.root).join(&self.rel_path)
    }
}

/// Up to `limit` queued calls whose next try is due, newest first, leaving out
/// the calls in `skip`.
pub async fn due(db: impl PgExecutor<'_>, limit: i64, skip: &[i64]) -> sqlx::Result<Vec<Queued>> {
    sqlx::query_as(
        "SELECT q.call_id, q.started_at, q.attempts, l.root, c.rel_path \
         FROM transcription_queue q \
         JOIN calls c ON c.id = q.call_id AND c.started_at = q.started_at \
         JOIN locations l ON l.id = c.location_id \
         WHERE q.state = 'queued' AND q.next_try_at <= now() AND q.call_id <> ALL($2) \
         ORDER BY q.started_at DESC, q.call_id DESC LIMIT $1",
    )
    .bind(limit)
    .bind(skip)
    .fetch_all(db)
    .await
}

/// What a transcript's write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// The call has the machine's text.
    Text,
    /// The machine heard no speech.
    NoSpeech,
    /// A person's text was there already and stays.
    KeptHuman,
}

/// Writes the machine's `text` for `call` and takes the call out of the
/// queue, in one fenced transaction. A transcript that a person wrote is
/// never replaced. Announces the transcript on `scannerpulse_transcripts`.
pub async fn write(
    conn: &mut PgConnection,
    holder: &str,
    call: (i64, DateTime<Utc>),
    text: &str,
    model: &str,
) -> Result<Written, RecordError> {
    let mut tx = store::fenced(conn, holder).await?;
    // A person's save takes this lock first, so the two are taken in turn.
    sqlx::query("SELECT 1 FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE")
        .bind(call.0)
        .bind(call.1)
        .execute(&mut *tx)
        .await?;
    let origin: Option<String> = sqlx::query_scalar(
        "SELECT origin FROM transcripts WHERE call_id = $1 AND started_at = $2 FOR UPDATE",
    )
    .bind(call.0)
    .bind(call.1)
    .fetch_optional(&mut *tx)
    .await?;
    let text = text.trim();
    let written = if origin.as_deref() == Some("human") {
        Written::KeptHuman
    } else if text.is_empty() {
        sqlx::query(
            "DELETE FROM transcripts WHERE call_id = $1 AND started_at = $2 AND origin = 'machine'",
        )
        .bind(call.0)
        .bind(call.1)
        .execute(&mut *tx)
        .await?;
        Written::NoSpeech
    } else {
        let done = sqlx::query(
            "INSERT INTO transcripts (call_id, started_at, text, origin, model, updated_at) \
             VALUES ($1, $2, $3, 'machine', $4, now()) \
             ON CONFLICT (call_id, started_at) DO UPDATE \
             SET text = EXCLUDED.text, model = EXCLUDED.model, updated_at = now() \
             WHERE transcripts.origin = 'machine'",
        )
        .bind(call.0)
        .bind(call.1)
        .bind(text)
        .bind(model)
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            Written::KeptHuman
        } else {
            Written::Text
        }
    };
    if written != Written::KeptHuman {
        let state = if written == Written::Text {
            "text"
        } else {
            "no_speech"
        };
        sqlx::query("UPDATE calls SET transcript_state = $3 WHERE id = $1 AND started_at = $2")
            .bind(call.0)
            .bind(call.1)
            .bind(state)
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT pg_notify('scannerpulse_transcripts', $1)")
            .bind(call.0.to_string())
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("DELETE FROM transcription_queue WHERE call_id = $1 AND started_at = $2")
        .bind(call.0)
        .bind(call.1)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(written)
}

/// Counts a failure against a queued call, in one fenced transaction: it is
/// tried again after `retry_after`, or marked failed with `error`. True when
/// it failed for good; `None` when no row changed, because a correction took
/// the call out of the queue.
pub async fn count_failure(
    conn: &mut PgConnection,
    holder: &str,
    queued: &Queued,
    error: &str,
) -> Result<Option<bool>, RecordError> {
    let failures = queued.attempts.saturating_add(1);
    let wait = retry_after(failures);
    let mut tx = store::fenced(conn, holder).await?;
    let done = sqlx::query(
        "UPDATE transcription_queue SET attempts = $3, last_error = $4, \
             next_try_at = CASE WHEN $5::float8 IS NULL THEN next_try_at \
                                ELSE now() + make_interval(secs => $5::float8) END, \
             state = CASE WHEN $5::float8 IS NULL THEN 'failed' ELSE 'queued' END \
         WHERE call_id = $1 AND started_at = $2 AND state = 'queued'",
    )
    .bind(queued.call_id)
    .bind(queued.started_at)
    .bind(failures)
    .bind(error)
    .bind(wait.map(|w| w.as_secs_f64()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((done.rows_affected() > 0).then_some(wait.is_none()))
}

/// Moves a queued call's next try along `RETRY_AFTER` after the servers
/// failed on it `faults` times (2 or more), in one fenced transaction. Its
/// attempts and state stay, so a server fault never fails a call.
pub async fn defer(
    conn: &mut PgConnection,
    holder: &str,
    queued: &Queued,
    faults: u16,
) -> Result<(), RecordError> {
    let at = usize::from(faults.saturating_sub(2)).min(RETRY_AFTER.len() - 1);
    let mut tx = store::fenced(conn, holder).await?;
    sqlx::query(
        "UPDATE transcription_queue SET next_try_at = now() + make_interval(secs => $3) \
         WHERE call_id = $1 AND started_at = $2 AND state = 'queued'",
    )
    .bind(queued.call_id)
    .bind(queued.started_at)
    .bind(RETRY_AFTER[at].as_secs_f64())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// The queue in numbers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
pub struct Counts {
    pub queued: i64,
    /// Queued calls whose next try is due.
    pub due: i64,
    pub failed: i64,
    /// Calls queued in the last minute.
    pub last_minute: i64,
}

pub async fn counts(db: impl PgExecutor<'_>) -> sqlx::Result<Counts> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'queued') AS queued, \
                count(*) FILTER (WHERE state = 'queued' AND next_try_at <= now()) AS due, \
                count(*) FILTER (WHERE state = 'failed') AS failed, \
                count(*) FILTER (WHERE queued_at > now() - interval '1 minute') AS last_minute \
         FROM transcription_queue",
    )
    .fetch_one(db)
    .await
}

/// A call whose transcription failed for good.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow, serde::Serialize)]
pub struct Failed {
    pub call_id: i64,
    pub started_at: DateTime<Utc>,
    pub attempts: i16,
    pub last_error: Option<String>,
}

/// The calls that failed for good, newest first, `limit` at most.
pub async fn failed(db: impl PgExecutor<'_>, limit: i64) -> sqlx::Result<Vec<Failed>> {
    sqlx::query_as(
        "SELECT call_id, started_at, attempts, last_error FROM transcription_queue \
         WHERE state = 'failed' ORDER BY started_at DESC, call_id DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(db)
    .await
}

/// Puts a failed call back in the queue with no failures counted; false when
/// the call had not failed.
pub async fn retry(db: impl PgExecutor<'_>, call: (i64, DateTime<Utc>)) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "UPDATE transcription_queue \
         SET state = 'queued', attempts = 0, last_error = NULL, next_try_at = now() \
         WHERE call_id = $1 AND started_at = $2 AND state = 'failed'",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(db)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// What `count_past` and `queue_past` take: the past calls that were never
/// tried, at least `min_ms` long, of one talkgroup, or with no talkgroup of
/// every talkgroup whose transcription is on (the backlog); from `from`, and
/// until before `until`, when they are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Past {
    pub talkgroup_id: Option<i32>,
    pub from: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub min_ms: i32,
}

const PAST: &str = "FROM calls c \
     WHERE (c.talkgroup_id = $1 \
            OR ($1::int IS NULL AND c.talkgroup_id IN (SELECT id FROM talkgroups WHERE transcribe))) \
       AND ($2::timestamptz IS NULL OR c.started_at >= $2) \
       AND ($3::timestamptz IS NULL OR c.started_at < $3) \
       AND c.transcript_state = 'untried' AND c.duration_ms >= $4 \
       AND NOT EXISTS (SELECT 1 FROM transcription_queue q \
                       WHERE q.call_id = c.id AND q.started_at = c.started_at)";

/// How many of those calls are not queued yet.
pub async fn count_past(db: impl PgExecutor<'_>, past: &Past) -> sqlx::Result<i64> {
    sqlx::query_scalar(&format!("SELECT count(*) {PAST}"))
        .bind(past.talkgroup_id)
        .bind(past.from)
        .bind(past.until)
        .bind(past.min_ms)
        .fetch_one(db)
        .await
}

/// Queues up to `batch` of those calls, newest first, from `before` back
/// (`None`: from the newest); returns how many, and the oldest start it
/// queued, which is the next call's `before`. Called again until it returns 0.
pub async fn queue_past(
    db: impl PgExecutor<'_>,
    past: &Past,
    batch: i64,
    before: Option<DateTime<Utc>>,
) -> sqlx::Result<(u64, Option<DateTime<Utc>>)> {
    // `<=`, not `<`: calls that share the cursor's instant and missed the last
    // batch are still taken; the ones it queued are left out by NOT EXISTS.
    let (n, oldest): (i64, Option<DateTime<Utc>>) = sqlx::query_as(&format!(
        "WITH queued AS ( \
           INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
           SELECT c.id, c.started_at, now(), now(), 'queued' {PAST} \
             AND ($6::timestamptz IS NULL OR c.started_at <= $6) \
           ORDER BY c.started_at DESC LIMIT $5 ON CONFLICT DO NOTHING \
           RETURNING started_at) \
         SELECT count(*), min(started_at) FROM queued"
    ))
    .bind(past.talkgroup_id)
    .bind(past.from)
    .bind(past.until)
    .bind(past.min_ms)
    .bind(batch)
    .bind(before)
    .fetch_one(db)
    .await?;
    Ok((u64::try_from(n).unwrap_or(0), oldest))
}

/// Keeps the servers' state for the admin page, and forgets servers that are
/// no longer listed.
pub async fn save_servers(db: &PgPool, states: &[ServerState]) -> sqlx::Result<()> {
    let urls: Vec<&str> = states.iter().map(|s| s.url.as_str()).collect();
    let cooling: Vec<Option<f64>> = states
        .iter()
        .map(|s| s.cooling_for.map(|d| d.as_secs_f64()))
        .collect();
    let rounds: Vec<i32> = states
        .iter()
        .map(|s| i32::try_from(s.failed_rounds).unwrap_or(i32::MAX))
        .collect();
    let errors: Vec<Option<&str>> = states.iter().map(|s| s.last_error.as_deref()).collect();
    let mut tx = db.begin().await?;
    sqlx::query(
        "INSERT INTO transcription_servers (url, cooling_until, failed_rounds, last_error, updated_at) \
         SELECT u.url, now() + make_interval(secs => u.secs), u.rounds, u.error, now() \
         FROM unnest($1::text[], $2::float8[], $3::int[], $4::text[]) AS u (url, secs, rounds, error) \
         ON CONFLICT (url) DO UPDATE SET cooling_until = EXCLUDED.cooling_until, \
             failed_rounds = EXCLUDED.failed_rounds, last_error = EXCLUDED.last_error, \
             updated_at = EXCLUDED.updated_at",
    )
    .bind(&urls)
    .bind(&cooling)
    .bind(&rounds)
    .bind(&errors)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM transcription_servers WHERE url <> ALL($1)")
        .bind(&urls)
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

/// A server as last saved.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct StoredServer {
    pub url: String,
    pub cooling_until: Option<DateTime<Utc>>,
    pub failed_rounds: i32,
    pub last_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

/// The servers as last saved, by URL.
pub async fn servers(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<StoredServer>> {
    sqlx::query_as(
        "SELECT url, cooling_until, failed_rounds, last_error, updated_at \
         FROM transcription_servers ORDER BY url",
    )
    .fetch_all(db)
    .await
}
