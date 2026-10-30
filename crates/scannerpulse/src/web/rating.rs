//! `PUT` and `DELETE /api/calls/<id>/rating`: anyone may rate a call's
//! transcript from 1 to 5 stars, once per browser. The browser is the
//! cookie `sp_rater`; only its SHA-256 is stored.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::client::Client;
use super::edit::{allowed, not_paused};
use super::{ApiError, AppState};

/// The ratings one address may make per minute on one host.
pub const RATINGS_PER_MINUTE: u32 = 60;
/// The cookie that holds the browser's token.
pub const COOKIE: &str = "sp_rater";
/// The token's length in hex characters (16 random bytes).
const TOKEN_CHARS: usize = 32;
const YEAR_SECONDS: u32 = 365 * 86_400;

#[derive(Debug, Deserialize)]
pub struct Stars {
    pub stars: i64,
}

/// The browser's token from the request's cookies, if it holds a well-formed one.
fn token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .find(|token| token.len() == TOKEN_CHARS && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
}

/// The `Set-Cookie` value of a browser's token.
fn cookie(token: &str) -> String {
    format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={YEAR_SECONDS}")
}

/// The guards both routes share: maintenance, the address's limit, and the call.
async fn guarded(
    state: &AppState,
    id: i64,
    address: std::net::IpAddr,
) -> Result<DateTime<Utc>, ApiError> {
    not_paused(state, "rating").await?;
    if !allowed(&state.ratings, address, RATINGS_PER_MINUTE) {
        return Err(ApiError::Limit(
            StatusCode::TOO_MANY_REQUESTS,
            format!("at most {RATINGS_PER_MINUTE} ratings a minute"),
        ));
    }
    let started_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT started_at FROM calls WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    started_at.ok_or(ApiError::NotFound)
}

pub async fn put(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Client(address): Client,
    headers: HeaderMap,
    Json(body): Json<Stars>,
) -> Result<Response, ApiError> {
    if !(1..=5).contains(&body.stars) {
        return Err(ApiError::BadRequest(
            "stars is a whole number from 1 to 5".into(),
        ));
    }
    let started_at = guarded(&state, id, address).await?;
    let token = token(&headers).unwrap_or_else(|| crate::sessions::random_hex(TOKEN_CHARS / 2));
    let mut conn = state.pool.acquire().await?;
    let saved = crate::ratings::set(
        &mut conn,
        (id, started_at),
        &crate::ratings::rater(&token),
        body.stars as i16,
    )
    .await?;
    if !saved {
        return Err(ApiError::NotFound);
    }
    Ok((
        [(header::SET_COOKIE, cookie(&token))],
        Json(json!({"id": id, "stars": body.stars})),
    )
        .into_response())
}

pub async fn clear(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Client(address): Client,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let started_at = guarded(&state, id, address).await?;
    if let Some(token) = token(&headers) {
        let mut conn = state.pool.acquire().await?;
        crate::ratings::clear(&mut conn, (id, started_at), &crate::ratings::rater(&token)).await?;
    }
    Ok(Json(json!({"id": id, "stars": null})))
}
