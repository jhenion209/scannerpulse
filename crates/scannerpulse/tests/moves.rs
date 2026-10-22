mod common;

use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::UTC;
use scannerpulse::jobs::{self, Context, JobError};
use scannerpulse::{moves, storage};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 1-1";

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

/// The main archive and the location Cold, both usable on host-a, whose
/// ingest runs.
struct Site {
    pool: PgPool,
    archive: TempDir,
    cold: TempDir,
    primary: i16,
    source: i16,
    cold_id: i16,
}

impl Site {
    async fn new(pool: PgPool) -> Site {
        let (archive, cold) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (primary, source) =
            common::seed_places(&pool, archive.path(), Path::new("/srv/in/live")).await;
        let mut conn = pool.acquire().await.unwrap();
        let cold_id = storage::add(&mut conn, "Cold", cold.path(), None, |folder| {
            storage::check_here(folder, "Cold")
        })
        .await
        .unwrap();
        common::hold_lease(&pool, HOLDER).await;
        let site = Site {
            pool,
            archive,
            cold,
            primary,
            source,
            cold_id,
        };
        site.beat("host-a").await;
        site.usable("host-a").await;
        site
    }

    async fn beat(&self, host: &str) {
        sqlx::query(
            "INSERT INTO leases (name, holder, expires_at, renewed_at) \
             VALUES ('ingest:' || $1, $1, now() + interval '1 minute', now())",
        )
        .bind(host)
        .execute(&self.pool)
        .await
        .unwrap();
    }

    async fn usable(&self, host: &str) {
        storage::record_health(
            &self.pool,
            host,
            &[
                (self.primary, Ok((5 << 40, 10 << 40))),
                (self.cold_id, Ok((5 << 40, 10 << 40))),
            ],
        )
        .await
        .unwrap();
    }

    /// A call at `at` whose file of `bytes` is `rel` in the main archive.
    async fn call(&self, rel: &str, at: DateTime<Utc>, bytes: &[u8]) -> i64 {
        let path = self.archive.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
        common::insert_call(
            &self.pool,
            (self.primary, self.source),
            rel,
            at,
            2000,
            false,
        )
        .await
        .0
    }

    async fn place(&self, id: i64) -> (i16, String) {
        sqlx::query_as("SELECT location_id, rel_path FROM calls WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn preview(
        &self,
        to: &str,
        from: &str,
        until: &str,
    ) -> Result<moves::MovePreview, JobError> {
        let mut conn = self.pool.acquire().await.unwrap();
        moves::preview(&mut conn, to, day(from), day(until), UTC).await
    }

    async fn start(
        &self,
        to: &str,
        from: &str,
        until: &str,
        confirm: &str,
    ) -> Result<i64, JobError> {
        let mut conn = self.pool.acquire().await.unwrap();
        moves::start(&mut conn, to, day(from), day(until), UTC, confirm, "test")
            .await
            .map(|(job, _)| job)
    }

    async fn run(&self) {
        let context = Context::new(None);
        while jobs::step(&self.pool, "host-a", &context)
            .await
            .unwrap()
            .is_some()
        {}
    }

    async fn job(&self, id: i64) -> (String, String, i64, Option<i64>) {
        sqlx::query_as("SELECT state, message, done, total FROM jobs WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn sweep(&self) {
        let mut filer = scannerpulse::filer::Filer::new(
            self.pool.clone(),
            common::HeldFence::new(HOLDER),
            scannerpulse::store::Location {
                id: self.primary,
                root: self.archive.path().to_path_buf(),
            },
            UTC,
            Duration::from_secs(900),
        )
        .with_host("host-a");
        filer.pass(1000).await.unwrap();
    }
}

fn march(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 3, 1, 12, minute, 0).unwrap()
}

fn refused<T: std::fmt::Debug>(result: Result<T, JobError>) -> String {
    match result {
        Err(JobError::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_is_counted_and_measured_first(pool: PgPool) {
    let site = Site::new(pool).await;
    for n in 0..3 {
        site.call(&format!("2025-03-01/1-A/{n}.mp3"), march(n), &[7; 100])
            .await;
    }
    site.call(
        "2025-04-01/1-A/later.mp3",
        Utc.with_ymd_and_hms(2025, 4, 1, 0, 0, 0).unwrap(),
        &[7; 100],
    )
    .await;
    let preview = site
        .preview("Cold", "2025-03-01", "2025-03-31")
        .await
        .unwrap();
    assert_eq!(preview.calls, 3);
    assert_eq!(preview.held.len(), 1);
    assert_eq!(
        (preview.held[0].location.as_str(), preview.held[0].calls),
        ("primary", 3)
    );
    assert_eq!(preview.bytes_about, Some(300));
    assert!(preview.free_bytes.unwrap() > 0);
    assert_eq!(preview.refused, None);
    // Nothing changed.
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE location_id = $1")
        .bind(site.primary)
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(left, 4);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_is_refused_what_it_cannot_do(pool: PgPool) {
    let site = Site::new(pool).await;
    site.call("2025-03-01/1-A/a.mp3", march(0), &[7; 100]).await;
    let why = refused(site.preview("Nowhere", "2025-03-01", "2025-03-31").await);
    assert_eq!(why, "there is no location named Nowhere");
    let why = refused(site.preview("Cold", "2025-03-31", "2025-03-01").await);
    assert_eq!(why, "2025-03-01 is before 2025-03-31");
    let empty = site
        .preview("Cold", "2024-01-01", "2024-12-31")
        .await
        .unwrap();
    assert_eq!(
        empty.refused.as_deref(),
        Some("no call of 2024-01-01 to 2024-12-31 is outside Cold")
    );
    let why = refused(site.start("Cold", "2025-03-01", "2025-03-31", "mov").await);
    assert_eq!(why, "type move to confirm");
    // A second host runs, and has not found Cold usable yet.
    site.beat("host-b").await;
    let held = site
        .preview("Cold", "2025-03-01", "2025-03-31")
        .await
        .unwrap();
    assert_eq!(held.calls, 1);
    assert_eq!(
        held.refused.as_deref(),
        Some("host-b has not checked Cold yet: wait a minute")
    );
    let why = refused(site.start("Cold", "2025-03-01", "2025-03-31", "move").await);
    assert!(why.contains("host-b has not checked Cold"), "{why}");
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(jobs, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_copies_checks_and_points_each_call_and_the_filer_deletes_the_old_file(
    pool: PgPool,
) {
    let site = Site::new(pool).await;
    let mut calls = Vec::new();
    for n in 0..3 {
        let rel = format!("2025-03-01/1-A/{n}.mp3");
        calls.push((site.call(&rel, march(n), &[n as u8; 100]).await, rel));
    }
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    assert_eq!(site.job(job).await.3, Some(3));
    site.run().await;
    let (state, message, done, _) = site.job(job).await;
    assert_eq!(state, "done");
    assert_eq!(done, 3);
    assert_eq!(
        message,
        "3 calls of 2025-03-01 to 2025-03-31 moved to Cold; 0 missing, 0 changed while they were moved, 0 refused"
    );
    for (n, (id, rel)) in calls.iter().enumerate() {
        assert_eq!(site.place(*id).await, (site.cold_id, rel.clone()));
        let copy = site.cold.path().join(rel);
        assert_eq!(std::fs::read(&copy).unwrap(), vec![n as u8; 100]);
        assert_eq!(
            std::fs::metadata(&copy).unwrap().modified().unwrap(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
            "the copy keeps the file's time"
        );
        assert!(
            site.archive.path().join(rel).exists(),
            "the old file waits for the filer"
        );
    }
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM archive_leftovers WHERE location_id = $1")
            .bind(site.primary)
            .fetch_one(&site.pool)
            .await
            .unwrap();
    assert_eq!(left, 3);
    site.sweep().await;
    for (_, rel) in &calls {
        assert!(!site.archive.path().join(rel).exists());
    }
    // And back to the main archive.
    let job = site
        .start("primary", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    site.run().await;
    assert_eq!(site.job(job).await.0, "done");
    for (id, rel) in &calls {
        assert_eq!(site.place(*id).await, (site.primary, rel.clone()));
        assert!(site.archive.path().join(rel).exists());
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_back_never_takes_an_old_file_still_waiting_for_its_deletion(pool: PgPool) {
    let site = Site::new(pool).await;
    let first = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    let second = site.call("2025-03-01/1-A/b.mp3", march(1), b"call b").await;
    site.start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    site.run().await;
    // Moved back before the sweep reached the old files, which are still
    // in the main archive at the same paths.
    site.start("primary", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    // The step waits for the second call after taking the first: the sweep
    // runs meanwhile, as the filer's does every second.
    let mut holding = site.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM calls WHERE id = $1 FOR UPDATE")
        .bind(second)
        .execute(&mut *holding)
        .await
        .unwrap();
    let pool = site.pool.clone();
    let stepping = tokio::spawn(async move {
        jobs::step(&pool, "host-a", &Context::new(None))
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    site.sweep().await;
    holding.commit().await.unwrap();
    stepping.await.unwrap();
    site.run().await;
    site.sweep().await;
    site.sweep().await;
    for (id, bytes) in [(first, &b"call a"[..]), (second, &b"call b"[..])] {
        let (location, rel) = site.place(id).await;
        assert_eq!(location, site.primary);
        let file = site.archive.path().join(&rel);
        assert_eq!(
            std::fs::read(&file).ok().as_deref(),
            Some(bytes),
            "call {id}: {rel}"
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_between_two_paths_of_one_folder_is_refused(pool: PgPool) {
    let site = Site::new(pool).await;
    let id = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    // Cold is the main archive's folder reached by another path, as a bind
    // mount or a second mount of one share would be.
    let links = tempfile::tempdir().unwrap();
    let same = links.path().join("archive");
    std::os::unix::fs::symlink(site.archive.path(), &same).unwrap();
    std::fs::write(site.archive.path().join(storage::MARKER), b"").unwrap();
    sqlx::query("UPDATE locations SET root = $2 WHERE id = $1")
        .bind(site.cold_id)
        .bind(same.to_str().unwrap())
        .execute(&site.pool)
        .await
        .unwrap();
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    site.run().await;
    site.sweep().await;
    let (state, message, ..) = site.job(job).await;
    assert_eq!(state, "failed");
    assert!(message.contains("one folder"), "{message}");
    assert_eq!(site.place(id).await.0, site.primary);
    let file = site.archive.path().join("2025-03-01/1-A/a.mp3");
    assert_eq!(std::fs::read(file).ok().as_deref(), Some(&b"call a"[..]));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_that_stops_answering_fails_the_move_instead_of_holding_it(pool: PgPool) {
    let site = Site::new(pool).await;
    let id = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    // The call's file stops answering, as a hung share's would: a pipe
    // nobody writes holds its reader for ever.
    let pipe = site.archive.path().join("2025-03-01/1-A/a.mp3");
    std::fs::remove_file(&pipe).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap()
            .success()
    );
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    let mut context = Context::new(None);
    context.file_limit = Duration::from_secs(2);
    let (pool, handle) = (site.pool.clone(), tokio::runtime::Handle::current());
    let stepping =
        tokio::task::spawn_blocking(move || handle.block_on(jobs::step(&pool, "host-a", &context)));
    let ended = tokio::time::timeout(Duration::from_secs(20), stepping).await;
    // The reader left waiting ends either way.
    std::fs::write(&pipe, b"").unwrap();
    assert!(ended.is_ok(), "the step waited on the file for ever");
    let (state, message, ..) = site.job(job).await;
    assert_eq!(state, "failed");
    assert!(message.contains("did not answer"), "{message}");
    assert_eq!(site.place(id).await.0, site.primary);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_copy_left_there_is_taken_and_another_file_of_its_name_is_kept(pool: PgPool) {
    let site = Site::new(pool).await;
    let a = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    let b = site.call("2025-03-01/1-A/b.mp3", march(1), b"call b").await;
    let gone = site.call("2025-03-01/1-A/gone.mp3", march(2), b"x").await;
    std::fs::remove_file(site.archive.path().join("2025-03-01/1-A/gone.mp3")).unwrap();
    let dir = site.cold.path().join("2025-03-01/1-A");
    std::fs::create_dir_all(&dir).unwrap();
    // A step that did not commit left a's copy; b's name holds another file.
    std::fs::write(dir.join("a.mp3"), b"call a").unwrap();
    std::fs::write(dir.join("b.mp3"), b"not call b").unwrap();
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    site.run().await;
    let (state, message, ..) = site.job(job).await;
    assert_eq!(state, "done");
    assert!(
        message.contains("2 calls of") && message.contains("1 missing"),
        "{message}"
    );
    assert_eq!(
        site.place(a).await,
        (site.cold_id, "2025-03-01/1-A/a.mp3".into())
    );
    assert_eq!(
        site.place(b).await,
        (site.cold_id, "2025-03-01/1-A/b_1.mp3".into())
    );
    assert_eq!(std::fs::read(dir.join("b.mp3")).unwrap(), b"not call b");
    assert_eq!(std::fs::read(dir.join("b_1.mp3")).unwrap(), b"call b");
    assert_eq!(
        site.place(gone).await.0,
        site.primary,
        "a call without its file stays"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_move_started_again_takes_the_calls_left(pool: PgPool) {
    let site = Site::new(pool).await;
    let count = moves::FILES_PER_STEP + 5;
    for n in 0..count {
        let at = march(0) + chrono::Duration::seconds(n);
        site.call(&format!("2025-03-01/1-A/{n}.mp3"), at, b"call")
            .await;
    }
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    let context = Context::new(None);
    jobs::step(&site.pool, "host-a", &context).await.unwrap();
    let mut conn = site.pool.acquire().await.unwrap();
    jobs::stop(&mut conn, job, "test").await.unwrap();
    site.run().await;
    let (state, _, done, _) = site.job(job).await;
    assert_eq!((state.as_str(), done), ("stopped", moves::FILES_PER_STEP));
    let preview = site
        .preview("Cold", "2025-03-01", "2025-03-31")
        .await
        .unwrap();
    assert_eq!(preview.calls, 5);
    let again = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    site.run().await;
    assert_eq!(site.job(again).await.0, "done");
    let in_cold: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE location_id = $1")
        .bind(site.cold_id)
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(in_cold, count);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_location_that_is_not_mounted_holds_the_move(pool: PgPool) {
    let site = Site::new(pool).await;
    let id = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    // Its share is unmounted: the mount point holds no marker.
    let marker = site.cold.path().join(storage::MARKER);
    let kept = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&marker).unwrap();
    let context = Context::new(None);
    let error = jobs::step(&site.pool, "host-a", &context)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("Cold is not mounted"), "{error}");
    assert_eq!(site.job(job).await.0, "queued");
    assert_eq!(site.place(id).await.0, site.primary);
    std::fs::write(&marker, kept).unwrap();
    site.run().await;
    assert_eq!(site.job(job).await.0, "done");
    assert_eq!(site.place(id).await.0, site.cold_id);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_stops_when_a_running_host_can_no_longer_use_its_destination(pool: PgPool) {
    let site = Site::new(pool).await;
    site.beat("host-b").await;
    site.usable("host-b").await;
    let id = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    // host-b loses Cold after the move started: its plays of moved calls would fail.
    storage::record_health(
        &site.pool,
        "host-b",
        &[(site.cold_id, Err("it cannot be read".into()))],
    )
    .await
    .unwrap();
    site.run().await;
    let (state, message, ..) = site.job(job).await;
    assert_eq!(state, "failed");
    assert!(message.contains("host-b cannot use Cold"), "{message}");
    assert!(message.contains("0 calls were moved"), "{message}");
    assert_eq!(site.place(id).await.0, site.primary);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_changed_while_its_step_waited_for_it_is_left_where_it_is(pool: PgPool) {
    let site = Site::new(pool).await;
    let id = site.call("2025-03-01/1-A/a.mp3", march(0), b"call a").await;
    let job = site
        .start("Cold", "2025-03-01", "2025-03-31", "move")
        .await
        .unwrap();
    // A correction holds the call and gives it another file.
    let mut holding = site.pool.begin().await.unwrap();
    sqlx::query("SELECT 1 FROM calls WHERE id = $1 FOR UPDATE")
        .bind(id)
        .execute(&mut *holding)
        .await
        .unwrap();
    let pool = site.pool.clone();
    let stepping = tokio::spawn(async move {
        jobs::step(&pool, "host-a", &Context::new(None))
            .await
            .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(500)).await;
    sqlx::query("UPDATE calls SET rel_path = '2025-03-01/2-B/a.mp3' WHERE id = $1")
        .bind(id)
        .execute(&mut *holding)
        .await
        .unwrap();
    holding.commit().await.unwrap();
    stepping.await.unwrap();
    site.run().await;
    let (state, message, ..) = site.job(job).await;
    assert_eq!(state, "done");
    assert!(
        message.contains("1 changed while they were moved"),
        "{message}"
    );
    assert_eq!(
        site.place(id).await,
        (site.primary, "2025-03-01/2-B/a.mp3".into())
    );
    assert!(!site.cold.path().join("2025-03-01/1-A/a.mp3").exists());
}
