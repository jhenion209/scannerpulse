mod common;

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use scannerpulse::queue;
use scannerpulse::ratings;
use scannerpulse::transcripts;
use sqlx::PgPool;

const HOLDER: &str = "host-a 100-1900000000";

async fn call(pool: &PgPool, name: &str, hour: u32) -> (i64, DateTime<Utc>) {
    // The places are made once for a database; a second call finds them.
    let places: Option<(i16, i16)> = sqlx::query_as(
        "SELECT l.id, s.id FROM locations l, sources s WHERE l.name = 'primary' LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    let places = match places {
        Some(places) => places,
        None => common::seed_places(pool, Path::new("/arch"), Path::new("/in")).await,
    };
    let at = DateTime::parse_from_rfc3339(&format!("2031-01-15T{hour:02}:00:00Z"))
        .unwrap()
        .to_utc();
    let (id, _) = common::insert_call(pool, places, name, at, 2000, true).await;
    (id, at)
}

async fn transcribed(pool: &PgPool, name: &str, hour: u32, model: &str) -> (i64, DateTime<Utc>) {
    common::hold_lease(pool, HOLDER).await;
    let call = call(pool, name, hour).await;
    let mut conn = pool.acquire().await.unwrap();
    queue::write(&mut conn, HOLDER, call, name, "Engine five", model)
        .await
        .unwrap();
    call
}

async fn count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM transcript_ratings")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[test]
fn the_stored_rater_is_a_sha256_and_not_the_token() {
    let r = ratings::rater("abc");
    assert_eq!(r.len(), 64);
    assert_eq!(
        r,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rating_is_one_per_browser_and_can_change_or_clear(pool: PgPool) {
    let call = transcribed(&pool, "a.mp3", 18, "model-x").await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(ratings::set(&mut conn, call, "r1", 4).await.unwrap());
    assert!(ratings::set(&mut conn, call, "r1", 2).await.unwrap());
    assert!(ratings::set(&mut conn, call, "r2", 5).await.unwrap());
    assert_eq!(count(&pool).await, 2);
    let (stars, model, origin): (i16, Option<String>, String) =
        sqlx::query_as("SELECT stars, model, origin FROM transcript_ratings WHERE rater = 'r1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (stars, model.as_deref(), origin.as_str()),
        (2, Some("model-x"), "machine")
    );

    assert!(ratings::clear(&mut conn, call, "r1").await.unwrap());
    assert!(!ratings::clear(&mut conn, call, "r1").await.unwrap());
    assert_eq!(count(&pool).await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_with_no_transcript_cannot_be_rated(pool: PgPool) {
    let call = call(&pool, "a.mp3", 18).await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(!ratings::set(&mut conn, call, "r1", 3).await.unwrap());
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn stars_outside_one_to_five_are_refused_by_the_database(pool: PgPool) {
    let call = transcribed(&pool, "a.mp3", 18, "model-x").await;
    let mut conn = pool.acquire().await.unwrap();
    for stars in [0, 6, -1] {
        assert!(ratings::set(&mut conn, call, "r1", stars).await.is_err());
    }
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rating_keeps_what_was_judged_and_goes_with_its_call(pool: PgPool) {
    let call = transcribed(&pool, "a.mp3", 18, "model-x").await;
    let mut conn = pool.acquire().await.unwrap();
    assert!(ratings::set(&mut conn, call, "r1", 1).await.unwrap());
    // A person's edit does not rewrite the earlier rating; a new rating
    // names the model that wrote the first text and the human origin.
    assert!(
        transcripts::correct(&mut conn, call, "Engine 5", None)
            .await
            .unwrap()
    );
    assert!(ratings::set(&mut conn, call, "r2", 5).await.unwrap());
    let rows: Vec<(String, Option<String>, String)> =
        sqlx::query_as("SELECT rater, model, origin FROM transcript_ratings ORDER BY rater")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        [
            ("r1".into(), Some("model-x".into()), "machine".into()),
            ("r2".into(), Some("model-x".into()), "human".into()),
        ]
    );

    sqlx::query("DELETE FROM calls WHERE id = $1 AND started_at = $2")
        .bind(call.0)
        .bind(call.1)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count(&pool).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_report_groups_by_model_and_origin(pool: PgPool) {
    let a = transcribed(&pool, "a.mp3", 18, "model-x").await;
    let b = transcribed(&pool, "b.mp3", 19, "model-y").await;
    let mut conn = pool.acquire().await.unwrap();
    for (call, rater, stars) in [(a, "r1", 5), (a, "r2", 3), (b, "r1", 2)] {
        assert!(ratings::set(&mut conn, call, rater, stars).await.unwrap());
    }
    let report = ratings::report(&mut conn, None).await.unwrap();
    assert_eq!(report.len(), 2);
    let x = &report[0];
    assert_eq!(x.model.as_deref(), Some("model-x"));
    assert_eq!((x.ratings, x.calls), (2, 1));
    assert!((x.mean - 4.0).abs() < 1e-9);
    assert_eq!((x.three, x.five, x.one), (1, 1, 0));
    assert_eq!(report[1].model.as_deref(), Some("model-y"));
    assert_eq!(report[1].two, 1);

    let none = ratings::report(&mut conn, Some(Utc::now() + Duration::hours(1)))
        .await
        .unwrap();
    assert!(none.is_empty());
}
