//! The admin page's Storage area: the main archive and the locations beside
//! it, adding and removing one, and moving a date range of calls into one.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::jobs::JobError;
use crate::storage::{self, Refused};

/// How long the check of a new location's folder may take: a hung mount must
/// not hold the request.
const CHECK_LIMIT: Duration = Duration::from_secs(5);

/// How long the locations' counts are kept: counting reads every call.
pub const COUNTS_FOR: Duration = Duration::from_secs(600);

/// How long counting, and measuring the counted files, may take.
const COUNT_LIMIT: Duration = Duration::from_secs(60);

/// The locations' counts: location id, calls, the first and the last call,
/// and the space their files use, about.
pub type Counts = Vec<(
    i16,
    i64,
    Option<DateTime<Utc>>,
    Option<DateTime<Utc>>,
    Option<i64>,
)>;

/// The counts kept on this host: when they were made, as an instant and as
/// a time, and the counts.
pub type KeptCounts = std::sync::Arc<tokio::sync::Mutex<Option<(Instant, DateTime<Utc>, Counts)>>>;

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

fn job_refused(error: JobError) -> ApiError {
    match error {
        JobError::Database(error) => ApiError::Database(error),
        other => ApiError::BadRequest(other.to_string()),
    }
}

/// Whether a job that changes where calls are (a move, a talkgroup's purge)
/// ended since `at`: the counts made then are out of date.
async fn moved_since(pool: &sqlx::PgPool, at: DateTime<Utc>) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind IN ('move', 'purge', 'purge_before') AND finished_at > $1)",
    )
    .bind(at)
    .fetch_one(pool)
    .await
}

/// The counts kept, or new ones: one request counts at a time on this host,
/// and the others wait for its answer.
async fn counts(state: &AppState) -> Result<(DateTime<Utc>, Counts), ApiError> {
    let mut kept = state.storage_counts.lock().await;
    if let Some((made, at, counts)) = kept.as_ref()
        && made.elapsed() < COUNTS_FOR
        && !moved_since(&state.pool, *at).await?
    {
        return Ok((*at, counts.clone()));
    }
    let mut tx = state.pool.begin().await?;
    // The database's time, as the jobs' ends are: a job that ends while
    // this count runs has the next request count again.
    let at: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{}ms", COUNT_LIMIT.as_millis()))
        .execute(&mut *tx)
        .await?;
    let counted = storage::counts(&mut *tx).await?;
    let roots: Vec<(i16, String)> = sqlx::query_as("SELECT id, root FROM locations")
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    let measured = tokio::time::timeout(
        COUNT_LIMIT,
        tokio::task::spawn_blocking(move || {
            counted
                .into_iter()
                .map(|c| {
                    let bytes =
                        roots
                            .iter()
                            .find(|(id, _)| *id == c.location_id)
                            .and_then(|(_, root)| {
                                storage::bytes_about(&PathBuf::from(root), &c.sample, c.calls)
                            });
                    (c.location_id, c.calls, c.first_call, c.last_call, bytes)
                })
                .collect::<Counts>()
        }),
    )
    .await
    .map_err(|_| ApiError::Internal("the locations' files did not answer within a minute".into()))?
    .map_err(|error| ApiError::Internal(format!("the count failed: {error}")))?;
    *kept = Some((Instant::now(), at, measured.clone()));
    Ok((at, measured))
}

/// `GET /api/admin/storage`: every location with its calls (counted at most
/// ten minutes ago) and what each host found there, and the hosts whose
/// ingest runs.
pub async fn list(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let (counted_at, counts) = counts(&state).await?;
    let mut conn = state.pool.acquire().await?;
    let locations = storage::list(&mut conn).await?;
    let hosts: Vec<String> = crate::lease::status(&mut *conn)
        .await?
        .heartbeats
        .into_iter()
        .filter(|beat| beat.expires_in > 0.0)
        .map(|beat| beat.host)
        .collect();
    let locations: Vec<Value> = locations
        .into_iter()
        .map(|location| {
            let count = counts.iter().find(|c| c.0 == location.id);
            let mut value = json!(location);
            value["calls"] = json!(count.map_or(0, |c| c.1));
            value["first_call"] = json!(count.and_then(|c| c.2));
            value["last_call"] = json!(count.and_then(|c| c.3));
            value["bytes_about"] = json!(count.and_then(|c| c.4));
            value
        })
        .collect();
    Ok(Json(json!({
        "locations": locations,
        "hosts": hosts,
        "counted_at": counted_at,
    })))
}

#[derive(Debug, Deserialize)]
pub struct NewLocation {
    pub name: String,
    pub folder: String,
}

/// `POST /api/admin/storage`: adds a location once its folder is checked on
/// this machine, as the service account, and marked. The folder is touched
/// only once its name and its place may be added.
pub async fn add(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<NewLocation>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let folder = PathBuf::from(&body.folder);
    let backups = state.machine.backups.clone();
    let mut conn = state.pool.acquire().await?;
    storage::addable(&mut conn, &body.name, &folder, backups.as_deref())
        .await
        .map_err(refused)?;
    let (checking, name) = (folder.clone(), body.name.clone());
    let checked = tokio::time::timeout(
        CHECK_LIMIT,
        tokio::task::spawn_blocking(move || storage::check_here(&checking, &name)),
    )
    .await
    .map_err(|_| {
        ApiError::BadRequest(format!(
            "{} did not answer within {} s: is it mounted?",
            folder.display(),
            CHECK_LIMIT.as_secs()
        ))
    })?
    .map_err(|error| ApiError::Internal(format!("the folder check failed: {error}")))?;
    let id = storage::add(&mut conn, &body.name, &folder, backups.as_deref(), |_| {
        checked
    })
    .await
    .map_err(refused)?;
    *state.storage_counts.lock().await = None;
    record(
        &admin,
        format!("added the location {} ({})", body.name, folder.display()),
    );
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

/// `DELETE /api/admin/storage/<id>`: removes a location that holds no call.
pub async fn remove(
    admin: Admin,
    State(state): State<AppState>,
    Path(id): Path<i16>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let name = storage::remove(&mut conn, id).await.map_err(refused)?;
    *state.storage_counts.lock().await = None;
    record(&admin, format!("removed the location {name}"));
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct MoveAsk {
    pub to: String,
    pub from: NaiveDate,
    pub until: NaiveDate,
}

/// `GET /api/admin/storage/move?to=&from=&until=`: what moving the calls of
/// those days into the location would do. It changes nothing.
pub async fn move_preview(
    _: Admin,
    State(state): State<AppState>,
    Query(ask): Query<MoveAsk>,
) -> Result<Json<crate::moves::MovePreview>, ApiError> {
    let zone = super::audio::time_zone(&state).await?;
    let mut conn = state.pool.acquire().await?;
    crate::moves::preview(&mut conn, &ask.to, ask.from, ask.until, zone)
        .await
        .map(Json)
        .map_err(job_refused)
}

#[derive(Debug, Deserialize)]
pub struct MoveStart {
    pub to: String,
    pub from: NaiveDate,
    pub until: NaiveDate,
    pub confirm: String,
}

/// `POST /api/admin/storage/move`: starts the move, with `move` typed.
pub async fn move_start(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<MoveStart>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let zone = super::audio::time_zone(&state).await?;
    let mut conn = state.pool.acquire().await?;
    let (job, preview) = crate::moves::start(
        &mut conn,
        &body.to,
        body.from,
        body.until,
        zone,
        &body.confirm,
        &admin.0.login,
    )
    .await
    .map_err(job_refused)?;
    record(
        &admin,
        format!(
            "started job {job}: move the {} calls of {} to {} into {}",
            preview.calls, body.from, body.until, body.to
        ),
    );
    Ok((
        StatusCode::CREATED,
        Json(json!({"job": job, "calls": preview.calls})),
    ))
}
