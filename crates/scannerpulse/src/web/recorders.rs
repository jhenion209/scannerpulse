//! The admin page's Recorders area: the SDRTrunk folders ingest takes calls
//! from, adding one, changing one and previewing one; and what waits for the
//! administrator's decision, unknown system names and talkgroup name
//! proposals.

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::names;
use crate::preview::Preview;
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

/// `GET /api/admin/recorders/<id>/preview`: what filing would do with a
/// sample of the recorder's files. It changes nothing.
pub async fn preview(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i16>,
) -> Result<Json<Preview>, ApiError> {
    let zone = super::audio::time_zone(&state).await?;
    let mut conn = state.pool.acquire().await?;
    crate::preview::preview(&mut conn, id, zone)
        .await
        .map(Json)
        .map_err(refused)
}

/// `GET /api/admin/names`: what waits for a decision, unknown system names
/// and talkgroup name proposals, and how many decisions that is.
pub async fn waiting(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let unknown = names::unknown_systems(&mut conn).await?;
    let proposals = names::proposals(&mut conn).await?;
    Ok(Json(json!({
        "waiting": unknown.len() + proposals.len(),
        "unknown": unknown,
        "proposals": proposals,
    })))
}

#[derive(Debug, Deserialize)]
pub struct Proposed {
    pub talkgroup_id: i32,
    pub name: String,
}

/// `POST /api/admin/names/<approve|dismiss>`: decides a proposal.
pub async fn decide_name(
    admin: Admin,
    State(state): State<AppState>,
    Path(decision): Path<String>,
    Json(body): Json<Proposed>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    match decision.as_str() {
        "approve" => names::approve(&mut conn, body.talkgroup_id, &body.name).await,
        "dismiss" => names::dismiss(&mut conn, body.talkgroup_id, &body.name).await,
        _ => return Err(ApiError::NotFound),
    }
    .map_err(refused)?;
    record(
        &admin,
        format!(
            "{decision} the name {:?} for talkgroup {}",
            body.name, body.talkgroup_id
        ),
    );
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct Unknown {
    pub system: String,
    /// The system it is the same as (map).
    #[serde(default)]
    pub to: String,
    /// The name typed (discard).
    #[serde(default)]
    pub confirm: String,
}

/// `POST /api/admin/staging/<new-system|map|discard>`: decides an unknown
/// system name. Discard answers how many staged files it deleted.
pub async fn decide_system(
    admin: Admin,
    State(state): State<AppState>,
    Path(decision): Path<String>,
    Json(body): Json<Unknown>,
) -> Result<Response, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let system = &body.system;
    let (answer, what) = match decision.as_str() {
        "new-system" => {
            names::new_system(&mut conn, system)
                .await
                .map_err(refused)?;
            (
                StatusCode::NO_CONTENT.into_response(),
                format!("added the system {system:?}"),
            )
        }
        "map" => {
            names::map_system(&mut conn, system, &body.to)
                .await
                .map_err(refused)?;
            (
                StatusCode::NO_CONTENT.into_response(),
                format!("mapped {system:?} to the system {:?}", body.to),
            )
        }
        "discard" => {
            let discarded = names::discard(&mut conn, system, &body.confirm)
                .await
                .map_err(refused)?;
            (
                Json(json!({"discarded": discarded})).into_response(),
                format!("discarded {discarded} staged files of {system:?}"),
            )
        }
        _ => return Err(ApiError::NotFound),
    };
    record(&admin, what);
    Ok(answer)
}
