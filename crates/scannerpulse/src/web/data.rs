//! The admin page's Data tab: rename a talkgroup, the talkgroups whose files
//! still carry an old name, and the jobs that correct, merge and purge.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::talkgroups::{self, Renamed};

/// `GET /api/admin/data/uncorrected`: the talkgroups whose files are not yet
/// corrected.
pub async fn uncorrected(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(talkgroups::uncorrected(&state.pool).await?)))
}

#[derive(Debug, Deserialize)]
pub struct NewName {
    pub name: String,
}

/// `POST /api/admin/talkgroups/<id>/rename`.
pub async fn rename(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(body): Json<NewName>,
) -> Result<Json<Value>, ApiError> {
    if let Some(problem) = talkgroups::name_problem(&body.name) {
        return Err(ApiError::BadRequest(problem));
    }
    let mut conn = state.pool.acquire().await?;
    let old: Option<String> = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    match talkgroups::rename(&mut conn, id, &body.name).await? {
        Renamed::Done { files_correct } => {
            admin
                .record(
                    &state,
                    format!(
                        "renamed talkgroup {id} from {:?} to {:?}",
                        old.unwrap_or_default(),
                        body.name
                    ),
                )
                .await;
            Ok(Json(
                json!({"id": id, "name": body.name, "files_correct": files_correct}),
            ))
        }
        Renamed::Same => Ok(Json(
            json!({"id": id, "name": body.name, "unchanged": true}),
        )),
        Renamed::Taken(other, written) => Err(ApiError::Conflict(format!(
            "talkgroup {other} ({written}) has that name already: merge into it instead"
        ))),
        Renamed::NoSuch => Err(ApiError::Missing(format!("there is no talkgroup {id}"))),
    }
}

/// The typed confirmation of a job that moves or deletes files: the
/// talkgroup's name, exactly.
#[derive(Debug, Deserialize)]
pub struct Confirm {
    pub confirm: String,
}

/// `GET /api/admin/talkgroups/<id>/correct`: how many files a correction
/// would change.
pub async fn correct_preview(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(json!(
        crate::data::correct_preview(&mut conn, id).await?
    )))
}

/// `POST /api/admin/talkgroups/<id>/correct`: starts the correction job.
pub async fn correct_start(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(body): Json<Confirm>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (job, count) =
        crate::data::start_correct(&mut conn, id, &body.confirm, &admin.0.login).await?;
    admin
        .record(
            &state,
            format!("started job {job}: correct {count} files of talkgroup {id}"),
        )
        .await;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": job, "count": count})),
    ))
}

#[derive(Debug, Deserialize)]
pub struct MergeTo {
    pub into: i32,
}

/// `GET /api/admin/talkgroups/<id>/merge?into=<id>`: what merging the
/// talkgroup into another would move.
pub async fn merge_preview(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Query(ask): Query<MergeTo>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(json!(
        crate::data::merge_preview(&mut conn, id, ask.into).await?
    )))
}

#[derive(Debug, Deserialize)]
pub struct MergeAsk {
    pub into: i32,
    pub confirm: String,
}

/// `POST /api/admin/talkgroups/<id>/merge`: starts the merge job; the
/// talkgroup that goes is confirmed by its name.
pub async fn merge_start(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(body): Json<MergeAsk>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (job, count) =
        crate::data::start_merge(&mut conn, id, body.into, &body.confirm, &admin.0.login).await?;
    admin
        .record(
            &state,
            format!(
                "started job {job}: merge talkgroup {id} into {} ({count} calls)",
                body.into
            ),
        )
        .await;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": job, "count": count})),
    ))
}

/// `GET /api/admin/talkgroups/<id>/purge`: what purging the talkgroup would
/// delete, and where its files would be kept.
pub async fn purge_preview(
    _: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
) -> Result<Json<Value>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(json!(
        crate::data::purge_preview(&mut conn, id).await?
    )))
}

/// `POST /api/admin/talkgroups/<id>/purge`: starts the purge job.
pub async fn purge_start(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i32>,
    Json(body): Json<Confirm>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (job, count) =
        crate::data::start_purge(&mut conn, id, &body.confirm, &admin.0.login).await?;
    admin
        .record(
            &state,
            format!("started job {job}: purge talkgroup {id} ({count} calls)"),
        )
        .await;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": job, "count": count})),
    ))
}
