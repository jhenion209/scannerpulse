mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn signed(pool: &PgPool, app: &axum::Router) -> (String, String) {
    admin(pool, "ann", PASSWORD).await;
    sign_in(app, "ann", PASSWORD).await
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_turns_a_talkgroups_transcription_on_and_off(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    common::talkgroup(&pool, "SYSA", 3).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/talkgroups", &[], None)
            .await
            .status,
        401
    );
    let (cookie, csrf) = signed(&pool, &app).await;
    let list = send(
        &app,
        "GET",
        "/api/admin/talkgroups",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    let numbers: Vec<i64> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, [3, 7]);
    assert_eq!(list.body[1]["transcribe"], false);
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let uri = format!("/api/admin/talkgroups/{seven}/transcribe");
    let on = send(&app, "PUT", &uri, &change, Some(json!({"on": true}))).await;
    assert_eq!(on.status, 200);
    let stored: bool = sqlx::query_scalar("SELECT transcribe FROM talkgroups WHERE id = $1")
        .bind(seven)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(stored);
    let unknown = send(
        &app,
        "PUT",
        "/api/admin/talkgroups/999999/transcribe",
        &change,
        Some(json!({"on": true})),
    )
    .await;
    assert_eq!(unknown.status, 404);
    let without = send(
        &app,
        "PUT",
        &uri,
        &[("cookie", &cookie)],
        Some(json!({"on": false})),
    )
    .await;
    assert_eq!(without.status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_puts_the_failed_calls_back_in_the_queue(pool: PgPool) {
    let talkgroup = common::talkgroup(&pool, "SYSA", 7).await;
    sqlx::query("UPDATE talkgroups SET transcribe = true")
        .execute(&pool)
        .await
        .unwrap();
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    for n in 0..3 {
        let at = at + chrono::Duration::minutes(n);
        let id = common::call(&pool, talkgroup, at, None).await;
        sqlx::query(
            "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, attempts, state, last_error) \
             VALUES ($1, $2, now(), now(), 4, $3, 'down')",
        )
        .bind(id)
        .bind(at)
        .bind(if n < 2 { "failed" } else { "queued" })
        .execute(&pool)
        .await
        .unwrap();
    }
    let app = app(&pool);
    let (cookie, csrf) = signed(&pool, &app).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let retried = send(&app, "POST", "/api/admin/queue/retry", &change, None).await;
    assert_eq!(retried.body, json!({"retried": 2}));
    let (queued, attempts): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'queued'), sum(attempts) FROM transcription_queue",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        (queued, attempts),
        (3, 4),
        "the retried calls start again from no attempt"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_retry_leaves_the_failed_calls_of_a_talkgroup_that_is_switched_off(pool: PgPool) {
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    let mut ids = Vec::new();
    for (number, on) in [(7, false), (8, true)] {
        let talkgroup = common::talkgroup(&pool, "SYSA", number).await;
        sqlx::query("UPDATE talkgroups SET transcribe = $2 WHERE id = $1")
            .bind(talkgroup)
            .bind(on)
            .execute(&pool)
            .await
            .unwrap();
        let id = common::call(&pool, talkgroup, at, None).await;
        sqlx::query(
            "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, attempts, state, last_error) \
             VALUES ($1, $2, now(), now(), 4, 'failed', 'down')",
        )
        .bind(id)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    let app = app(&pool);
    let (cookie, csrf) = signed(&pool, &app).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let retried = send(&app, "POST", "/api/admin/queue/retry", &change, None).await;
    assert_eq!(retried.body, json!({"retried": 1}));
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM transcription_queue ORDER BY call_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, ["failed", "queued"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_improvement_modules_sets_and_report(pool: PgPool) {
    let (_root, _) = common::labeled_set(
        &pool,
        &[("engine one", "engine one"), ("medic two", "medic")],
    )
    .await;
    let app = app(&pool);
    let (cookie, _) = signed(&pool, &app).await;
    let read = [("cookie", cookie.as_str())];
    assert_eq!(
        send(&app, "GET", "/api/admin/asr/sets", &read, None)
            .await
            .status,
        403
    );
    sqlx::query("INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    let sets = send(&app, "GET", "/api/admin/asr/sets", &read, None).await;
    assert_eq!(
        sets.body,
        json!([{"set": "s1", "size": 2, "labeled": 2, "runs": 0}])
    );
    let report = send(&app, "GET", "/api/admin/asr/report?set=s1", &read, None).await;
    assert_eq!(report.status, 200, "{:?}", report.body);
    assert_eq!(report.body["labeled"], 2);
    assert_eq!(report.body["production"]["words"], 4);
    assert_eq!(report.body["production"]["errors"], 1);
    assert!(report.body["runs"].as_array().unwrap().is_empty());
    let unknown = send(
        &app,
        "GET",
        "/api/admin/asr/report?set=nothing",
        &read,
        None,
    )
    .await;
    assert_eq!(unknown.status, 404);
}
