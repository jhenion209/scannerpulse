//! The admin routes. Each needs a signed-in admin (and, for a change, the
//! session's CSRF token), and each calls the same library function as its
//! command-line twin.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::accounts::{self, AccountError};

impl From<AccountError> for ApiError {
    fn from(error: AccountError) -> ApiError {
        match error {
            AccountError::Database(error) => ApiError::Database(error),
            AccountError::NoSuch(_) => ApiError::Missing(error.to_string()),
            AccountError::Exists(_) | AccountError::LastAdmin => {
                ApiError::Conflict(error.to_string())
            }
            AccountError::BadName(_) | AccountError::BadPassword(_) => {
                ApiError::BadRequest(error.to_string())
            }
        }
    }
}

/// Logs an admin's change with the admin's name.
fn record(admin: &Admin, what: String) {
    crate::log::info(format!("admin {}: {what}", admin.0.login));
}

/// The password's hash, made on the blocking threads.
async fn hashed(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || accounts::new_hash(&password))
        .await
        .map_err(|error| ApiError::Internal(format!("password hash: {error}")))?
        .map_err(ApiError::from)
}

pub async fn accounts_list(
    _: Admin,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(accounts::list(&state.pool).await?)))
}

#[derive(Debug, Deserialize)]
pub struct NewAccount {
    pub login: String,
    pub password: String,
}

pub async fn accounts_add(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<NewAccount>,
) -> Result<StatusCode, ApiError> {
    accounts::check_login(&body.login)?;
    let hash = hashed(body.password).await?;
    let mut conn = state.pool.acquire().await?;
    accounts::add_hashed(&mut conn, &body.login, &hash).await?;
    record(&admin, format!("added the admin {}", body.login));
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
pub struct NewPassword {
    pub password: String,
}

pub async fn accounts_password(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
    Json(body): Json<NewPassword>,
) -> Result<StatusCode, ApiError> {
    let hash = hashed(body.password).await?;
    let mut conn = state.pool.acquire().await?;
    accounts::set_hash(&mut conn, &login, &hash).await?;
    record(&admin, format!("gave {login} a new password"));
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accounts_disable(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    accounts::disable(&mut conn, &login).await?;
    record(&admin, format!("disabled {login}"));
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accounts_enable(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
) -> Result<StatusCode, ApiError> {
    accounts::enable(&state.pool, &login).await?;
    record(&admin, format!("enabled {login}"));
    Ok(StatusCode::NO_CONTENT)
}

impl From<crate::settings::SettingsError> for ApiError {
    fn from(error: crate::settings::SettingsError) -> ApiError {
        use crate::settings::SettingsError;
        match error {
            SettingsError::Database(error) => ApiError::Database(error),
            SettingsError::Unknown(_) => ApiError::Missing(error.to_string()),
            SettingsError::Missing(_) | SettingsError::Invalid { .. } => {
                ApiError::BadRequest(error.to_string())
            }
        }
    }
}

/// `GET /api/admin/status`: the Status tab.
pub async fn status(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(crate::status::read(&state.pool).await?))
}

/// How long a traffic report is kept before it is made again.
pub const TRAFFIC_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// `GET /api/admin/reports/traffic`: the traffic report, kept for ten
/// minutes on each host, since it reads months of calls; a changed time zone
/// makes it again.
pub async fn traffic(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let zone = crate::settings::load(&state.pool).await?.time_zone;
    let mut kept = state.traffic.lock().await;
    if let Some((when, kept_zone, value)) = kept.as_ref() {
        if *kept_zone == zone && when.elapsed() < TRAFFIC_FOR {
            return Ok(Json(value.clone()));
        }
    }
    let mut conn = state.pool.acquire().await?;
    let report = crate::reports::traffic(&mut conn, zone, chrono::Utc::now()).await?;
    let value = json!(report);
    *kept = Some((std::time::Instant::now(), zone, value.clone()));
    Ok(Json(value))
}

/// `POST /api/admin/rescan`: every running ingest lists its sources at once.
pub async fn rescan(admin: Admin, State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    crate::ingest::request_rescan(&state.pool).await?;
    record(&admin, "asked for a rescan of the sources".into());
    Ok(StatusCode::ACCEPTED)
}

/// Every stored setting, with the time it was changed.
pub async fn settings_list(
    _: Admin,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let rows: Vec<(String, Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT key, value, updated_at FROM settings ORDER BY key")
            .fetch_all(&state.pool)
            .await?;
    Ok(Json(json!(
        rows.into_iter()
            .map(|(key, value, updated_at)| json!({"key": key, "value": value, "updated_at": updated_at}))
            .collect::<Vec<_>>()
    )))
}

#[derive(Debug, Deserialize)]
pub struct NewValue {
    pub value: Value,
}

pub async fn settings_set(
    admin: Admin,
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<NewValue>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::settings::change(&mut conn, &key, Some(&body.value)).await?;
    record(&admin, format!("set {key} to {}", body.value));
    Ok(StatusCode::NO_CONTENT)
}

pub async fn settings_unset(
    admin: Admin,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::settings::change(&mut conn, &key, None).await?;
    record(&admin, format!("removed {key}"));
    Ok(StatusCode::NO_CONTENT)
}

impl From<crate::favorites::FavoriteError> for ApiError {
    fn from(error: crate::favorites::FavoriteError) -> ApiError {
        use crate::favorites::FavoriteError;
        match error {
            FavoriteError::Database(error) => ApiError::Database(error),
            FavoriteError::NoSuch(_) => ApiError::Missing(error.to_string()),
            _ => ApiError::BadRequest(error.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Favorite {
    pub talkgroups: Vec<i32>,
}

/// Creates the list, or replaces its talkgroups.
pub async fn favorites_set(
    admin: Admin,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<Favorite>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::favorites::set(&mut conn, &name, &body.talkgroups).await?;
    record(
        &admin,
        format!(
            "set the favorites {name:?} to {} talkgroups",
            body.talkgroups.len()
        ),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn favorites_remove(
    admin: Admin,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    crate::favorites::remove(&state.pool, &name).await?;
    record(&admin, format!("removed the favorites {name:?}"));
    Ok(StatusCode::NO_CONTENT)
}
