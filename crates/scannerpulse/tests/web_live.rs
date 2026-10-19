mod common;

use std::time::Duration;

use chrono::{TimeZone, Utc};
use common::{call, talkgroup};
use futures_util::StreamExt;
use scannerpulse::web::{self, AppState, live};
use serde_json::Value;
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::Message;

/// The router served on a port of its own, with the host's feed listening.
async fn serve(pool: &PgPool) -> String {
    let state = AppState::new(pool.clone());
    live::spawn(pool.clone(), state.live.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, web::router(state)).await.unwrap() });
    format!("ws://{address}/api/live")
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(ws: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(10), ws.next())
            .await
            .expect("an event within 10 s")
            .unwrap()
            .unwrap();
        if let Message::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn notify(pool: &PgPool, channel: &str, id: i64) {
    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(channel)
        .bind(id.to_string())
        .execute(pool)
        .await
        .unwrap();
}

/// Notifies `id` until the socket hears it: the host's listener starts on
/// its own and may not listen yet.
async fn first_call(pool: &PgPool, ws: &mut Socket, id: i64) -> Value {
    for _ in 0..50 {
        notify(pool, "scannerpulse_calls", id).await;
        if let Ok(Some(Ok(Message::Text(text)))) =
            tokio::time::timeout(Duration::from_millis(200), ws.next()).await
        {
            return serde_json::from_str(&text).unwrap();
        }
    }
    panic!("the feed never heard call {id}");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_page_hears_its_talkgroups_calls_and_transcripts(pool: PgPool) {
    let mine = talkgroup(&pool, "SYSA", 1).await;
    let other = talkgroup(&pool, "SYSA", 2).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let url = serve(&pool).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{url}?talkgroups={mine}"))
        .await
        .unwrap();
    let first = call(&pool, mine, at(0), None).await;
    let heard = first_call(&pool, &mut ws, first).await;
    assert_eq!(heard["type"], "call");
    assert_eq!(heard["call"]["id"], first);
    // Another talkgroup's call is not sent; the next call of mine is, and it
    // is the next message.
    let theirs = call(&pool, other, at(1), None).await;
    notify(&pool, "scannerpulse_calls", theirs).await;
    let second = call(&pool, mine, at(2), Some("engine five")).await;
    notify(&pool, "scannerpulse_calls", second).await;
    assert_eq!(next(&mut ws).await["call"]["id"], second);
    notify(&pool, "scannerpulse_transcripts", second).await;
    let transcript = next(&mut ws).await;
    assert_eq!(transcript["type"], "transcript");
    assert_eq!(transcript["id"], second);
    assert_eq!(transcript["transcript"], "engine five");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_page_that_reconnects_gets_the_calls_it_missed_first(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let seen = call(&pool, tg, at(0), None).await;
    let missed_a = call(&pool, tg, at(1), None).await;
    let missed_b = call(&pool, tg, at(2), None).await;
    let url = serve(&pool).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{url}?since={seen}"))
        .await
        .unwrap();
    assert_eq!(next(&mut ws).await["call"]["id"], missed_a);
    assert_eq!(next(&mut ws).await["call"]["id"], missed_b);
    // A missed call heard again live is not sent twice.
    let later = call(&pool, tg, at(3), None).await;
    notify(&pool, "scannerpulse_calls", missed_b).await;
    let heard = first_call(&pool, &mut ws, later).await;
    assert_eq!(heard["call"]["id"], later);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_page_hears_new_calls_again_once_the_lost_listener_is_back(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let url = serve(&pool).await;
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("{url}?talkgroups={tg}"))
        .await
        .unwrap();
    let before = call(&pool, tg, at(0), None).await;
    first_call(&pool, &mut ws, before).await;
    // Only this database's listening sessions: the other tests have their own.
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE query ILIKE 'LISTEN%' AND datname = current_database() AND pid <> pg_backend_pid()",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Stale messages may still arrive before the resync: skip them.
    loop {
        if next(&mut ws).await["type"] == "resync" {
            break;
        }
    }
    // The page reads again once it is told to, so a call notified now must
    // reach it at once, without a retry.
    let later = call(&pool, tg, at(1), None).await;
    notify(&pool, "scannerpulse_calls", later).await;
    assert_eq!(next(&mut ws).await["call"]["id"], later);
}

#[tokio::test]
async fn a_page_that_connects_during_shutdown_is_closed_at_once() {
    // A state whose shutdown fired before the page connected; no database is
    // needed, because the socket closes before it reads any call.
    let pool = PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused").unwrap();
    let state = AppState::new(pool.clone());
    state.shutdown.send_replace(true);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, web::router(state)).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/live"))
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("the socket closes within 5 s");
    assert!(
        !matches!(end, Some(Ok(Message::Text(_)))),
        "no event is sent after shutdown"
    );
}

#[tokio::test]
async fn a_page_that_sends_a_large_message_is_dropped() {
    // The page never needs to send anything, so the socket's incoming
    // messages are kept small; no database is needed.
    let pool = PgPool::connect_lazy("postgres://unused@127.0.0.1:1/unused").unwrap();
    let state = AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, web::router(state)).await.unwrap() });
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{address}/api/live"))
        .await
        .unwrap();
    futures_util::SinkExt::send(&mut ws, Message::Text("x".repeat(8192).into()))
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("the socket closes within 5 s");
    assert!(
        !matches!(end, Some(Ok(Message::Text(_)))),
        "no event is sent to it"
    );
}
