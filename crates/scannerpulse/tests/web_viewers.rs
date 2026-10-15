mod common;

use std::net::SocketAddr;
use std::time::Duration;

use common::{admin, send, sign_in};
use futures_util::SinkExt;
use scannerpulse::web::{self, AppState, viewers};
use sqlx::PgPool;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const PASSWORD: &str = "correct horse battery";

/// The router on a port of its own, with each connection's address, as
/// `web` serves it.
async fn serve(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            web::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    address
}

async fn rows(pool: &PgPool) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query_as("SELECT host, address, user_agent, admin FROM viewers ORDER BY admin NULLS FIRST, connected_at")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Waits until `viewers` holds `n` rows.
async fn until_rows(pool: &PgPool, n: usize) {
    for _ in 0..100 {
        if rows(pool).await.len() == n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("viewers never held {n} rows: {:?}", rows(pool).await);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_open_page_is_a_viewer_until_it_closes(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let mut state = AppState::new(pool.clone(), (*pool.connect_options()).clone());
    state.host = "web-a".into();
    let address = serve(state.clone()).await;
    let (cookie, _) = sign_in(&common::app(&pool), "ann", PASSWORD).await;
    let mut visitor = format!("ws://{address}/api/live")
        .into_client_request()
        .unwrap();
    visitor
        .headers_mut()
        .insert("user-agent", "TestBrowser/1.0".parse().unwrap());
    let (mut page, _) = tokio_tungstenite::connect_async(visitor).await.unwrap();
    let mut signed = format!("ws://{address}/api/live")
        .into_client_request()
        .unwrap();
    signed
        .headers_mut()
        .insert("cookie", cookie.parse().unwrap());
    let (mut admin_page, _) = tokio_tungstenite::connect_async(signed).await.unwrap();
    until_rows(&pool, 2).await;
    let seen = rows(&pool).await;
    assert_eq!(
        seen[0],
        (
            "web-a".into(),
            "127.0.0.1".into(),
            "TestBrowser/1.0".into(),
            None
        )
    );
    assert_eq!(seen[1].3.as_deref(), Some("ann"));
    // The admin list shows both, marked local.
    let (cookie, _) = sign_in(&common::app(&pool), "ann", PASSWORD).await;
    let list = send(
        &common::app(&pool),
        "GET",
        "/api/admin/viewers",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body.as_array().unwrap().len(), 2);
    assert_eq!(list.body[0]["local"], true);
    assert_eq!(list.body[0]["host"], "web-a");
    assert_eq!(
        send(&common::app(&pool), "GET", "/api/admin/viewers", &[], None)
            .await
            .status,
        401
    );
    page.send(Message::Close(None)).await.unwrap();
    until_rows(&pool, 1).await;
    admin_page.send(Message::Close(None)).await.unwrap();
    until_rows(&pool, 0).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_refresh_keeps_the_open_pages_and_clears_the_rest(pool: PgPool) {
    let mut state = AppState::new(pool.clone(), (*pool.connect_options()).clone());
    state.host = "web-a".into();
    let page = viewers::Page {
        address: "192.0.2.9".into(),
        user_agent: String::new(),
        admin: None,
        connected_at: chrono::Utc::now(),
    };
    state
        .viewers
        .lock()
        .unwrap()
        .insert("open".into(), Some(page.clone()));
    // A page whose row was lost (a failed write, or another host's pruning).
    state
        .viewers
        .lock()
        .unwrap()
        .insert("lost".into(), Some(page));
    sqlx::query(
        "INSERT INTO viewers (id, host, address, user_agent, connected_at, seen_at) VALUES \
         ('open', 'web-a', '192.0.2.9', '', now(), now() - interval '60 seconds'), \
         ('ghost', 'web-a', '192.0.2.9', '', now(), now()), \
         ('other', 'web-b', '192.0.2.9', '', now(), now()), \
         ('stopped', 'web-c', '192.0.2.9', '', now(), now() - interval '91 seconds'), \
         ('opening', 'web-a', '192.0.2.9', '', now() + interval '1 hour', now())",
    )
    .execute(&pool)
    .await
    .unwrap();
    viewers::refresh(&state).await.unwrap();
    let left: Vec<(String, bool)> = sqlx::query_as(
        "SELECT id, seen_at > now() - interval '5 seconds' FROM viewers ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    // This host's closed page and a stopped host's page are gone; the other
    // host's page is that host's to keep; a lost row is written again; a
    // row written after the refresh began (a page opening meanwhile) stays.
    assert_eq!(
        left,
        [
            ("lost".into(), true),
            ("open".into(), true),
            ("opening".into(), true),
            ("other".into(), true)
        ]
    );
}

#[test]
fn local_addresses_are_told_from_the_internets() {
    for ip in [
        "127.0.0.1",
        "169.254.1.1",
        "fd00::1",
        "fe80::1",
        "::1",
        "::ffff:127.0.0.1",
    ] {
        assert!(viewers::local(ip.parse().unwrap()), "{ip}");
    }
    for ip in [
        "203.0.113.9",
        "198.51.100.7",
        "2001:db8::1",
        "::ffff:203.0.113.9",
    ] {
        assert!(!viewers::local(ip.parse().unwrap()), "{ip}");
    }
}
