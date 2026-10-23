mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use common::seed_places;
use id3::TagLike;
use scannerpulse::jobs::JobError;
use scannerpulse::recorders::{self, Refused};
use scannerpulse::store;
use scannerpulse::{consolidate, imports};
use sqlx::PgPool;
use tempfile::TempDir;

const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;

/// A recording of the system's talkgroup `number` "`name`", about `seconds`
/// long.
fn recording(system: &str, number: i64, name: &str, seconds: usize) -> Vec<u8> {
    let mut tag = id3::Tag::new();
    tag.set_title(format!("{number}\"{name}\""));
    tag.set_text("TIT1", system);
    common::tagged(&tag, seconds * 1000 / 36)
}

/// 2019-03-01 at `hour`:`minute` in the site's zone.
fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    ZONE.with_ymd_and_hms(2019, 3, 1, hour, minute, 0)
        .unwrap()
        .with_timezone(&Utc)
}

/// An install: its archive, the recorder live, the site's zone, the system
/// Metro with its talkgroups 702 "Fire" and 703 "Police", and one call of
/// 702 at 12:00, 10 s long. Returns the archive's folder and its places.
async fn site(pool: &PgPool) -> (TempDir, (i16, i16)) {
    let archive = tempfile::tempdir().unwrap();
    let places = seed_places(pool, archive.path(), Path::new("/srv/in/live")).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"America/Chicago\"')")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "WITH s AS (INSERT INTO systems (name) VALUES ('Metro') RETURNING id) \
         INSERT INTO talkgroups (system_id, number, name, transcribe) \
         SELECT id, n, t, true FROM s, (VALUES (702, 'Fire'), (703, 'Police')) AS v (n, t)",
    )
    .execute(pool)
    .await
    .unwrap();
    store::ensure_partition(pool, at(12, 0)).await.unwrap();
    sqlx::query(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
         SELECT $1, 10000, t.id, $2, $3, '2019-03-01/702-Fire/20190301_120000.mp3' \
         FROM talkgroups t WHERE t.number = 702",
    )
    .bind(at(12, 0))
    .bind(places.1)
    .bind(places.0)
    .execute(pool)
    .await
    .unwrap();
    (archive, places)
}

/// An older install's folder:
/// - a copy of the call of 12:00;
/// - a new call of 702 at 13:00;
/// - a new call of 703 at 14:00, whose file names it otherwise;
/// - a file with no date in its name;
/// - a call of the system County, which this install does not know;
/// - a file with no audio in it.
fn older_archive() -> TempDir {
    let old = tempfile::tempdir().unwrap();
    let day = old.path().join("2019-03-01");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::create_dir_all(old.path().join("notes")).unwrap();
    for (path, data) in [
        (
            day.join("20190301_120000_a.mp3"),
            recording("Metro", 702, "Fire", 10),
        ),
        (
            day.join("20190301_130000_b.mp3"),
            recording("Metro", 702, "Fire", 4),
        ),
        (
            old.path().join("20190301_140000_c.mp3"),
            recording("Metro", 703, "Police Main", 4),
        ),
        (
            old.path().join("notes/readme.mp3"),
            recording("Metro", 702, "Fire", 2),
        ),
        (
            day.join("20190301_150000_e.mp3"),
            recording("County", 9, "Roads", 4),
        ),
        (
            day.join("20190301_160000_f.mp3"),
            b"not audio at all".to_vec(),
        ),
    ] {
        std::fs::write(path, data).unwrap();
    }
    old
}

/// Every file and folder under `root`, with its size and modification time:
/// what an import must leave as it was.
fn snapshot(root: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let meta = std::fs::metadata(&folder).unwrap();
        found.push((folder.clone(), 0, meta.modified().unwrap()));
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::metadata(&path).unwrap();
            if meta.is_dir() {
                folders.push(path);
            } else {
                found.push((path, meta.len(), meta.modified().unwrap()));
            }
        }
    }
    found.sort();
    found
}

async fn run_jobs(pool: &PgPool) {
    let context = scannerpulse::jobs::Context::new(None);
    while scannerpulse::jobs::step(pool, "host-a", &context)
        .await
        .unwrap()
        .is_some()
    {}
}

async fn count(pool: &PgPool, query: &str) -> i64 {
    sqlx::query_scalar(query).fetch_one(pool).await.unwrap()
}

fn refusal<T: std::fmt::Debug>(result: Result<T, JobError>) -> String {
    match result {
        Err(JobError::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// An import made by hand: a read-only location, a source of the kind
/// import, and the row that joins them. Returns the location and the source.
async fn import_by_hand(pool: &PgPool, name: &str, root: &Path) -> (i16, i16) {
    let location: i16 = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode) VALUES ($1, $2, 'readonly') RETURNING id",
    )
    .bind(name)
    .bind(root.display().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    let source: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path, enabled) VALUES ($1, 'import', $2, false) RETURNING id",
    )
    .bind(name)
    .bind(root.display().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO imports (source_id, location_id) VALUES ($1, $2)")
        .bind(source)
        .bind(location)
        .execute(pool)
        .await
        .unwrap();
    (location, source)
}

#[sqlx::test(migrations = "./migrations")]
async fn an_imports_files_are_never_a_recorders(pool: PgPool) {
    seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let (_, source) = import_by_hand(&pool, "Old", Path::new("/srv/old")).await;
    sqlx::query("INSERT INTO systems (name) VALUES ('Metro')")
        .execute(&pool)
        .await
        .unwrap();
    let month_ago = Utc::now() - TimeDelta::days(30);
    for (rel, state, system) in [
        ("a.mp3", "waiting", None),
        ("b.mp3", "staged", Some("Metro")),
        ("c.mp3", "done", None),
        ("d.mp3", "rejected", None),
    ] {
        sqlx::query(
            "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, staged_system) \
             VALUES ($1, $2, $3, $3, $4, $5)",
        )
        .bind(source)
        .bind(rel)
        .bind(month_ago)
        .bind(state)
        .bind(system)
        .execute(&pool)
        .await
        .unwrap();
    }
    // The filer never takes them: the import reads its own files, and a
    // staged one is in the import's folder, not in the staging folder.
    assert!(store::waiting(&pool, 100, &[]).await.unwrap().is_empty());
    // They are the import's register, kept however old.
    assert_eq!(store::prune_done(&pool, 7).await.unwrap(), 0);
    // It is no recorder: not listed, not found by name, and its files are
    // not counted on Status.
    let mut conn = pool.acquire().await.unwrap();
    let listed: Vec<String> = recorders::list(&mut conn, month_ago)
        .await
        .unwrap()
        .into_iter()
        .map(|recorder| recorder.name)
        .collect();
    assert_eq!(listed, ["live"]);
    assert!(matches!(
        recorders::find(&mut conn, "Old").await,
        Err(Refused::NoSuch(_))
    ));
    match recorders::add(&mut conn, "Old", Path::new("/srv/in/old"), false, |f| {
        Ok(f.to_path_buf())
    })
    .await
    {
        Err(Refused::Refused(why)) => assert!(why.contains("imported as Old"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let status = scannerpulse::status::read_within(&pool, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(status["ingest"]["waiting"], 0);
    assert_eq!(status["ingest"]["rejected"], 0);
    assert_eq!(status["ingest"]["recent_rejected"], serde_json::json!([]));
    assert_eq!(status["failover"]["files_left_in_sources"], 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_review_counts_an_older_archive_and_writes_nothing(pool: PgPool) {
    site(&pool).await;
    let old = older_archive();
    let before = snapshot(old.path());
    let mut conn = pool.acquire().await.unwrap();
    let job = imports::start_review(&mut conn, "Old", old.path(), "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let review = imports::review(&mut conn, job).await.unwrap().unwrap();
    assert!(review.done);
    assert_eq!((review.files, review.no_date, review.read), (6, 1, 5));
    assert_eq!(
        (
            review.new,
            review.copies,
            review.unknown,
            review.differ,
            review.unreadable
        ),
        (2, 1, 1, 1, 1)
    );
    assert_eq!(review.examples.no_date, ["notes/readme.mp3"]);
    assert!(
        review.examples.copies[0].starts_with("2019-03-01/20190301_120000_a.mp3: a copy of call "),
        "{:?}",
        review.examples.copies
    );
    assert_eq!(
        review.examples.unknown,
        ["2019-03-01/20190301_150000_e.mp3: the system County"]
    );
    assert_eq!(
        review.examples.differ,
        ["20190301_140000_c.mp3: talkgroup 703 is Police here, Police Main in the file"]
    );
    assert_eq!(review.suggested, None);
    // It writes nothing, in the folder or in the database.
    assert_eq!(snapshot(old.path()), before);
    assert_eq!(count(&pool, "SELECT count(*) FROM locations").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM sources").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM calls").await, 1);
    assert_eq!(count(&pool, "SELECT count(*) FROM ingest_files").await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_review_suggests_the_offset_at_which_the_files_are_copies(pool: PgPool) {
    let (_archive, places) = site(&pool).await;
    // The archive's calls of 13:05 to 13:55; the older install's clock named
    // them an hour early.
    let old = tempfile::tempdir().unwrap();
    for k in 0..6 {
        let minute = 5 + 10 * k;
        sqlx::query(
            "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
             SELECT $1, 4000, t.id, $2, $3, $4 FROM talkgroups t WHERE t.number = 702",
        )
        .bind(at(13, minute))
        .bind(places.1)
        .bind(places.0)
        .bind(format!("x{k}.mp3"))
        .execute(&pool)
        .await
        .unwrap();
        std::fs::write(
            old.path().join(format!("20190301_12{minute:02}00_x.mp3")),
            recording("Metro", 702, "Fire", 4),
        )
        .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    let job = imports::start_review(&mut conn, "Old", old.path(), "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let review = imports::review(&mut conn, job).await.unwrap().unwrap();
    assert!(review.done);
    assert_eq!(review.copies_at, [0, 0, 0, 6, 0]);
    // Imported at +01:00, none of them would be a new call.
    assert_eq!(review.new_at, [6, 6, 6, 0, 6]);
    assert_eq!((review.copies, review.new), (0, 6));
    assert_eq!(review.suggested, Some(60));
    assert_eq!(imports::offset_text(60), "+01:00");
    assert_eq!(imports::parse_offset("+01:00"), Ok(60));
    assert_eq!(imports::parse_offset("-0:30"), Ok(-30));
    assert_eq!(imports::parse_offset("0"), Ok(0));
    assert!(imports::parse_offset("+25:00").is_err());
    assert!(imports::parse_offset("1h").is_err());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_import_files_its_new_calls_where_they_are(pool: PgPool) {
    let (archive, places) = site(&pool).await;
    common::set_transcription(&pool, "http://192.0.2.1:9000", 0, 60).await;
    let old = older_archive();
    let before = snapshot(old.path());
    let mut conn = pool.acquire().await.unwrap();
    let (job, started) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    assert!(!started.again);
    run_jobs(&pool).await;
    let (state, message): (String, String) =
        sqlx::query_as("SELECT state, message FROM jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "done", "{message}");
    assert_eq!(
        message,
        "2 calls imported into Old; 1 copy of a call already in the archive skipped; \
         1 file waits until its system is named and the import runs again; 2 files skipped: they cannot be read as calls"
    );
    // Its new calls are in Old, at their paths in its folder, from the
    // import, and none is queued for transcription.
    let calls: Vec<(String, DateTime<Utc>, i64, String)> = sqlx::query_as(
        "SELECT c.rel_path, c.started_at, t.number, s.name FROM calls c \
         JOIN talkgroups t ON t.id = c.talkgroup_id JOIN sources s ON s.id = c.source_id \
         WHERE c.location_id = $1 ORDER BY c.started_at",
    )
    .bind(started.location)
    .fetch_all(&pool)
    .await
    .unwrap();
    let old_name = || "Old".to_string();
    assert_eq!(
        calls,
        [
            (
                "2019-03-01/20190301_130000_b.mp3".into(),
                at(13, 0),
                702,
                old_name()
            ),
            ("20190301_140000_c.mp3".into(), at(14, 0), 703, old_name()),
        ]
    );
    assert_eq!(
        count(&pool, "SELECT count(*) FROM transcription_queue").await,
        0
    );
    // 703 keeps its name: the file's is a proposal, from the import.
    let proposals: Vec<(String, Vec<i16>)> =
        sqlx::query_as("SELECT name, source_ids FROM name_proposals")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        proposals,
        [("Police Main".to_string(), vec![started.source])]
    );
    // The copy is skipped: the archive's call keeps its recording, and
    // knows the import had one.
    let copies: Option<Vec<i16>> =
        sqlx::query_scalar("SELECT copies_from FROM calls WHERE location_id = $1")
            .bind(places.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(copies, Some(vec![started.source]));
    // The file of County waits in the import's folder: nothing is copied
    // into the staging folder.
    let staged: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT rel_path, staged_system FROM ingest_files WHERE state = 'staged'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        staged,
        [(
            "2019-03-01/20190301_150000_e.mp3".to_string(),
            Some("County".to_string())
        )]
    );
    assert!(!archive.path().join(".scannerpulse-staging").exists());
    // Nothing in its folder changed, and it is read-only.
    assert_eq!(snapshot(old.path()), before);
    let mode: String = sqlx::query_scalar("SELECT mode FROM locations WHERE id = $1")
        .bind(started.location)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(mode, "readonly");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_import_run_again_takes_only_the_files_it_has_not(pool: PgPool) {
    site(&pool).await;
    let old = older_archive();
    let mut conn = pool.acquire().await.unwrap();
    imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    // A file the older install wrote since, and the system County named.
    std::fs::write(
        old.path().join("20190301_170000_g.mp3"),
        recording("Metro", 702, "Fire", 4),
    )
    .unwrap();
    scannerpulse::names::new_system(&mut conn, "County")
        .await
        .unwrap();
    let (job, started) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    assert!(started.again);
    run_jobs(&pool).await;
    let message: String = sqlx::query_scalar("SELECT message FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(message, "2 calls imported into Old");
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT rel_path FROM calls WHERE location_id = $1 ORDER BY started_at")
            .bind(started.location)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        paths,
        [
            "2019-03-01/20190301_130000_b.mp3",
            "20190301_140000_c.mp3",
            "2019-03-01/20190301_150000_e.mp3",
            "20190301_170000_g.mp3",
        ]
    );
    // It keeps its folder and its offset.
    let why = refusal(imports::start(&mut conn, "Old", old.path(), Some(60), "admin").await);
    assert!(
        why.contains("Old was imported with the offset +00:00"),
        "{why}"
    );
    let other = tempfile::tempdir().unwrap();
    let why = refusal(imports::start(&mut conn, "Old", other.path(), None, "admin").await);
    assert!(why.contains("Old is the import of"), "{why}");
    // A name another location or a recorder has is refused.
    let why = refusal(imports::start(&mut conn, "live", other.path(), None, "admin").await);
    assert!(why.contains("live"), "{why}");
}

/// Several recorders filed into one older folder (6B3's M5; the owner: "Keep
/// first arrived"): of an import's copies of a call, the one whose file
/// arrived first, by its time, is the call, in whichever order the import
/// takes them; the others are copies, as is every copy of a call the archive
/// already holds.
#[sqlx::test(migrations = "./migrations")]
async fn an_import_keeps_the_copy_that_arrived_first(pool: PgPool) {
    let (_archive, places) = site(&pool).await;
    let old = tempfile::tempdir().unwrap();
    let start = SystemTime::now() - Duration::from_secs(86_400);
    let put = |name: &str, seconds: usize, after: u64| {
        let path = old.path().join(name);
        std::fs::write(&path, recording("Metro", 702, "Fire", seconds)).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(start + Duration::from_secs(after))
            .unwrap();
    };
    // First, a copy of the archive's call of 12:00, and the copy of 13:00
    // that arrived last.
    put("20190301_120000_a.mp3", 10, 0);
    put("20190301_130001_b.mp3", 6, 3605);
    let mut conn = pool.acquire().await.unwrap();
    let (_, started) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    // Then the copy of 13:00 that arrived first, another copy of 12:00, and
    // both copies of 14:00.
    put("20190301_120001_b.mp3", 10, 1);
    put("20190301_130000_a.mp3", 4, 3604);
    put("20190301_140000_a.mp3", 4, 7204);
    put("20190301_140001_b.mp3", 6, 7203);
    let (job, _) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let message: String = sqlx::query_scalar("SELECT message FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        message,
        "1 call imported into Old; 3 copies of calls already in the archive skipped"
    );
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT rel_path FROM calls WHERE location_id = $1 ORDER BY started_at")
            .bind(started.location)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(paths, ["20190301_130000_a.mp3", "20190301_140001_b.mp3"]);
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT rel_path, state FROM ingest_files WHERE source_id = $1 ORDER BY rel_path",
    )
    .bind(started.source)
    .fetch_all(&pool)
    .await
    .unwrap();
    let row = |path: &str, state: &str| (path.to_string(), state.to_string());
    assert_eq!(
        rows,
        [
            row("20190301_120000_a.mp3", "duplicate"),
            row("20190301_120001_b.mp3", "duplicate"),
            row("20190301_130000_a.mp3", "done"),
            row("20190301_130001_b.mp3", "duplicate"),
            row("20190301_140000_a.mp3", "duplicate"),
            row("20190301_140001_b.mp3", "done"),
        ]
    );
    let copies: Option<Vec<i16>> =
        sqlx::query_scalar("SELECT copies_from FROM calls WHERE location_id = $1")
            .bind(places.0)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(copies, Some(vec![started.source]));
}

#[sqlx::test(migrations = "./migrations")]
async fn an_imports_offset_and_its_purge_date_its_files(pool: PgPool) {
    site(&pool).await;
    let old = tempfile::tempdir().unwrap();
    let write = |name: &str| {
        std::fs::write(old.path().join(name), recording("Metro", 702, "Fire", 4)).unwrap()
    };
    write("20190301_130000_b.mp3");
    write("20190302_090000_h.mp3");
    let mut conn = pool.acquire().await.unwrap();
    let (_, started) = imports::start(&mut conn, "Old", old.path(), Some(60), "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let times = || async {
        sqlx::query_scalar::<_, DateTime<Utc>>(
            "SELECT started_at FROM calls WHERE location_id = $1 ORDER BY started_at",
        )
        .bind(started.location)
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    // The older install's clock was an hour behind.
    assert_eq!(times().await, [at(14, 0), at(13, 0) + TimeDelta::hours(21)]);
    // A purge before 2019-03-02 takes the call of March 1; its file stays.
    let day = chrono::NaiveDate::from_ymd_opt(2019, 3, 2).unwrap();
    scannerpulse::purges::start(&mut conn, day, ZONE, "2019-03-02", "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    assert_eq!(times().await, [at(13, 0) + TimeDelta::hours(21)]);
    assert!(old.path().join("20190301_130000_b.mp3").exists());
    // Run again, it takes neither that file nor a new one from before the
    // day, and takes one from after it.
    write("20190301_200000_i.mp3");
    write("20190303_090000_j.mp3");
    let (job, _) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    assert_eq!(
        times().await,
        [
            at(13, 0) + TimeDelta::hours(21),
            at(13, 0) + TimeDelta::hours(45)
        ]
    );
    let message: String = sqlx::query_scalar("SELECT message FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        message,
        "1 call imported into Old; 1 file skipped: it is from before 2019-03-02, before which Old was purged"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_consolidation_copies_an_imports_calls_into_the_archive(pool: PgPool) {
    let (archive, places) = site(&pool).await;
    let old = older_archive();
    let mut conn = pool.acquire().await.unwrap();
    let (_, started) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let before = snapshot(old.path());
    // Counted first, and started only with the word typed.
    let preview = consolidate::preview(&mut conn, "Old").await.unwrap();
    assert_eq!((preview.calls, preview.refused.as_deref()), (2, None));
    let why = refusal(consolidate::start(&mut conn, "Old", "yes", "admin").await);
    assert_eq!(why, "type move to confirm");
    let why = refusal(consolidate::start(&mut conn, "New", "move", "admin").await);
    assert_eq!(why, "there is no import named New");
    consolidate::start(&mut conn, "Old", "move", "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    // Its calls are in the main archive's layout now: the day, the folder of
    // the talkgroup their files name, their names.
    let paths: Vec<String> =
        sqlx::query_scalar("SELECT rel_path FROM calls WHERE location_id = $1 ORDER BY started_at")
            .bind(places.0)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        paths,
        [
            "2019-03-01/702-Fire/20190301_120000.mp3",
            "2019-03-01/702-Fire/20190301_130000_b.mp3",
            "2019-03-01/703-Police Main/20190301_140000_c.mp3",
        ]
    );
    for (copy, original) in [
        (&paths[1], "2019-03-01/20190301_130000_b.mp3"),
        (&paths[2], "20190301_140000_c.mp3"),
    ] {
        assert_eq!(
            std::fs::read(archive.path().join(copy)).unwrap(),
            std::fs::read(old.path().join(original)).unwrap()
        );
    }
    // The import's files stay as they were, and none is left for the sweep.
    assert_eq!(snapshot(old.path()), before);
    assert_eq!(
        count(&pool, "SELECT count(*) FROM archive_leftovers").await,
        0
    );
    // It holds no call now: it can be removed, and its register stays.
    let preview = consolidate::preview(&mut conn, "Old").await.unwrap();
    assert_eq!(preview.refused.as_deref(), Some("Old holds no call"));
    assert_eq!(
        scannerpulse::storage::remove(&mut conn, started.location)
            .await
            .unwrap(),
        "Old"
    );
    let linked: Option<i16> =
        sqlx::query_scalar("SELECT location_id FROM imports WHERE source_id = $1")
            .bind(started.source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(linked, None);
    // Imported again, it takes nothing it took before.
    let (job, again) = imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    assert!(again.again);
    run_jobs(&pool).await;
    let message: String = sqlx::query_scalar("SELECT message FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(message, "0 calls imported into Old");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_imports_files_are_listed_where_no_filer_takes_them(pool: PgPool) {
    site(&pool).await;
    let old = older_archive();
    let mut conn = pool.acquire().await.unwrap();
    imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    // The first step lists the folder.
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    // The filer of every version takes the waiting files, and deletes each
    // from its folder once filed: an import's files are listed instead.
    let states: Vec<(String, i64)> =
        sqlx::query_as("SELECT state, count(*) FROM ingest_files GROUP BY state")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, [("listed".to_string(), 6)]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_talkgroup_with_imported_calls_is_corrected_all_the_same(pool: PgPool) {
    site(&pool).await;
    let old = tempfile::tempdir().unwrap();
    std::fs::write(
        old.path().join("20190301_130000_b.mp3"),
        recording("Metro", 703, "Police Main", 4),
    )
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let correct = || async {
        sqlx::query_scalar::<_, bool>("SELECT files_correct FROM talkgroups WHERE number = 703")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    // The file names 703 otherwise: its files are not all under its name.
    assert!(!correct().await);
    let id: i32 = sqlx::query_scalar("SELECT id FROM talkgroups WHERE number = 703")
        .fetch_one(&pool)
        .await
        .unwrap();
    scannerpulse::data::start_correct(&mut conn, id, "Police", "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    // A correction never writes in the import's folder, so its file keeps
    // its name there; the talkgroup is corrected all the same.
    assert!(correct().await);
    assert!(old.path().join("20190301_130000_b.mp3").exists());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_review_counts_the_files_whose_system_was_named_since(pool: PgPool) {
    site(&pool).await;
    let old = older_archive();
    let mut conn = pool.acquire().await.unwrap();
    imports::start(&mut conn, "Old", old.path(), None, "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    // County's file waits; once County is named, the next run takes it, and
    // the review counts it among the files left, not the files taken.
    scannerpulse::names::new_system(&mut conn, "County")
        .await
        .unwrap();
    let job = imports::start_review(&mut conn, "Old", old.path(), "admin")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let review = imports::review(&mut conn, job).await.unwrap().unwrap();
    assert_eq!(
        (review.taken, review.left, review.read, review.new),
        (4, 1, 1, 1)
    );
}
