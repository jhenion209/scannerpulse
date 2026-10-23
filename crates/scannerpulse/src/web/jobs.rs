//! The admin page's jobs: list them, follow one, stop one, and start the
//! jobs of this sub-project. The host that holds the processing lease runs
//! them; this service only creates them.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::jobs::{self, JobError, Stopped};

/// The jobs the list shows.
pub const LISTED: i64 = 50;

impl From<JobError> for ApiError {
    fn from(error: JobError) -> ApiError {
        match error {
            JobError::Database(error) => ApiError::Database(error),
            JobError::NoSuch(_) => ApiError::Missing(error.to_string()),
            JobError::Ended(_) | JobError::Busy(_) => ApiError::Conflict(error.to_string()),
            JobError::Refused(_) => ApiError::BadRequest(error.to_string()),
        }
    }
}

pub async fn list(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(jobs::list(&state.pool, LISTED).await?)))
}

pub async fn one(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let job = jobs::get(&state.pool, id)
        .await?
        .ok_or(JobError::NoSuch(id))?;
    Ok(Json(json!(job)))
}

pub async fn stop(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    // Whoever started a job may always stop it.
    let started_by: Option<String> =
        sqlx::query_scalar("SELECT started_by FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    if started_by.as_deref() != Some(admin.0.login.as_str()) {
        admin.require(crate::roles::Function::StopJobs)?;
    }
    let stopped = jobs::stop(&mut conn, id, &admin.0.login).await?;
    admin
        .record(&state, format!("asked job {id} to stop"))
        .await;
    Ok(Json(json!({
        "id": id,
        "stopped": match stopped {
            Stopped::Now => "now",
            Stopped::AfterStep => "after its current step",
        },
    })))
}

/// Which calls a queue job takes: a talkgroup (none: the backlog of every
/// talkgroup whose transcription is on), and local days.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct QueueAsk {
    pub talkgroup_id: Option<i32>,
    pub from: Option<NaiveDate>,
    pub to: Option<NaiveDate>,
}

/// `GET /api/admin/jobs/queue`: how many calls a queue job would take.
pub async fn queue_preview(
    _: Admin,
    State(state): State<AppState>,
    Query(ask): Query<QueueAsk>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let past = jobs::queue_plan(&mut conn, ask.talkgroup_id, ask.from, ask.to).await?;
    let count = crate::queue::count_past(&mut *conn, &past).await?;
    Ok(Json(json!({"count": count, "min_ms": past.min_ms})))
}

/// `POST /api/admin/jobs/queue`: starts the queue job.
pub async fn queue_start(
    admin: Admin,
    State(state): State<AppState>,
    Json(ask): Json<QueueAsk>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut conn = state.pool.acquire().await?;
    let past = jobs::queue_plan(&mut conn, ask.talkgroup_id, ask.from, ask.to).await?;
    let (id, count) = jobs::start_queue(&mut conn, &past, &admin.0.login).await?;
    admin
        .record(&state, format!("started job {id}: queue {count} calls"))
        .await;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "count": count}))))
}

#[derive(Debug, Deserialize)]
pub struct AsrAsk {
    pub set: String,
    pub server: String,
    pub model: String,
    #[serde(default)]
    pub prompt: Option<String>,
}

/// `POST /api/admin/jobs/asr-run`: starts a candidate's run.
pub async fn asr_start(
    admin: Admin,
    State(state): State<AppState>,
    Json(ask): Json<AsrAsk>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (id, count) = jobs::start_asr_run(
        &mut conn,
        &ask.set,
        &ask.server,
        &ask.model,
        ask.prompt,
        &admin.0.login,
    )
    .await?;
    admin
        .record(
            &state,
            format!(
                "started job {id}: the set {} with {} at {}",
                ask.set, ask.model, ask.server
            ),
        )
        .await;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "count": count}))))
}
