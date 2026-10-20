//! The admin page's Recorders area: the SDRTrunk folders ingest takes calls
//! from, adding one, and changing one.

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::recorders::{self, Change, Refused};

/// How long the check of a new recorder's folder may take: a hung mount must
/// not hold the request.
const CHECK_LIMIT: Duration = Duration::from_secs(5);

fn record(admin: &Admin, what: String) {
    crate::log::info(format!("admin {}: {what}", admin.0.login));
}

fn refused(error: Refused) -> ApiError {
    match error {
        Refused::Refused(why) => ApiError::BadRequest(why),
        Refused::NoSuch(why) => ApiError::Missing(why),
        Refused::Database(error) => ApiError::Database(error),
    }
}

/// `GET /api/admin/recorders`: every recorder with its counts of the day and
/// what each host found in its folder, and the hosts whose ingest runs.
pub async fn list(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let zone = super::audio::time_zone(&state).await?;
    let mut conn = state.pool.acquire().await?;
    let recorders = recorders::list(&mut conn, recorders::start_of_day(Utc::now(), zone)).await?;
    let hosts: Vec<String> = crate::lease::status(&mut *conn)
        .await?
        .heartbeats
        .into_iter()
        .filter(|beat| beat.expires_in > 0.0)
        .map(|beat| beat.host)
        .collect();
    Ok(Json(json!({"recorders": recorders, "hosts": hosts})))
}

#[derive(Debug, Deserialize)]
pub struct NewRecorder {
    pub name: String,
    pub folder: String,
    /// Taken in when asked, rather than watched.
    #[serde(default)]
    pub when_asked: bool,
}

/// `POST /api/admin/recorders`: adds a recorder once its folder is checked on
/// this machine, as the service account.
pub async fn add(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<NewRecorder>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let folder = PathBuf::from(&body.folder);
    let checking = folder.clone();
    let checked = tokio::time::timeout(
        CHECK_LIMIT,
        tokio::task::spawn_blocking(move || recorders::check_here(&checking)),
    )
    .await
    .map_err(|_| {
        ApiError::BadRequest(format!(
            "{} did not answer within {} s: is its share mounted?",
            folder.display(),
            CHECK_LIMIT.as_secs()
        ))
    })?
    .map_err(|error| ApiError::Internal(format!("the folder check failed: {error}")))?;
    let mut conn = state.pool.acquire().await?;
    let id = recorders::add(&mut conn, &body.name, &folder, body.when_asked, |_| checked)
        .await
        .map_err(refused)?;
    record(
        &admin,
        format!(
            "added the recorder {} ({}, {})",
            body.name,
            folder.display(),
            if body.when_asked {
                "taken in when asked"
            } else {
                "watched"
            }
        ),
    );
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

/// `POST /api/admin/recorders/<id>/<change>`: watch, ask, pause, bring-in or
/// remove.
pub async fn change(
    admin: Admin,
    State(state): State<AppState>,
    Path((id, change)): Path<(i16, String)>,
) -> Result<StatusCode, ApiError> {
    let change: Change = change.parse().map_err(|_| ApiError::NotFound)?;
    let mut conn = state.pool.acquire().await?;
    recorders::change(&mut conn, id, change)
        .await
        .map_err(refused)?;
    record(&admin, format!("recorder {id}: {change:?}"));
    Ok(StatusCode::NO_CONTENT)
}
