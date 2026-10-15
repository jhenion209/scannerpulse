//! Active Viewers: every open page is a row of `viewers`, written by the
//! host that holds its live socket, so the admin page sees both hosts'
//! pages. A live snapshot only: a page's row goes when the page closes.
//! These writes are best effort and never hold up the live feed.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use axum::http::{HeaderMap, header};

use super::auth::{self, Admin};
use super::{ApiError, AppState, client};

/// How often a host marks its pages as still open.
pub const REFRESH: Duration = Duration::from_secs(30);
/// A row not marked for this long belongs to a host that stopped.
pub const STALE_SECONDS: i32 = 90;
/// The most of a browser's description that is kept.
const USER_AGENT: usize = 512;

/// What a page's row holds, once its first write has found it out.
#[derive(Debug, Clone)]
pub struct Page {
    pub address: String,
    pub user_agent: String,
    pub admin: Option<String>,
    pub connected_at: DateTime<Utc>,
}

/// The pages this host holds open, by id: `None` until the page's first
/// write has found out its address and admin.
pub type Open = Arc<Mutex<HashMap<String, Option<Page>>>>;

fn pages(open: &Open) -> std::sync::MutexGuard<'_, HashMap<String, Option<Page>>> {
    open.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An open page: its id, and the write that records its row.
pub struct Viewer {
    id: String,
    written: tokio::task::JoinHandle<()>,
}

/// Records an open page. The row is written by a task of its own, so a
/// slow database never holds up the page's feed.
pub fn opened(state: &AppState, peer: IpAddr, headers: HeaderMap) -> Viewer {
    let id = crate::sessions::random_hex(16);
    pages(&state.viewers).insert(id.clone(), None);
    let (task_state, task_id) = (state.clone(), id.clone());
    let written = tokio::spawn(async move {
        if let Err(error) = record(&task_state, &task_id, peer, &headers).await {
            crate::log::error(format!("viewers: {error}"));
        }
    });
    Viewer { id, written }
}

async fn record(state: &AppState, id: &str, peer: IpAddr, headers: &HeaderMap) -> sqlx::Result<()> {
    let trusted = client::trusted_proxies(&state.pool).await?;
    let address = client::address(peer, headers, &trusted);
    let admin = auth::session(&state.pool, headers)
        .await?
        .map(|session| session.login);
    let user_agent: String = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(USER_AGENT)
        .collect();
    let connected_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO viewers (id, host, address, user_agent, admin, connected_at, seen_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now()) RETURNING connected_at",
    )
    .bind(id)
    .bind(&state.host)
    .bind(address.to_string())
    .bind(&user_agent)
    .bind(&admin)
    .fetch_one(&state.pool)
    .await?;
    if let Some(page) = pages(&state.viewers).get_mut(id) {
        *page = Some(Page {
            address: address.to_string(),
            user_agent,
            admin,
            connected_at,
        });
    }
    Ok(())
}

/// Removes a closed page's row, once the write that recorded it is done.
pub fn closed(state: &AppState, viewer: Viewer) {
    pages(&state.viewers).remove(&viewer.id);
    let state = state.clone();
    tokio::spawn(async move {
        let _ = viewer.written.await;
        if let Err(error) = sqlx::query("DELETE FROM viewers WHERE id = $1")
            .bind(&viewer.id)
            .execute(&state.pool)
            .await
        {
            crate::log::error(format!("viewers: {error}"));
        }
    });
}

/// Writes again the row of every page this host holds open, removes its
/// rows of pages it no longer holds, and removes every row a stopped host
/// left behind. Only rows written before the refresh began are removed: a
/// page opened meanwhile keeps its row.
pub async fn refresh(state: &AppState) -> sqlx::Result<()> {
    let began: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&state.pool)
        .await?;
    let (open, known): (Vec<String>, Vec<(String, Page)>) = {
        let pages = pages(&state.viewers);
        (
            pages.keys().cloned().collect(),
            pages
                .iter()
                .filter_map(|(id, page)| Some((id.clone(), page.clone()?)))
                .collect(),
        )
    };
    let ids: Vec<&str> = known.iter().map(|(id, _)| id.as_str()).collect();
    let addresses: Vec<&str> = known.iter().map(|(_, p)| p.address.as_str()).collect();
    let agents: Vec<&str> = known.iter().map(|(_, p)| p.user_agent.as_str()).collect();
    let admins: Vec<Option<&str>> = known.iter().map(|(_, p)| p.admin.as_deref()).collect();
    let since: Vec<DateTime<Utc>> = known.iter().map(|(_, p)| p.connected_at).collect();
    sqlx::query(
        "INSERT INTO viewers (id, host, address, user_agent, admin, connected_at, seen_at) \
         SELECT u.id, $1, u.address, u.agent, u.admin, u.since, now() \
         FROM unnest($2::text[], $3::text[], $4::text[], $5::text[], $6::timestamptz[]) \
              AS u(id, address, agent, admin, since) \
         ON CONFLICT (id) DO UPDATE SET seen_at = now()",
    )
    .bind(&state.host)
    .bind(&ids)
    .bind(&addresses)
    .bind(&agents)
    .bind(&admins)
    .bind(&since)
    .execute(&state.pool)
    .await?;
    sqlx::query("DELETE FROM viewers WHERE host = $1 AND NOT (id = ANY($2)) AND connected_at < $3")
        .bind(&state.host)
        .bind(&open)
        .bind(began)
        .execute(&state.pool)
        .await?;
    sqlx::query("DELETE FROM viewers WHERE seen_at < now() - make_interval(secs => $1)")
        .bind(f64::from(STALE_SECONDS))
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Refreshes every `REFRESH` until the process ends.
pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(REFRESH);
        loop {
            tick.tick().await;
            if let Err(error) = refresh(&state).await {
                crate::log::error(format!("viewers: {error}"));
            }
        }
    });
}

/// An address on this network or a private one, rather than the Internet's.
pub fn local(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.to_ipv4_mapped().is_some_and(|v4| local(IpAddr::V4(v4)))
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
struct Row {
    host: String,
    address: String,
    user_agent: String,
    admin: Option<String>,
    connected_at: DateTime<Utc>,
}

/// `GET /api/admin/viewers`: the open pages of every host.
pub async fn list(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT host, address, user_agent, admin, connected_at FROM viewers \
         WHERE seen_at >= now() - make_interval(secs => $1) ORDER BY connected_at",
    )
    .bind(f64::from(STALE_SECONDS))
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(json!(
        rows.into_iter()
            .map(|row| {
                json!({
                    "host": row.host,
                    "local": row.address.parse().is_ok_and(local),
                    "address": row.address,
                    "user_agent": row.user_agent,
                    "admin": row.admin,
                    "connected_at": row.connected_at,
                })
            })
            .collect::<Vec<_>>()
    )))
}
