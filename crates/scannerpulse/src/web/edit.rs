//! `PATCH /api/calls/<id>/transcript`: anyone may correct a transcript. The
//! save keeps the machine's first output beside the person's text.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::MaybeAdmin;
use super::client::Client;
use super::{ApiError, AppState};

/// The longest text a person may save, in characters.
pub const MAX_TEXT: usize = 4000;
/// The saves one address may make per minute on one host.
pub const SAVES_PER_MINUTE: u32 = 30;
const MINUTE: Duration = Duration::from_secs(60);
/// Past this many addresses, those whose minute is over are forgotten.
const ADDRESSES: usize = 10_000;

/// Each address's saves in its current minute, on this host.
pub type Saves = Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>;

/// Counts a save for `address`; false when it is over `limit` this minute.
pub(super) fn allowed(saves: &Saves, address: IpAddr, limit: u32) -> bool {
    let mut saves = saves
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let now = Instant::now();
    if saves.len() >= ADDRESSES {
        saves.retain(|_, (start, _)| now.duration_since(*start) < MINUTE);
    }
    let entry = saves.entry(address).or_insert((now, 0));
    if now.duration_since(entry.0) >= MINUTE {
        *entry = (now, 0);
    }
    entry.1 += 1;
    entry.1 <= limit
}

/// Refuses with 503 while the site is under maintenance; `what` names the
/// paused action.
pub(super) async fn not_paused(state: &AppState, what: &str) -> Result<(), ApiError> {
    let maintenance: Option<Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'maintenance.enabled'")
            .fetch_optional(&state.pool)
            .await?;
    if maintenance.and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err(ApiError::Unavailable(format!(
            "the site is under maintenance: {what} is paused"
        )));
    }
    Ok(())
}

/// Why a text cannot be saved, if it cannot: the frontend renders text, but
/// markup is refused anyway, and so is every control character but a new
/// line.
pub fn refused(text: &str) -> Option<String> {
    if text.chars().count() > MAX_TEXT {
        return Some(format!("a transcript has at most {MAX_TEXT} characters"));
    }
    if text.contains(['<', '>']) {
        return Some("a transcript cannot contain < or >".into());
    }
    if text.chars().any(|c| c.is_control() && c != '\n') {
        return Some("a transcript cannot contain control characters other than a new line".into());
    }
    None
}

#[derive(Debug, Deserialize)]
pub struct Edit {
    pub text: String,
}

pub async fn save(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Client(address): Client,
    MaybeAdmin(admin): MaybeAdmin,
    Json(body): Json<Edit>,
) -> Result<Json<Value>, ApiError> {
    not_paused(&state, "saving").await?;
    if !allowed(&state.saves, address, SAVES_PER_MINUTE) {
        return Err(ApiError::Limit(
            StatusCode::TOO_MANY_REQUESTS,
            format!("at most {SAVES_PER_MINUTE} saves a minute"),
        ));
    }
    if let Some(why) = refused(&body.text) {
        return Err(ApiError::BadRequest(why));
    }
    let started_at: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT started_at FROM calls WHERE id = $1")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let started_at = started_at.ok_or(ApiError::NotFound)?;
    let labeler = admin.as_ref().map(|session| session.login.as_str());
    let mut conn = state.pool.acquire().await?;
    // A save can be a deadlock's victim: ingest creates a month's partitions,
    // and an evaluation draw locks its calls. It is tried once more.
    let mut attempt = 0;
    let saved = loop {
        match crate::transcripts::correct(&mut conn, (id, started_at), &body.text, labeler).await {
            Err(error) if attempt == 0 && crate::listing::is_deadlock(&error) => attempt += 1,
            other => break other?,
        }
    };
    if !saved {
        return Err(ApiError::NotFound);
    }
    Ok(Json(json!({
        "id": id,
        "transcript": body.text.trim(),
        "origin": "human",
    })))
}
