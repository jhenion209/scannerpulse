mod common;

use chrono::{DateTime, Duration, TimeZone, Utc};
use scannerpulse::evaluation::{self, Draw, EvalError};
use sqlx::PgPool;

const ZONE: chrono_tz::Tz = chrono_tz::UTC;

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2031, 3, 20, 12, 0, 0).unwrap()
}

/// A call of SYSA's talkgroup `number`, `ms` long, in `state`, at `at`.
async fn call(pool: &PgPool, number: i64, at: DateTime<Utc>, ms: i32, state: &str) -> i64 {
    let places = places(pool).await;
    scannerpulse::store::ensure_partition(pool, at)
        .await
        .unwrap();
    let talkgroup: i32 = sqlx::query_scalar(
        "INSERT INTO talkgroups (system_id, number, name, transcribe) \
         VALUES ((SELECT id FROM systems WHERE name = 'SYSA'), $1, 'Group ' || $1, true) \
         ON CONFLICT (system_id, number) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, \
                            rel_path, transcript_state) \
         VALUES ($1, $2, $3, $4, $5, gen_random_uuid()::text || '.mp3', $6) RETURNING id",
    )
    .bind(at)
    .bind(ms)
    .bind(talkgroup)
    .bind(places.1)
    .bind(places.0)
    .bind(state)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The location and source, made once, and the system SYSA.
async fn places(pool: &PgPool) -> (i16, i16) {
    let found: Option<(i16, i16)> = sqlx::query_as(
        "SELECT (SELECT id FROM locations LIMIT 1), (SELECT id FROM sources LIMIT 1) \
         WHERE EXISTS (SELECT 1 FROM locations)",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    if let Some(found) = found {
        return found;
    }
    sqlx::query("INSERT INTO systems (name) VALUES ('SYSA')")
        .execute(pool)
        .await
        .unwrap();
    common::seed_places(
        pool,
        std::path::Path::new("/nonexistent/archive"),
        std::path::Path::new("/nonexistent/in"),
    )
    .await
}

fn ask(set: &str, numbers: &[i64], calls: usize) -> Draw {
    Draw {
        set: set.to_string(),
        talkgroups: numbers.iter().map(|n| ("SYSA".to_string(), *n)).collect(),
        calls,
        days: 60,
        min_ms: 1000,
        seed: 1,
    }
}

async fn drawn(pool: &PgPool, set: &str) -> Vec<i64> {
    sqlx::query_scalar("SELECT call_id FROM eval_calls WHERE set_name = $1 ORDER BY call_id")
        .bind(set)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_draw_shares_the_calls_among_talkgroups_and_months(pool: PgPool) {
    let feb = Utc.with_ymd_and_hms(2031, 2, 10, 9, 0, 0).unwrap();
    let mar = Utc.with_ymd_and_hms(2031, 3, 10, 9, 0, 0).unwrap();
    for i in 0..10 {
        call(&pool, 127, feb + Duration::minutes(i), 2000, "text").await;
    }
    for i in 0..2 {
        call(&pool, 127, mar + Duration::minutes(i), 2000, "no_speech").await;
    }
    for i in 0..3 {
        call(&pool, 128, mar + Duration::minutes(i), 2000, "text").await;
    }
    // Not drawn: too short, never transcribed, or before the window.
    call(&pool, 128, mar, 999, "text").await;
    call(&pool, 128, mar, 2000, "untried").await;
    call(&pool, 128, now() - Duration::days(61), 2000, "text").await;

    let mut conn = pool.acquire().await.unwrap();
    let result = evaluation::draw(&mut conn, &ask("s1", &[127, 128], 8), now(), ZONE)
        .await
        .unwrap();
    // 128 has 3 calls, less than its share of 4, so 127 takes the other 5:
    // both of its March calls and 3 of February's.
    let counts: Vec<(i64, usize, usize)> = result
        .talkgroups
        .iter()
        .map(|t| (t.number, t.eligible, t.drawn))
        .collect();
    assert_eq!(counts, vec![(127, 12, 5), (128, 3, 3)]);
    assert_eq!(result.total(), 8);
    let months: Vec<(String, i64)> = sqlx::query_as(
        "SELECT to_char(c.started_at, 'YYYY-MM'), count(*) FROM eval_calls e \
         JOIN calls c ON c.id = e.call_id AND c.started_at = e.started_at \
         JOIN talkgroups t ON t.id = c.talkgroup_id WHERE t.number = 127 GROUP BY 1 ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(months, vec![("2031-02".into(), 3), ("2031-03".into(), 2)]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_talkgroup_listed_twice_counts_once(pool: PgPool) {
    let feb = Utc.with_ymd_and_hms(2031, 2, 10, 9, 0, 0).unwrap();
    for n in 0..6 {
        call(&pool, 127, feb + Duration::minutes(n), 2000, "text").await;
    }
    let mut conn = pool.acquire().await.unwrap();
    let once = evaluation::draw(&mut conn, &ask("once", &[127], 4), now(), ZONE)
        .await
        .unwrap();
    let twice = evaluation::draw(&mut conn, &ask("twice", &[127, 127], 4), now(), ZONE)
        .await
        .unwrap();
    assert_eq!(twice.talkgroups.len(), 1);
    assert_eq!(twice.talkgroups[0].drawn, once.talkgroups[0].drawn);
    assert_eq!(drawn(&pool, "twice").await, drawn(&pool, "once").await);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_same_seed_draws_the_same_calls(pool: PgPool) {
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap();
    for i in 0..40 {
        call(&pool, 127, at + Duration::minutes(i), 2000, "text").await;
    }
    let mut conn = pool.acquire().await.unwrap();
    evaluation::draw(&mut conn, &ask("s1", &[127], 10), now(), ZONE)
        .await
        .unwrap();
    let first = drawn(&pool, "s1").await;
    let again = evaluation::draw(&mut conn, &ask("s1", &[127], 10), now(), ZONE)
        .await
        .unwrap();
    assert_eq!(again.replaced, 10);
    assert_eq!(drawn(&pool, "s1").await, first);
    let mut other = ask("s2", &[127], 10);
    other.seed = 2;
    evaluation::draw(&mut conn, &other, now(), ZONE)
        .await
        .unwrap();
    assert_ne!(drawn(&pool, "s2").await, first);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_set_with_labels_is_never_replaced(pool: PgPool) {
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap();
    for i in 0..5 {
        call(&pool, 127, at + Duration::minutes(i), 2000, "text").await;
    }
    let mut conn = pool.acquire().await.unwrap();
    evaluation::draw(&mut conn, &ask("s1", &[127], 3), now(), ZONE)
        .await
        .unwrap();
    let before = drawn(&pool, "s1").await;
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, source) \
         SELECT call_id, started_at, 'engine two', 'user_edit' FROM eval_calls LIMIT 1",
    )
    .execute(&pool)
    .await
    .unwrap();
    let refused = evaluation::draw(&mut conn, &ask("s1", &[127], 5), now(), ZONE).await;
    assert!(
        matches!(refused, Err(EvalError::Labeled { labeled: 1, .. })),
        "{refused:?}"
    );
    assert_eq!(drawn(&pool, "s1").await, before);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_talkgroup_is_refused_before_anything_is_written(pool: PgPool) {
    call(&pool, 127, now() - Duration::days(1), 2000, "text").await;
    let mut conn = pool.acquire().await.unwrap();
    let refused = evaluation::draw(&mut conn, &ask("s1", &[127, 999], 5), now(), ZONE).await;
    assert!(
        matches!(&refused, Err(EvalError::UnknownTalkgroup(name)) if name == "SYSA/999"),
        "{refused:?}"
    );
    assert_eq!(evaluation::set_size(&pool, "s1").await.unwrap(), 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_labelers_list_is_the_set_without_its_labeled_calls(pool: PgPool) {
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        ids.push(call(&pool, 127, at + Duration::minutes(i), 2000, "text").await);
    }
    let mut conn = pool.acquire().await.unwrap();
    evaluation::draw(&mut conn, &ask("s1", &[127], 3), now(), ZONE)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, source) \
         SELECT id, started_at, '', 'user_edit' FROM calls WHERE id = $1",
    )
    .bind(ids[1])
    .execute(&pool)
    .await
    .unwrap();
    let list = evaluation::to_label(&pool, "s1").await.unwrap();
    let listed: Vec<i64> = list.iter().map(|c| c.call_id).collect();
    assert_eq!(listed, vec![ids[0], ids[2]]);
    assert_eq!(list[0].system, "SYSA");
    assert_eq!(list[0].number, 127);
    assert_eq!(list[0].talkgroup, "Group 127");
    assert_eq!(evaluation::set_size(&pool, "s1").await.unwrap(), 3);
}
