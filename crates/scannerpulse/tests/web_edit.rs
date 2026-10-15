mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

/// One call with the machine's text; returns its id.
async fn machine_call(pool: &PgPool) -> i64 {
    let talkgroup = common::talkgroup(pool, "SYSA", 127).await;
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    common::call(pool, talkgroup, at, Some("engine for responding")).await
}

fn uri(id: i64) -> String {
    format!("/api/calls/{id}/transcript")
}

async fn labels(pool: &PgPool) -> Vec<(String, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT text, machine_text, labeler FROM transcript_labels ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_visitors_save_keeps_the_machines_text_beside_it(pool: PgPool) {
    let id = machine_call(&pool).await;
    let app = app(&pool);
    let saved = send(
        &app,
        "PATCH",
        &uri(id),
        &[],
        Some(json!({"text": " Engine 4 responding \n"})),
    )
    .await;
    assert_eq!(saved.status, 200, "{:?}", saved.body);
    assert_eq!(
        saved.body,
        json!({"id": id, "transcript": "Engine 4 responding", "origin": "human"})
    );
    assert_eq!(
        labels(&pool).await,
        [(
            "Engine 4 responding".into(),
            Some("engine for responding".into()),
            None
        )]
    );
    let call = send(&app, "GET", &format!("/api/calls/{id}"), &[], None).await;
    assert_eq!(call.body["transcript"], "Engine 4 responding");
    assert_eq!(call.body["origin"], "human");
    assert_eq!(call.body["labeled"], true);
    // A second save keeps the machine's first output, not the person's text.
    send(
        &app,
        "PATCH",
        &uri(id),
        &[],
        Some(json!({"text": "Engine 5 responding"})),
    )
    .await;
    assert_eq!(
        labels(&pool).await[1].1.as_deref(),
        Some("engine for responding")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admins_save_carries_the_admins_name(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let id = machine_call(&pool).await;
    let app = app(&pool);
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let text = Some(json!({"text": "Engine 4"}));
    let signed = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    assert_eq!(
        send(&app, "PATCH", &uri(id), &signed, text.clone())
            .await
            .status,
        200
    );
    // Without the token the save is a visitor's.
    assert_eq!(
        send(&app, "PATCH", &uri(id), &[("cookie", &cookie)], text)
            .await
            .status,
        200
    );
    let labelers: Vec<Option<String>> = labels(&pool).await.into_iter().map(|l| l.2).collect();
    assert_eq!(labelers, [Some("ann".to_string()), None]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_text_with_markup_control_characters_or_too_long_is_refused(pool: PgPool) {
    let id = machine_call(&pool).await;
    let app = app(&pool);
    let long = "a".repeat(4001);
    for bad in [
        "<b>fire</b>",
        "a > b",
        "tab\there",
        "line\r\nend",
        "nul\u{0}",
        long.as_str(),
    ] {
        let answer = send(&app, "PATCH", &uri(id), &[], Some(json!({"text": bad}))).await;
        assert_eq!(answer.status, 400, "{bad:?}");
    }
    assert!(labels(&pool).await.is_empty());
    let longest = "é".repeat(4000);
    for good in ["two\nlines", longest.as_str(), ""] {
        let answer = send(&app, "PATCH", &uri(id), &[], Some(json!({"text": good}))).await;
        assert_eq!(answer.status, 200, "{good:?}");
    }
    // An empty text marks the call as having no speech.
    let state: String = sqlx::query_scalar("SELECT transcript_state FROM calls WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "no_speech");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_call_is_not_found(pool: PgPool) {
    let answer = send(
        &app(&pool),
        "PATCH",
        &uri(424_242),
        &[],
        Some(json!({"text": "x"})),
    )
    .await;
    assert_eq!(answer.status, 404);
}

#[sqlx::test(migrations = "./migrations")]
async fn maintenance_pauses_the_saves(pool: PgPool) {
    let id = machine_call(&pool).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('maintenance.enabled', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    let answer = send(
        &app(&pool),
        "PATCH",
        &uri(id),
        &[],
        Some(json!({"text": "x"})),
    )
    .await;
    assert_eq!(answer.status, 503);
    assert!(labels(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn thirty_saves_a_minute_per_address(pool: PgPool) {
    let id = machine_call(&pool).await;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('web.trusted_proxies', '[\"198.51.100.7\"]')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app(&pool);
    let from = |ip: &'static str| [("x-forwarded-for", ip)];
    for n in 0..30 {
        let answer = send(
            &app,
            "PATCH",
            &uri(id),
            &from("203.0.113.9"),
            Some(json!({"text": format!("t{n}")})),
        )
        .await;
        assert_eq!(answer.status, 200, "save {n}");
    }
    let over = send(
        &app,
        "PATCH",
        &uri(id),
        &from("203.0.113.9"),
        Some(json!({"text": "t30"})),
    )
    .await;
    assert_eq!(over.status, 429);
    // Another address is counted on its own.
    let other = send(
        &app,
        "PATCH",
        &uri(id),
        &from("203.0.113.10"),
        Some(json!({"text": "u"})),
    )
    .await;
    assert_eq!(other.status, 200);
}
