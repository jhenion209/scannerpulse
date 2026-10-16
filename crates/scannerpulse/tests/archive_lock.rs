//! The archive lock is one per process: this test holds it, so it runs in a
//! test binary of its own.
mod common;

use chrono::{TimeZone, Utc};
use scannerpulse::talkgroups;
use sqlx::PgPool;

async fn rel_of(pool: &PgPool, call: i64) -> String {
    sqlx::query_scalar("SELECT rel_path FROM calls WHERE id = $1")
        .bind(call)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn run_jobs(pool: &PgPool) {
    let context = scannerpulse::jobs::Context::new(None);
    while scannerpulse::jobs::step(pool, "host-a", &context)
        .await
        .unwrap()
        .is_some()
    {}
}

/// A recorder's tag for SYSA, with `title` when given.
fn tag(title: Option<&str>) -> id3::Tag {
    use id3::TagLike;
    let mut tag = id3::Tag::new();
    if let Some(title) = title {
        tag.set_title(title);
    }
    tag.set_artist("711351");
    tag.set_text("TIT1", "SYSA");
    tag
}

/// Writes `data` at `rel` under `root` (unless it is `None`) and records its
/// call of SYSA/127 at `at`.
async fn filed(
    pool: &PgPool,
    places: (i16, i16),
    root: &std::path::Path,
    rel: &str,
    data: Option<Vec<u8>>,
    at: chrono::DateTime<Utc>,
) -> (i64, i32) {
    if let Some(data) = data {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }
    common::insert_call(pool, places, rel, at, 2000, false).await
}

/// `n` calls of SYSA/127, one a minute from 2031-03-01 00:00, each with its
/// file in `<day>/127-Alpha Fire/`; their ids and the talkgroup.
async fn alpha_calls(
    pool: &PgPool,
    places: (i16, i16),
    root: &std::path::Path,
    n: i64,
) -> (Vec<i64>, i32) {
    let mut calls = Vec::new();
    let mut talkgroup = 0;
    for i in 0..n {
        let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap() + chrono::Duration::minutes(i);
        let name = format!("{}SYSA.mp3", at.format("%Y%m%d_%H%M%S"));
        let (call, id) = filed(
            pool,
            places,
            root,
            &format!("2031-03-01/127-Alpha Fire/{name}"),
            Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
            at,
        )
        .await;
        calls.push(call);
        talkgroup = id;
    }
    (calls, talkgroup)
}

#[sqlx::test(migrations = "./migrations")]
async fn a_correction_step_never_waits_long_for_the_archive_lock(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let (calls, talkgroup) = alpha_calls(&pool, places, &root, 1).await;
    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha 2")
        .await
        .unwrap();
    let (job, _) = scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    let context = scannerpulse::jobs::Context::new(None);
    let held = scannerpulse::archive::WRITES.lock().await;
    let started = std::time::Instant::now();
    let step = scannerpulse::jobs::step(&pool, "host-a", &context).await;
    assert!(step.is_err(), "the step gave up");
    assert!(started.elapsed() < std::time::Duration::from_secs(8));
    let state: String = sqlx::query_scalar("SELECT state FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "queued", "the step rolled back");
    assert!(
        rel_of(&pool, calls[0])
            .await
            .starts_with("2031-03-01/127-Alpha Fire/")
    );
    drop(held);
    run_jobs(&pool).await;
    assert!(
        rel_of(&pool, calls[0])
            .await
            .starts_with("2031-03-01/127-Alpha 2/")
    );
}
