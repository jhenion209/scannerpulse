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
