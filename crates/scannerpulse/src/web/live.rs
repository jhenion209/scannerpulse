//! `/api/live`: new calls and transcripts as they happen.
//!
//! One listener per host hears ingest's notifications, reads each new call or
//! transcript once, and passes it to every open page of the host; each page
//! keeps only its talkgroups. A page that reconnects says the last call it
//! saw, and gets the calls it missed first.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use tokio::sync::broadcast;

use super::AppState;
use crate::listing::Row;

/// The events a host can hold for a slow page before that page misses some
/// and is told to read again.
pub const BACKLOG: usize = 1024;
/// The most missed calls sent to a page that reconnects; beyond that it is
/// told to read again.
pub const CATCH_UP: i64 = 500;

/// What a page hears.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Call {
        call: Row,
    },
    Transcript {
        id: i64,
        talkgroup_id: Option<i32>,
        transcript: Option<String>,
        origin: Option<String>,
    },
    /// Events were lost: the page reads its list again.
    Resync,
}

impl Event {
    fn talkgroup(&self) -> Option<i32> {
        match self {
            Event::Call { call } => call.talkgroup_id,
            Event::Transcript { talkgroup_id, .. } => *talkgroup_id,
            Event::Resync => None,
        }
    }
}

pub type Feed = broadcast::Sender<Arc<Event>>;

pub fn feed() -> Feed {
    broadcast::channel(BACKLOG).0
}

/// Listens for the host's feed until the process ends; a lost connection is
/// opened again after a pause. Once it is listening again the pages are told
/// to read again, so nothing sent while it was down is missed.
pub fn spawn(pool: PgPool, feed: Feed) {
    tokio::spawn(async move {
        let mut lost = false;
        loop {
            if let Err(error) = listen(&pool, &feed, lost).await {
                crate::log::error(format!("live feed: {error}"));
                lost = true;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

async fn listen(pool: &PgPool, feed: &Feed, lost: bool) -> sqlx::Result<()> {
    let mut listener = PgListener::connect_with(pool).await?;
    listener.ignore_pool_close_event(true);
    listener
        .listen_all(["scannerpulse_calls", "scannerpulse_transcripts"])
        .await?;
    if lost {
        let _ = feed.send(Arc::new(Event::Resync));
    }
    loop {
        // `try_recv` gives `None` when the connection was lost: notifications
        // sent meanwhile are gone, so the pages must read again.
        let Some(note) = listener.try_recv().await? else {
            return Err(sqlx::Error::Protocol(
                "the listening connection was lost".into(),
            ));
        };
        let Ok(id) = note.payload().parse::<i64>() else {
            continue;
        };
        let event = match note.channel() {
            "scannerpulse_calls" => call(pool, id).await?.map(|call| Event::Call { call }),
            _ => transcript(pool, id).await?,
        };
        if let Some(event) = event {
            let _ = feed.send(Arc::new(event));
        }
    }
}

const ROW: &str = "SELECT c.id, c.started_at, c.duration_ms, c.talkgroup_id, t.number AS talkgroup, \
     t.name AS talkgroup_name, s.name AS system, si.name AS site, c.radio_id, c.radio_alias, \
     tr.text AS transcript, tr.origin \
     FROM calls c \
     LEFT JOIN talkgroups t ON t.id = c.talkgroup_id \
     LEFT JOIN systems s ON s.id = t.system_id \
     LEFT JOIN sites si ON si.id = c.site_id \
     LEFT JOIN transcripts tr ON tr.call_id = c.id AND tr.started_at = c.started_at";

async fn call(pool: &PgPool, id: i64) -> sqlx::Result<Option<Row>> {
    sqlx::query_as(&format!("{ROW} WHERE c.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await
}

async fn transcript(pool: &PgPool, id: i64) -> sqlx::Result<Option<Event>> {
    Ok(call(pool, id).await?.map(|row| Event::Transcript {
        id: row.id,
        talkgroup_id: row.talkgroup_id,
        transcript: row.transcript,
        origin: row.origin,
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct LiveParams {
    /// Talkgroup ids, separated by commas; none means every talkgroup.
    pub talkgroups: Option<String>,
    /// The last call id the page saw.
    pub since: Option<i64>,
}

pub async fn socket(
    State(state): State<AppState>,
    Query(params): Query<LiveParams>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |ws| serve(state, params, ws))
}

async fn serve(state: AppState, params: LiveParams, mut ws: WebSocket) {
    let talkgroups: HashSet<i32> = params
        .talkgroups
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter_map(|id| id.trim().parse().ok())
        .collect();
    let wanted = |event: &Event| {
        talkgroups.is_empty()
            || matches!(event, Event::Resync)
            || event.talkgroup().is_some_and(|t| talkgroups.contains(&t))
    };
    // Listen before catching up, so nothing falls between the two.
    let mut events = state.live.subscribe();
    let mut stop = state.shutdown.subscribe();
    // `subscribe` marks the current value as seen: a stop that fired before
    // this page connected would never be reported by `changed`.
    let stopped = *stop.borrow();
    if stopped {
        close(&mut ws).await;
        return;
    }
    // The calls sent by the catch-up that may still arrive live: each is
    // skipped once, then forgotten, so the set is at most `CATCH_UP` long.
    let mut sent: HashSet<i64> = HashSet::new();
    if let Some(since) = params.since {
        let missed: sqlx::Result<Vec<Row>> =
            sqlx::query_as(&format!("{ROW} WHERE c.id > $1 ORDER BY c.id LIMIT $2"))
                .bind(since)
                .bind(CATCH_UP + 1)
                .fetch_all(&state.pool)
                .await;
        let missed = match missed {
            Ok(missed) if missed.len() as i64 <= CATCH_UP => missed,
            _ => {
                let _ = send(&mut ws, &Event::Resync).await;
                Vec::new()
            }
        };
        for call in missed {
            sent.insert(call.id);
            let event = Event::Call { call };
            if wanted(&event) && send(&mut ws, &event).await.is_err() {
                return;
            }
        }
    }
    loop {
        tokio::select! {
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => Arc::new(Event::Resync),
                    Err(broadcast::error::RecvError::Closed) => return,
                };
                if let Event::Call { call } = &*event {
                    if sent.remove(&call.id) {
                        continue;
                    }
                }
                if wanted(&event) && send(&mut ws, &event).await.is_err() {
                    return;
                }
            }
            incoming = ws.recv() => match incoming {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            },
            _ = stop.changed() => {
                close(&mut ws).await;
                return;
            }
        }
    }
}

async fn send(ws: &mut WebSocket, event: &Event) -> Result<(), axum::Error> {
    let text =
        serde_json::to_string(event).unwrap_or_else(|_| json!({"type": "resync"}).to_string());
    ws.send(Message::Text(text.into())).await
}

/// Sends the Close frame, waiting at most a second: a client that stopped
/// reading must not hold the task.
async fn close(ws: &mut WebSocket) {
    let _ = tokio::time::timeout(Duration::from_secs(1), ws.send(Message::Close(None))).await;
}
