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
    let mut conn = state.pool.acquire().await?;
    accounts::add(&mut conn, &body.login, &body.password).await?;
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
    let mut conn = state.pool.acquire().await?;
    accounts::set_password(&mut conn, &login, &body.password).await?;
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
