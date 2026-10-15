//! The small public reads: one call, the filter choices, the favorites, the
//! banner's status and the labelers' list.

use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{ApiError, AppState};

/// How long the filter choices are kept before they are read again.
pub const FILTERS_FOR: Duration = Duration::from_secs(300);

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CallDetail {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Option<i32>,
    pub talkgroup_id: Option<i32>,
    pub talkgroup: Option<i64>,
    pub talkgroup_name: Option<String>,
    pub system: Option<String>,
    pub site: Option<String>,
    pub radio_id: Option<i64>,
    pub radio_alias: Option<String>,
    pub file: String,
    pub transcript: Option<String>,
    pub origin: Option<String>,
    /// A person has saved a text for this call.
    pub labeled: bool,
}

pub async fn call(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<CallDetail>, ApiError> {
    let found: Option<CallDetail> = sqlx::query_as(
        "SELECT c.id, c.started_at, c.duration_ms, c.talkgroup_id, t.number AS talkgroup, \
                t.name AS talkgroup_name, s.name AS system, si.name AS site, c.radio_id, \
                c.radio_alias, regexp_replace(c.rel_path, '^.*/', '') AS file, \
                tr.text AS transcript, tr.origin, \
                EXISTS (SELECT 1 FROM transcript_labels l \
                        WHERE l.call_id = c.id AND l.started_at = c.started_at) AS labeled \
         FROM calls c \
         LEFT JOIN talkgroups t ON t.id = c.talkgroup_id \
         LEFT JOIN systems s ON s.id = t.system_id \
         LEFT JOIN sites si ON si.id = c.site_id \
         LEFT JOIN transcripts tr ON tr.call_id = c.id AND tr.started_at = c.started_at \
         WHERE c.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    found.map(Json).ok_or(ApiError::NotFound)
}

/// The talkgroups grouped by system, the systems and the sites. Systems come
/// in the order of the `web.system_order` setting, then by name.
pub async fn filters(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    if let Some((when, value)) = state.filters.lock().await.clone() {
        if when.elapsed() < FILTERS_FOR {
            return Ok(Json(value));
        }
    }
    let value = read_filters(&state).await?;
    *state.filters.lock().await = Some((Instant::now(), value.clone()));
    Ok(Json(value))
}

async fn read_filters(state: &AppState) -> Result<Value, ApiError> {
    let order: Vec<String> =
        sqlx::query_scalar::<_, Value>("SELECT value FROM settings WHERE key = 'web.system_order'")
            .fetch_optional(&state.pool)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
    let mut systems: Vec<(i32, String)> =
        sqlx::query_as("SELECT id, name FROM systems ORDER BY name")
            .fetch_all(&state.pool)
            .await?;
    let rank = |name: &str| order.iter().position(|o| o == name).unwrap_or(order.len());
    systems.sort_by(|a, b| rank(&a.1).cmp(&rank(&b.1)).then(a.1.cmp(&b.1)));
    let talkgroups: Vec<(i32, i32, i64, String)> =
        sqlx::query_as("SELECT id, system_id, number, name FROM talkgroups ORDER BY name, number")
            .fetch_all(&state.pool)
            .await?;
    let sites: Vec<(i32, String)> = sqlx::query_as("SELECT id, name FROM sites ORDER BY name")
        .fetch_all(&state.pool)
        .await?;
    let groups: Vec<Value> = systems
        .iter()
        .map(|(id, name)| {
            json!({
                "system": name,
                "system_id": id,
                "talkgroups": talkgroups
                    .iter()
                    .filter(|t| t.1 == *id)
                    .map(|t| json!({"id": t.0, "number": t.2, "name": t.3}))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({
        "talkgroup_groups": groups,
        "systems": systems.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
        "sites": sites.iter().map(|(id, name)| json!({"id": id, "name": name})).collect::<Vec<_>>(),
    }))
}

/// The named favorite lists, each with its talkgroup ids.
pub async fn favorites(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let rows: Vec<(i64, String, Vec<i32>)> = sqlx::query_as(
        "SELECT f.id, f.name, \
                COALESCE(array_agg(ft.talkgroup_id ORDER BY ft.talkgroup_id) \
                         FILTER (WHERE ft.talkgroup_id IS NOT NULL), '{}') \
         FROM favorites f LEFT JOIN favorite_talkgroups ft ON ft.favorite_id = f.id \
         GROUP BY f.id, f.name ORDER BY f.name",
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!(
        rows.into_iter()
            .map(|(id, name, talkgroups)| json!({"id": id, "name": name, "talkgroups": talkgroups}))
            .collect::<Vec<_>>()
    )))
}

/// What the page's banner shows.
pub async fn status(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let flag = |key: &'static str| {
        let pool = state.pool.clone();
        async move {
            let value: Option<Value> =
                sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
                    .bind(key)
                    .fetch_optional(&pool)
                    .await?;
            Ok::<bool, sqlx::Error>(value.and_then(|v| v.as_bool()).unwrap_or(false))
        }
    };
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM transcription_queue WHERE state = 'queued'")
            .fetch_one(&state.pool)
            .await?;
    Ok(Json(json!({
        "maintenance": flag("maintenance.enabled").await?,
        "transcription": flag("transcription.enabled").await?,
        "queue": queued,
    })))
}

#[derive(Debug, Deserialize)]
pub struct TodoParams {
    pub set: String,
}

/// The labelers' list of a set, when `labeler_list.public` is true.
pub async fn labels_todo(
    State(state): State<AppState>,
    Query(params): Query<TodoParams>,
) -> Result<Json<Value>, ApiError> {
    let improvement = crate::settings::load_improvement(&state.pool)
        .await
        .map_err(|error| match error {
            crate::settings::SettingsError::Database(error) => ApiError::Database(error),
            other => ApiError::BadRequest(other.to_string()),
        })?;
    if !improvement.enabled || !improvement.labeler_list_public {
        return Err(ApiError::Forbidden(
            "the labelers' list is not public".into(),
        ));
    }
    let size = crate::evaluation::set_size(&state.pool, &params.set).await?;
    if size == 0 {
        return Err(ApiError::NotFound);
    }
    let calls = crate::evaluation::to_label(&state.pool, &params.set).await?;
    Ok(Json(json!({
        "set": params.set,
        "size": size,
        "calls": calls.iter().map(|c| json!({
            "id": c.call_id,
            "started_at": c.started_at,
            "system": c.system,
            "talkgroup": c.number,
            "talkgroup_name": c.talkgroup,
            "duration_ms": c.duration_ms,
        })).collect::<Vec<_>>(),
    })))
}
