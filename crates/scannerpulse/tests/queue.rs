mod common;

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use scannerpulse::queue::{self, Past, Queued, Written};
use scannerpulse::servers::ServerState;
use scannerpulse::store::RecordError;
use sqlx::PgPool;
use sqlx::postgres::PgListener;

const HOLDER: &str = "host-a 100-1900000000";

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

async fn places(pool: &PgPool) -> (i16, i16) {
    let (location, source) = common::seed_places(pool, Path::new("/arch"), Path::new("/in")).await;
    (location, source)
}

async fn enqueue(pool: &PgPool, call: (i64, DateTime<Utc>)) {
    sqlx::query(
        "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
         VALUES ($1, $2, now(), now(), 'queued')",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(pool)
    .await
    .unwrap();
}

/// One queued call at `at`; returns it.
async fn queued_call(pool: &PgPool, at: &str) -> (i64, DateTime<Utc>) {
    let started = utc(at);
    let (id, _) = common::insert_call(
        pool,
        places_of(pool).await,
        &format!("{at}.mp3"),
        started,
        2000,
        true,
    )
    .await;
    enqueue(pool, (id, started)).await;
    (id, started)
}

async fn places_of(pool: &PgPool) -> (i16, i16) {
    let found: Option<(i16, i16)> = sqlx::query_as(
        "SELECT (SELECT id FROM locations LIMIT 1), (SELECT id FROM sources LIMIT 1) \
         WHERE EXISTS (SELECT 1 FROM locations)",
    )
    .fetch_optional(pool)
    .await
    .unwrap();
    match found {
        Some(ids) => ids,
        None => places(pool).await,
    }
}

async fn row(
    pool: &PgPool,
    call: (i64, DateTime<Utc>),
) -> Option<(i16, String, f64, Option<String>)> {
    sqlx::query_as(
        "SELECT attempts, state, extract(epoch FROM next_try_at - now())::float8, last_error \
         FROM transcription_queue WHERE call_id = $1 AND started_at = $2",
    )
    .bind(call.0)
    .bind(call.1)
    .fetch_optional(pool)
    .await
    .unwrap()
}

fn as_queued(call: (i64, DateTime<Utc>), attempts: i16) -> Queued {
    Queued {
        call_id: call.0,
        started_at: call.1,
        attempts,
        root: "/arch".to_string(),
        rel_path: "x.mp3".to_string(),
    }
}

#[test]
fn the_waits_are_one_minute_ten_minutes_one_hour_and_six_hours() {
    let waits: Vec<u64> = (1..=4)
        .map(|n| queue::retry_after(n).unwrap().as_secs())
        .collect();
    assert_eq!(waits, [60, 600, 3600, 21600]);
    assert_eq!(queue::retry_after(5), None);
    assert_eq!(queue::retry_after(0), None);
}

#[sqlx::test(migrations = "./migrations")]
async fn due_calls_come_newest_first_without_those_in_flight(pool: PgPool) -> sqlx::Result<()> {
    let older = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let later = queued_call(&pool, "2031-01-15T18:05:00Z").await;
    let newest = queued_call(&pool, "2031-01-15T18:10:00Z").await;
    sqlx::query(
        "UPDATE transcription_queue SET next_try_at = now() + interval '1 hour' WHERE call_id = $1",
    )
    .bind(later.0)
    .execute(&pool)
    .await?;
    let ids = |due: Vec<Queued>| due.into_iter().map(|q| q.call_id).collect::<Vec<i64>>();
    assert_eq!(ids(queue::due(&pool, 10, &[]).await?), [newest.0, older.0]);
    assert_eq!(ids(queue::due(&pool, 10, &[newest.0]).await?), [older.0]);
    assert_eq!(ids(queue::due(&pool, 1, &[]).await?), [newest.0]);
    let first = &queue::due(&pool, 1, &[]).await?[0];
    assert_eq!(first.path(), Path::new("/arch/2031-01-15T18:10:00Z.mp3"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_machine_s_text_is_written_and_announced(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("scannerpulse_transcripts").await?;
    let mut conn = pool.acquire().await?;
    let written = queue::write(
        &mut conn,
        HOLDER,
        call,
        "  Engine 5 responding. ",
        "model-x",
    )
    .await;
    assert_eq!(written.unwrap(), Written::Text);
    let (text, origin, model, state): (String, String, Option<String>, String) = sqlx::query_as(
        "SELECT t.text, t.origin, t.model, c.transcript_state FROM transcripts t \
         JOIN calls c ON c.id = t.call_id AND c.started_at = t.started_at",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        (
            text.as_str(),
            origin.as_str(),
            model.as_deref(),
            state.as_str()
        ),
        ("Engine 5 responding.", "machine", Some("model-x"), "text")
    );
    assert_eq!(row(&pool, call).await, None);
    let notice = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .unwrap()?;
    assert_eq!(notice.payload(), call.0.to_string());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn empty_text_is_no_speech(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let mut conn = pool.acquire().await?;
    assert_eq!(
        queue::write(&mut conn, HOLDER, call, "   ", "model-x")
            .await
            .unwrap(),
        Written::NoSpeech
    );
    let texts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await?;
    let state: String = sqlx::query_scalar("SELECT transcript_state FROM calls")
        .fetch_one(&pool)
        .await?;
    assert_eq!((texts, state.as_str()), (0, "no_speech"));
    assert_eq!(row(&pool, call).await, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_person_s_text_is_never_replaced(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin) VALUES ($1, $2, 'Engine 5 on scene.', 'human')",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(&pool)
    .await?;
    let mut conn = pool.acquire().await?;
    let written = queue::write(&mut conn, HOLDER, call, "Engine five on seen", "model-x").await;
    assert_eq!(written.unwrap(), Written::KeptHuman);
    let text: String = sqlx::query_scalar("SELECT text FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(text, "Engine 5 on scene.");
    assert_eq!(row(&pool, call).await, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_transcript_for_a_deleted_call_is_dropped_quietly(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    sqlx::query("DELETE FROM calls WHERE id = $1")
        .bind(call.0)
        .execute(&pool)
        .await?;
    let mut conn = pool.acquire().await?;
    let written = queue::write(&mut conn, HOLDER, call, "Engine 5", "model-x").await;
    assert_eq!(written.unwrap(), Written::CallGone);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn without_the_lease_nothing_is_written(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, "host-b 200-1900000000").await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let mut conn = pool.acquire().await?;
    let written = queue::write(&mut conn, HOLDER, call, "Engine 5", "model-x").await;
    assert!(matches!(written, Err(RecordError::LeaseLost)));
    let counted = queue::count_failure(&mut conn, HOLDER, &as_queued(call, 0), "400").await;
    assert!(matches!(counted, Err(RecordError::LeaseLost)));
    let texts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(texts, 0);
    assert_eq!(
        row(&pool, call).await.map(|r| (r.0, r.1)),
        Some((0, "queued".to_string()))
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_failure_for_a_call_that_is_gone_is_not_counted(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    sqlx::query("DELETE FROM transcription_queue")
        .execute(&pool)
        .await?;
    let mut conn = pool.acquire().await?;
    let counted = queue::count_failure(&mut conn, HOLDER, &as_queued(call, 0), "400").await;
    assert_eq!(counted.unwrap(), None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn failures_wait_longer_each_time_and_the_fifth_is_for_good(
    pool: PgPool,
) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let mut conn = pool.acquire().await?;
    for (attempts, wait) in [(0, 60.0), (1, 600.0), (2, 3600.0), (3, 21600.0)] {
        let failed = queue::count_failure(
            &mut conn,
            HOLDER,
            &as_queued(call, attempts),
            "400: bad audio",
        )
        .await;
        assert_eq!(failed.unwrap(), Some(false));
        let (counted, state, left, error) = row(&pool, call).await.unwrap();
        assert_eq!(
            (counted, state.as_str(), error.as_deref()),
            (attempts + 1, "queued", Some("400: bad audio"))
        );
        assert!((left - wait).abs() < 5.0, "{attempts}: {left}");
    }
    assert!(
        queue::due(&pool, 10, &[]).await?.is_empty(),
        "nothing is due during the wait"
    );
    let failed =
        queue::count_failure(&mut conn, HOLDER, &as_queued(call, 4), "400: bad audio").await;
    assert_eq!(failed.unwrap(), Some(true));
    assert_eq!(
        row(&pool, call).await.map(|r| (r.0, r.1)),
        Some((5, "failed".to_string()))
    );
    let counts = queue::counts(&pool).await?;
    assert_eq!(
        (counts.queued, counts.failed, counts.last_minute),
        (0, 1, 1)
    );
    let listed = queue::failed(&pool, 10).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].last_error.as_deref(), Some("400: bad audio"));

    assert!(queue::retry(&pool, call).await?);
    assert_eq!(
        row(&pool, call).await.map(|r| (r.0, r.1, r.3)),
        Some((0, "queued".to_string(), None))
    );
    assert!(
        !queue::retry(&pool, call).await?,
        "only a failed call is put back"
    );
    assert_eq!(queue::counts(&pool).await?.due, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn past_calls_are_counted_and_queued_in_batches(pool: PgPool) -> sqlx::Result<()> {
    let places = places(&pool).await;
    let mut ids = Vec::new();
    for (minute, duration) in [(0, 2000), (1, 2000), (2, 2000), (3, 500)] {
        let at = utc(&format!("2031-01-15T18:0{minute}:00Z"));
        ids.push((
            common::insert_call(&pool, places, &format!("{minute}.mp3"), at, duration, true).await,
            at,
        ));
    }
    let talkgroup = ids[0].0.1;
    // One of the long calls is queued already, another has its text.
    enqueue(&pool, (ids[0].0.0, ids[0].1)).await;
    sqlx::query("UPDATE calls SET transcript_state = 'text' WHERE id = $1")
        .bind(ids[1].0.0)
        .execute(&pool)
        .await?;
    let past = Past {
        talkgroup_id: Some(talkgroup),
        from: Some(utc("2031-01-15T00:00:00Z")),
        until: Some(utc("2031-01-16T00:00:00Z")),
        min_ms: 1000,
    };
    assert_eq!(queue::count_past(&pool, &past).await?, 1);
    let (n, oldest) = queue::queue_past(&pool, &past, 10, None).await?;
    assert_eq!((n, oldest), (1, Some(ids[2].1)));
    assert_eq!(
        queue::queue_past(&pool, &past, 10, oldest).await?,
        (0, None)
    );
    assert_eq!(queue::queue_past(&pool, &past, 10, None).await?, (0, None));
    assert_eq!(queue::count_past(&pool, &past).await?, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_servers_state_is_kept_for_the_admin_page(pool: PgPool) -> sqlx::Result<()> {
    let state = |url: &str, cooling: Option<u64>, rounds: u32| ServerState {
        url: url.to_string(),
        cooling_for: cooling.map(Duration::from_secs),
        failed_rounds: rounds,
        last_error: cooling.map(|_| "refused".to_string()),
    };
    queue::save_servers(
        &pool,
        &[state("http://a", Some(30), 1), state("http://b", None, 0)],
    )
    .await?;
    let saved = queue::servers(&pool).await?;
    assert_eq!(saved.len(), 2);
    assert_eq!(
        (saved[0].failed_rounds, saved[0].last_error.as_deref()),
        (1, Some("refused"))
    );
    let left = (saved[0].cooling_until.unwrap() - saved[0].updated_at).num_seconds();
    assert!((29..=31).contains(&left), "{left}");
    assert_eq!(saved[1].cooling_until, None);
    queue::save_servers(&pool, &[state("http://b", None, 0)]).await?;
    let urls: Vec<String> = queue::servers(&pool)
        .await?
        .into_iter()
        .map(|s| s.url)
        .collect();
    assert_eq!(urls, ["http://b"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_person_s_save_in_progress_is_waited_for_and_kept(pool: PgPool) -> sqlx::Result<()> {
    common::hold_lease(&pool, HOLDER).await;
    let call = queued_call(&pool, "2031-01-15T18:00:00Z").await;
    let mut person = pool.begin().await?;
    sqlx::query("SELECT 1 FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE")
        .bind(call.0)
        .bind(call.1)
        .execute(&mut *person)
        .await?;
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin) VALUES ($1, $2, 'Engine 5 on scene.', 'human')",
    )
    .bind(call.0)
    .bind(call.1)
    .execute(&mut *person)
    .await?;
    let writer = {
        let pool = pool.clone();
        tokio::spawn(async move {
            let mut conn = pool.acquire().await.unwrap();
            queue::write(&mut conn, HOLDER, call, "machine text", "model-x").await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    person.commit().await?;
    let written = tokio::time::timeout(Duration::from_secs(10), writer)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(written.unwrap(), Written::KeptHuman);
    let text: String = sqlx::query_scalar("SELECT text FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(text, "Engine 5 on scene.");
    assert_eq!(row(&pool, call).await, None);
    Ok(())
}
