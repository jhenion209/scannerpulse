//! The admin page's Transcription tab: each talkgroup's switch, the failed
//! calls, and the improvement module's sets and report. The backlog and a
//! talkgroup's past calls are queue jobs (`/api/admin/jobs/queue`), and a
//! candidate's run is a job too (`/api/admin/jobs/asr-run`).

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};

fn record(admin: &Admin, what: String) {
    crate::log::info(format!("admin {}: {what}", admin.0.login));
}

pub async fn talkgroups(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(crate::talkgroups::list(&state.pool).await?)))
}

#[derive(Debug, Deserialize)]
pub struct Switch {
    pub on: bool,
}

/// `PUT /api/admin/talkgroups/<id>/transcribe`: the talkgroup's switch.
pub async fn transcribe(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(body): Json<Switch>,
) -> Result<Json<Value>, ApiError> {
    if !crate::talkgroups::set_transcribe(&state.pool, id, body.on).await? {
        return Err(ApiError::Missing(format!("there is no talkgroup {id}")));
    }
    record(
        &admin,
        format!(
            "turned transcription {} for talkgroup {id}",
            if body.on { "on" } else { "off" }
        ),
    );
    Ok(Json(json!({"id": id, "transcribe": body.on})))
}

/// `POST /api/admin/queue/retry`: every failed call is tried again.
pub async fn retry_failed(
    admin: Admin,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let retried = crate::queue::retry_failed(&state.pool).await?;
    record(
        &admin,
        format!("put {retried} failed calls back in the queue"),
    );
    Ok(Json(json!({"retried": retried})))
}

/// The improvement module's routes answer only while it is on.
async fn improvement_on(state: &AppState) -> Result<(), ApiError> {
    let improvement = crate::settings::load_improvement(&state.pool)
        .await
        .map_err(|error| match error {
            crate::settings::SettingsError::Database(error) => ApiError::Database(error),
            other => ApiError::Internal(other.to_string()),
        })?;
    if improvement.enabled {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "the improvement module is off (improvement_module.enabled)".into(),
        ))
    }
}

/// `GET /api/admin/asr/sets`: the evaluation sets.
pub async fn sets(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    improvement_on(&state).await?;
    Ok(Json(json!(crate::evaluation::sets(&state.pool).await?)))
}

#[derive(Debug, Deserialize)]
pub struct SetAsk {
    pub set: String,
}

fn score(score: &crate::wer::Score) -> Value {
    json!({
        "wer": score.wer(),
        "errors": score.errors,
        "words": score.words,
        "calls": score.calls,
        "silent": score.silent,
        "phantom": score.phantom,
        "summary": score.summary(),
    })
}

/// `GET /api/admin/asr/report?set=NAME`: production's score against the
/// people's texts, and the set's kept runs, as `asr report` gives them.
pub async fn report(
    _: Admin,
    State(state): State<AppState>,
    Query(ask): Query<SetAsk>,
) -> Result<Json<Value>, ApiError> {
    improvement_on(&state).await?;
    let size = crate::evaluation::set_size(&state.pool, &ask.set).await?;
    if size == 0 {
        return Err(ApiError::Missing(format!("there is no set {}", ask.set)));
    }
    let labeled = crate::accuracy::labeled(&state.pool, &ask.set).await?;
    let runs = crate::accuracy::runs(&state.pool, &ask.set).await?;
    Ok(Json(json!({
        "set": ask.set,
        "size": size,
        "labeled": labeled.len(),
        "production": score(&crate::accuracy::production(&labeled)),
        "runs": runs,
    })))
}
