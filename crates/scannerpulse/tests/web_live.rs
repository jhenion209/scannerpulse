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
    let state = AppState::new(pool.clone(), (*pool.connect_options()).clone());
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
