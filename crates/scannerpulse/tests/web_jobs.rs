mod common;

use chrono::{Duration, TimeZone, Utc};
use common::{admin, app, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn signed(pool: &PgPool, app: &axum::Router) -> (String, String) {
    admin(pool, "ann", PASSWORD).await;
    sign_in(app, "ann", PASSWORD).await
}

/// Three untried calls of SYSA/7 on 2031-01-15, and transcription on with a
/// minimum of 1000 ms.
async fn calls(pool: &PgPool) -> i32 {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"UTC\"')")
        .execute(pool)
        .await
        .unwrap();
    common::set_transcription(pool, "http://127.0.0.1:9/v1/audio/transcriptions", 1000, 30).await;
    let talkgroup = common::talkgroup(pool, "SYSA", 7).await;
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    for n in 0..3 {
        common::call(pool, talkgroup, at + Duration::minutes(n), None).await;
    }
    talkgroup
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_previews_starts_follows_and_stops_a_queue_job(pool: PgPool) {
    let talkgroup = calls(&pool).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/jobs", &[], None).await.status,
        401
    );
    let (cookie, csrf) = signed(&pool, &app).await;
    let read = [("cookie", cookie.as_str())];
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let preview = send(
        &app,
        "GET",
        &format!("/api/admin/jobs/queue?talkgroup_id={talkgroup}&from=2031-01-15&to=2031-01-15"),
        &read,
        None,
    )
    .await;
    assert_eq!(preview.status, 200, "{:?}", preview.body);
    assert_eq!(preview.body, json!({"count": 3, "min_ms": 1000}));
    let outside = send(
        &app,
        "GET",
        &format!("/api/admin/jobs/queue?talkgroup_id={talkgroup}&from=2031-01-16"),
        &read,
        None,
    )
    .await;
    assert_eq!(outside.body["count"], 0);
    let started = send(
        &app,
        "POST",
        "/api/admin/jobs/queue",
        &change,
        Some(json!({"talkgroup_id": talkgroup})),
    )
    .await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    assert_eq!(started.body["count"], 3);
    let id = started.body["id"].as_i64().unwrap();
    let list = send(&app, "GET", "/api/admin/jobs", &read, None).await;
    assert_eq!(list.body[0]["id"], id);
    assert_eq!(list.body[0]["state"], "queued");
    assert_eq!(list.body[0]["started_by"], "ann");
    assert!(
        list.body[0].get("work").is_none(),
        "a job's working state is not shown"
    );
    let one = send(&app, "GET", &format!("/api/admin/jobs/{id}"), &read, None).await;
    assert_eq!(one.body["kind"], "queue");
    let stopped = send(
        &app,
        "POST",
        &format!("/api/admin/jobs/{id}/stop"),
        &change,
        None,
    )
    .await;
    assert_eq!(stopped.body["stopped"], "now");
    let again = send(
        &app,
        "POST",
        &format!("/api/admin/jobs/{id}/stop"),
        &change,
        None,
    )
    .await;
    assert_eq!(again.status, 409);
    assert_eq!(
        send(&app, "GET", "/api/admin/jobs/999999", &read, None)
            .await
            .status,
        404
    );
    // A change needs the CSRF token.
    let without = send(
        &app,
        "POST",
        "/api/admin/jobs/queue",
        &read,
        Some(json!({})),
    )
    .await;
    assert_eq!(without.status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_that_cannot_run_is_refused(pool: PgPool) {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"UTC\"')")
        .execute(&pool)
        .await
        .unwrap();
    let app = app(&pool);
    let (cookie, csrf) = signed(&pool, &app).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let off = send(
        &app,
        "POST",
        "/api/admin/jobs/queue",
        &change,
        Some(json!({})),
    )
    .await;
    assert_eq!(off.status, 400);
    assert!(
        off.body["detail"]
            .as_str()
            .unwrap()
            .contains("transcription is off")
    );
    let backwards = send(
        &app,
        "GET",
        "/api/admin/jobs/queue?from=2031-01-16&to=2031-01-15",
        &[("cookie", cookie.as_str())],
        None,
    )
    .await;
    assert_eq!(backwards.status, 400);
    let asr = send(
        &app,
        "POST",
        "/api/admin/jobs/asr-run",
        &change,
        Some(json!({"set": "s1", "server": "http://127.0.0.1:9/", "model": "m"})),
    )
    .await;
    assert_eq!(asr.status, 400, "the improvement module is off");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_asks_for_a_rescan(pool: PgPool) {
    let app = app(&pool);
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &[], None)
            .await
            .status,
        401
    );
    let (cookie, csrf) = signed(&pool, &app).await;
    let mut listener = sqlx::postgres::PgListener::connect_with(&pool)
        .await
        .unwrap();
    listener
        .listen(scannerpulse::ingest::RESCAN_CHANNEL)
        .await
        .unwrap();
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &change, None)
            .await
            .status,
        202
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv())
        .await
        .expect("the notice within 5 s")
        .unwrap();
}
