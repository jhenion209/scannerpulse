mod common;

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use scannerpulse::queue::{self, Written};
use scannerpulse::transcripts;
use sqlx::PgPool;
use sqlx::postgres::PgListener;

const HOLDER: &str = "host-a 100-1900000000";

async fn call(pool: &PgPool) -> (i64, DateTime<Utc>) {
    let places = common::seed_places(pool, Path::new("/arch"), Path::new("/in")).await;
    let at = DateTime::parse_from_rfc3339("2031-01-15T18:00:00Z")
        .unwrap()
        .to_utc();
    let (id, _) = common::insert_call(pool, places, "a.mp3", at, 2000, true).await;
    (id, at)
}

async fn labels(pool: &PgPool) -> Vec<(String, Option<String>, String, Option<String>)> {
    sqlx::query_as("SELECT text, machine_text, source, labeler FROM transcript_labels ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn transcript(pool: &PgPool) -> (String, String, String) {
    sqlx::query_as(
        "SELECT t.text, t.origin, c.transcript_state FROM transcripts t \
         JOIN calls c ON c.id = t.call_id AND c.started_at = t.started_at",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_correction_keeps_the_machine_s_first_output_beside_it(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = call(&pool).await;
    let mut conn = pool.acquire().await?;
    queue::write(&mut conn, HOLDER, call, "Engine five on seen", "model-x")
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
         VALUES ($1, $2, now(), now(), 'failed')",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(&pool)
    .await?;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("scannerpulse_transcripts").await?;

    assert!(transcripts::correct(&mut conn, call, " Engine 5 on scene. ", Some("admin")).await?);
    assert_eq!(
        labels(&pool).await,
        [(
            "Engine 5 on scene.".to_string(),
            Some("Engine five on seen".to_string()),
            "user_edit".to_string(),
            Some("admin".to_string())
        )]
    );
    assert_eq!(
        transcript(&pool).await,
        (
            "Engine 5 on scene.".to_string(),
            "human".to_string(),
            "text".to_string()
        )
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM transcription_queue")
        .fetch_one(&pool)
        .await?;
    assert_eq!(left, 0);
    let notice = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .unwrap()?;
    assert_eq!(notice.payload(), call.0.to_string());

    // A later edit carries the machine's first output, not the person's text.
    assert!(transcripts::correct(&mut conn, call, "Engine 5 on scene, command.", None).await?);
    let all = labels(&pool).await;
    assert_eq!(all[1].1.as_deref(), Some("Engine five on seen"));
    assert_eq!(all[1].3, None);

    // The machine never replaces it.
    sqlx::query(
        "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
         VALUES ($1, $2, now(), now(), 'queued')",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(&pool)
    .await?;
    let written = queue::write(&mut conn, HOLDER, call, "something else", "model-x")
        .await
        .unwrap();
    assert_eq!(written, Written::KeptHuman);
    assert_eq!(transcript(&pool).await.0, "Engine 5 on scene, command.");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_without_machine_text_and_an_empty_correction(pool: PgPool) -> sqlx::Result<()> {
    let call = call(&pool).await;
    let mut conn = pool.acquire().await?;
    assert!(transcripts::correct(&mut conn, call, "  ", None).await?);
    assert_eq!(labels(&pool).await[0].1, None);
    assert_eq!(
        transcript(&pool).await,
        (String::new(), "human".to_string(), "no_speech".to_string())
    );
    assert!(!transcripts::correct(&mut conn, (call.0 + 1, call.1), "x", None).await?);
    Ok(())
}
