mod common;

use chrono::{Duration, Utc};
use common::{admin, app, send, sign_in};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn the_status_of_an_empty_site(pool: PgPool) {
    let status = scannerpulse::status::read(&pool).await.unwrap();
    assert_eq!(status["lease"]["valid"], false);
    assert_eq!(status["ingest"]["waiting"], 0);
    assert_eq!(status["queue"]["queued"], 0);
    assert_eq!(status["database"]["primary"], true);
    assert_eq!(status["maintenance"], false);
    assert!(status["database"].get("address").is_some());
    assert!(status["database"].get("port").is_some());
    assert_eq!(status["failover"]["jobs_stuck"], 0);
    assert_eq!(status["failover"]["calls_filed_twice"], 0);
    assert!(status["recent_calls"].as_array().unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn the_status_counts_what_needs_a_look(pool: PgPool) {
    let talkgroup = common::talkgroup(&pool, "SYSA", 7).await;
    let at = Utc::now() - Duration::hours(1);
    let newest = common::call(&pool, talkgroup, at, None).await;
    let source: i16 = sqlx::query_scalar("SELECT id FROM sources LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, reason) VALUES \
         ($1, 'a.mp3', now() - interval '2 hours', now(), 'waiting', NULL), \
         ($1, 'b.mp3', now(), now(), 'waiting', NULL), \
         ($1, 'c.mp3', now(), now(), 'rejected', 'not an mp3')",
    )
    .bind(source)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO jobs (kind, started_by, state, started_at, updated_at) VALUES \
         ('queue', 'ann', 'running', now() - interval '1 hour', now() - interval '11 minutes')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let status = scannerpulse::status::read(&pool).await.unwrap();
    assert_eq!(status["ingest"]["waiting"], 2);
    assert_eq!(status["ingest"]["rejected"], 1);
    assert_eq!(
        status["ingest"]["recent_rejected"][0]["reason"],
        "not an mp3"
    );
    assert_eq!(status["failover"]["files_left_in_sources"], 1);
    assert_eq!(status["failover"]["jobs_stuck"], 1);
    assert_eq!(status["recent_calls"][0]["id"], newest);
    assert_eq!(status["recent_calls"][0]["talkgroup"], 7);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_status_tab_is_for_admins(pool: PgPool) {
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/status", &[], None)
            .await
            .status,
        401
    );
    admin(&pool, "ann", "correct horse battery").await;
    let (cookie, _) = sign_in(&app, "ann", "correct horse battery").await;
    let answer = send(
        &app,
        "GET",
        "/api/admin/status",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body["database"]["primary"], true);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_locked_table_fails_the_status_within_its_limit(pool: PgPool) {
    use sqlx::Connection;
    let mut holder = pool.acquire().await.unwrap();
    let mut tx = holder.begin().await.unwrap();
    sqlx::query("LOCK TABLE ingest_files IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let read = scannerpulse::status::read_within(&pool, std::time::Duration::from_secs(1)).await;
    assert!(read.is_err(), "the reading waited for the lock");
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    tx.rollback().await.unwrap();
    assert!(scannerpulse::status::read(&pool).await.is_ok());
}
