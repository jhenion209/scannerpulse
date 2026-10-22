mod common;

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::America::Chicago;
use scannerpulse::jobs::{self, Context, JobError};
use scannerpulse::{purges, storage};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 1-1";

fn day(text: &str) -> NaiveDate {
    text.parse().unwrap()
}

/// The main archive, the location Cold and the read-only location Old, all
/// usable on host-a, whose ingest runs.
struct Site {
    pool: PgPool,
    archive: TempDir,
    cold: TempDir,
    old: TempDir,
    primary: i16,
    source: i16,
    cold_id: i16,
    old_id: i16,
}

impl Site {
    async fn new(pool: PgPool) -> Site {
        let (archive, cold, old) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let (primary, source) =
            common::seed_places(&pool, archive.path(), Path::new("/srv/in/live")).await;
        let mut conn = pool.acquire().await.unwrap();
        let cold_id = storage::add(&mut conn, "Cold", cold.path(), None, |folder| {
            storage::check_here(folder, "Cold")
        })
        .await
        .unwrap();
        let old_id: i16 = sqlx::query_scalar(
            "INSERT INTO locations (name, root, mode) VALUES ('Old', $1, 'readonly') RETURNING id",
        )
        .bind(old.path().to_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        common::hold_lease(&pool, HOLDER).await;
        storage::record_health(
            &pool,
            "host-a",
            &[
                (primary, Ok((5 << 40, 10 << 40))),
                (cold_id, Ok((5 << 40, 10 << 40))),
                (old_id, Ok((5 << 40, 10 << 40))),
            ],
        )
        .await
        .unwrap();
        Site {
            pool,
            archive,
            cold,
            old,
            primary,
            source,
            cold_id,
            old_id,
        }
    }

    fn root(&self, location: i16) -> &Path {
        if location == self.primary {
            self.archive.path()
        } else if location == self.cold_id {
            self.cold.path()
        } else {
            self.old.path()
        }
    }

    /// A call at `at` in `location`, whose file `rel` holds `bytes`, with a
    /// transcript.
    async fn call(&self, location: i16, rel: &str, at: DateTime<Utc>, bytes: &[u8]) -> i64 {
        let path = self.root(location).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        let id = common::insert_call(&self.pool, (location, self.source), rel, at, 2000, false)
            .await
            .0;
        sqlx::query(
            "INSERT INTO transcripts (call_id, started_at, text, origin) VALUES ($1, $2, 'words', 'machine')",
        )
        .bind(id)
        .bind(at)
        .execute(&self.pool)
        .await
        .unwrap();
        id
    }

    async fn exists(&self, id: i64) -> bool {
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM calls WHERE id = $1)")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    async fn start(&self, before: &str, confirm: &str) -> Result<i64, JobError> {
        let mut conn = self.pool.acquire().await.unwrap();
        purges::start(&mut conn, day(before), Chicago, confirm, "test")
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

    async fn job(&self, id: i64) -> (String, String) {
        sqlx::query_as("SELECT state, message FROM jobs WHERE id = $1")
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
            Chicago,
            Duration::from_secs(900),
        )
        .with_host("host-a");
        filer.pass(1000).await.unwrap();
    }
}

/// 2025-03-01 at `hour`:`minute` UTC.
fn march(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2025, 3, 1, hour, minute, 0).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_is_counted_first_in_each_location(pool: PgPool) {
    let site = Site::new(pool).await;
    // The site's days are Chicago's: 2025-03-01 ends at 06:00 UTC on the 2nd.
    let last = Utc.with_ymd_and_hms(2025, 3, 2, 5, 59, 0).unwrap();
    let first_kept = Utc.with_ymd_and_hms(2025, 3, 2, 6, 0, 0).unwrap();
    site.call(
        site.primary,
        "2025-03-01/1-A/a.mp3",
        march(12, 0),
        &[1; 1000],
    )
    .await;
    site.call(site.primary, "2025-03-01/1-A/b.mp3", last, &[1; 3000])
        .await;
    site.call(site.primary, "2025-03-02/1-A/c.mp3", first_kept, &[1; 500])
        .await;
    site.call(
        site.cold_id,
        "2025-03-01/1-A/d.mp3",
        march(13, 0),
        &[1; 2000],
    )
    .await;
    site.call(site.old_id, "2025-03-01/1-A/e.mp3", march(14, 0), &[1; 700])
        .await;
    let mut conn = site.pool.acquire().await.unwrap();
    let preview = purges::preview(&mut conn, day("2025-03-02"), Chicago)
        .await
        .unwrap();
    assert_eq!(preview.calls, 4);
    assert_eq!(preview.refused, None);
    let held: Vec<(&str, bool, i64)> = preview
        .held
        .iter()
        .map(|h| (h.location.as_str(), h.read_only, h.calls))
        .collect();
    assert_eq!(
        held,
        [("primary", false, 2), ("Cold", false, 1), ("Old", true, 1)]
    );
    // The files that go: the read-only location's stay.
    assert_eq!(preview.bytes_about, Some(6000));
    let none = purges::preview(&mut conn, day("2025-02-01"), Chicago)
        .await
        .unwrap();
    assert_eq!(none.calls, 0);
    assert_eq!(
        none.refused.as_deref(),
        Some("no call is before 2025-02-01")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_starts_only_with_its_date_typed(pool: PgPool) {
    let site = Site::new(pool).await;
    site.call(site.primary, "2025-03-01/1-A/a.mp3", march(12, 0), b"a")
        .await;
    let february = Utc.with_ymd_and_hms(2025, 2, 15, 12, 0, 0).unwrap();
    site.call(site.primary, "2025-02-15/1-A/f.mp3", february, b"f")
        .await;
    match site.start("2025-03-02", "purge").await {
        Err(JobError::Refused(why)) => assert_eq!(why, "type 2025-03-02 to confirm"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    site.start("2025-03-02", "2025-03-02").await.unwrap();
    // A read-only location remembers what was purged from it, so that its
    // import skips those files; an earlier date never lowers it.
    site.start("2025-03-01", "2025-03-01").await.unwrap();
    let purged: Option<NaiveDate> =
        sqlx::query_scalar("SELECT purged_before FROM locations WHERE id = $1")
            .bind(site.old_id)
            .fetch_one(&site.pool)
            .await
            .unwrap();
    assert_eq!(purged, Some(day("2025-03-02")));
    let managed: Option<NaiveDate> =
        sqlx::query_scalar("SELECT purged_before FROM locations WHERE id = $1")
            .bind(site.cold_id)
            .fetch_one(&site.pool)
            .await
            .unwrap();
    assert_eq!(managed, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_deletes_the_records_first_and_the_sweep_their_files(pool: PgPool) {
    let site = Site::new(pool).await;
    let a = site
        .call(site.primary, "2025-03-01/1-A/a.mp3", march(12, 0), b"a")
        .await;
    let d = site
        .call(site.cold_id, "2025-03-01/1-A/d.mp3", march(13, 0), b"d")
        .await;
    let e = site
        .call(site.old_id, "2025-03-01/1-A/e.mp3", march(14, 0), b"e")
        .await;
    let kept = site
        .call(
            site.primary,
            "2025-03-03/1-A/k.mp3",
            Utc.with_ymd_and_hms(2025, 3, 3, 12, 0, 0).unwrap(),
            b"k",
        )
        .await;
    let job = site.start("2025-03-02", "2025-03-02").await.unwrap();
    site.run().await;
    let (state, message) = site.job(job).await;
    assert_eq!(state, "done");
    assert_eq!(
        message,
        "3 calls before 2025-03-02 purged: 2 files are deleted by the sweep; 1 in a read-only location lost only its record"
    );
    for id in [a, d, e] {
        assert!(!site.exists(id).await, "call {id}");
    }
    assert!(site.exists(kept).await);
    let transcripts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(
        transcripts, 1,
        "the purged calls' transcripts went with them"
    );
    // The records went first; the files wait for the sweep.
    assert!(site.archive.path().join("2025-03-01/1-A/a.mp3").exists());
    site.sweep().await;
    assert!(!site.archive.path().join("2025-03-01/1-A/a.mp3").exists());
    assert!(!site.cold.path().join("2025-03-01/1-A/d.mp3").exists());
    assert!(
        site.old.path().join("2025-03-01/1-A/e.mp3").exists(),
        "a read-only location's file stays"
    );
    assert!(site.archive.path().join("2025-03-03/1-A/k.mp3").exists());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM archive_leftovers")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_purge_started_again_takes_the_calls_left(pool: PgPool) {
    let site = Site::new(pool).await;
    let calls = purges::CALLS_PER_STEP + 5;
    for n in 0..calls {
        let at = march(0, 0) + chrono::TimeDelta::seconds(n);
        common::insert_call(
            &site.pool,
            (site.primary, site.source),
            &format!("2025-03-01/1-A/{n}.mp3"),
            at,
            2000,
            false,
        )
        .await;
    }
    let job = site.start("2025-03-02", "2025-03-02").await.unwrap();
    let context = Context::new(None);
    jobs::step(&site.pool, "host-a", &context).await.unwrap();
    let mut conn = site.pool.acquire().await.unwrap();
    jobs::stop(&mut conn, job, "test").await.unwrap();
    site.run().await;
    assert_eq!(site.job(job).await.0, "stopped");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(left, 5);
    let again = site.start("2025-03-02", "2025-03-02").await.unwrap();
    site.run().await;
    let (state, message) = site.job(again).await;
    assert_eq!(state, "done", "{message}");
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_day_in_the_future_is_refused(pool: PgPool) {
    let site = Site::new(pool).await;
    site.call(site.primary, "2025-03-01/1-A/a.mp3", march(12, 0), b"a")
        .await;
    // The site's today: a mistyped year must not purge every call.
    let today = Utc::now().with_timezone(&Chicago).date_naive();
    let tomorrow = today.succ_opt().unwrap();
    let why = format!("{tomorrow} is in the future: choose today or an earlier day");
    let mut conn = site.pool.acquire().await.unwrap();
    let future = purges::preview(&mut conn, tomorrow, Chicago).await.unwrap();
    assert_eq!(future.refused.as_deref(), Some(why.as_str()));
    assert_eq!(future.calls, 0, "a day in the future is not counted");
    let typed = tomorrow.to_string();
    match site.start(&typed, &typed).await {
        Err(JobError::Refused(refused)) => assert_eq!(refused, why),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let started: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'purge_before'")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(started, 0);
    let today = purges::preview(&mut conn, today, Chicago).await.unwrap();
    assert_eq!(today.refused, None);
    assert_eq!(today.calls, 1);
}
