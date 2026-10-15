//! The admin page's Data tab: rename a talkgroup, the talkgroups whose files
//! still carry an old name, and the jobs that correct, merge and purge.

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::talkgroups::{self, Renamed};

fn record(admin: &Admin, what: String) {
    crate::log::info(format!("admin {}: {what}", admin.0.login));
}

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
            record(
                &admin,
                format!(
                    "renamed talkgroup {id} from {:?} to {:?}",
                    old.unwrap_or_default(),
                    body.name
                ),
            );
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
