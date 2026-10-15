//! Errors as the API answers them: a status and a JSON body
//! `{"error": ..., "detail": ...}`, never a stack trace or a query.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    /// The request asks for something that cannot be done (400).
    BadRequest(String),
    NotFound,
    /// Too many at once, or too large (429 or 413 come from the caller).
    Limit(StatusCode, String),
    /// The database failed or is gone (503).
    Database(sqlx::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> ApiError {
        ApiError::Database(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error, detail) = match self {
            ApiError::BadRequest(detail) => (StatusCode::BAD_REQUEST, "bad request", detail),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found", String::new()),
            ApiError::Limit(status, detail) => (status, "limit", detail),
            ApiError::Database(error) => {
                if is_timeout(&error) {
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "too slow",
                        "the query took too long; narrow the search".to_string(),
                    )
                } else {
                    crate::log::error(format!("database: {error}"));
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "database",
                        "the database is not available".to_string(),
                    )
                }
            }
        };
        (status, Json(json!({ "error": error, "detail": detail }))).into_response()
    }
}

/// A statement cancelled by its time limit (57014).
fn is_timeout(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| code == "57014")
}
