mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use id3::TagLike;
use scannerpulse::archive;
use scannerpulse::filer::{self, Fence, Filer, Outcome};
use scannerpulse::reader;
use scannerpulse::store::{self, Location, NewCall, Seen, Source};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 100-1900000000";
const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;
/// A file of the recorder (invented values) and the folder it goes to.
const NAME: &str = "20310115_120000SYSB_South_Bravo__TO_215_FROM_4021.mp3";
const DIR: &str = "2031-01-15/215-Bravo EMS";

struct TestFence(AtomicBool);

impl Fence for TestFence {
    fn held(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    fn holder(&self) -> &str {
        HOLDER
    }
}

/// A database, an archive and a source, and the lease held by HOLDER.
struct Site {
    pool: PgPool,
    root: TempDir,
    source: TempDir,
    location: Location,
    source_row: Source,
    fence: Arc<TestFence>,
}

impl Site {
    async fn new(pool: PgPool) -> Site {
        let root = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let (location, source_id) = common::seed_places(&pool, root.path(), source.path()).await;
        common::hold_lease(&pool, HOLDER).await;
        Site {
            location: Location {
                id: location,
                root: root.path().to_path_buf(),
            },
            source_row: Source {
                id: source_id,
                name: "live".to_string(),
                path: source.path().to_path_buf(),
            },
            fence: Arc::new(TestFence(AtomicBool::new(true))),
            pool,
            root,
            source,
        }
    }

    fn filer(&self) -> Filer {
        Filer::new(
            self.pool.clone(),
            self.fence.clone(),
            self.location.clone(),
            vec![self.source_row.clone()],
            ZONE,
            Duration::from_secs(900),
        )
    }

    /// Puts a file into the source and records it as the watcher does.
    async fn arrive(&self, name: &str, data: &[u8]) -> PathBuf {
        let path = self.source.path().join(name);
        fs::write(&path, data).unwrap();
        self.seen(name).await;
        path
    }

    /// The watcher sees the file in the source again.
    async fn seen(&self, name: &str) {
        let meta = fs::metadata(self.source.path().join(name)).unwrap();
        let seen = Seen {
            rel_path: name.to_string(),
            size: meta.len() as i64,
            file_mtime: meta.modified().unwrap().into(),
        };
        store::upsert_seen(&self.pool, self.source_row.id, &[seen])
            .await
            .unwrap();
    }

    fn archived(&self, rel: &str) -> PathBuf {
        self.root.path().join(rel)
    }

    async fn calls(&self) -> Vec<(i64, String)> {
        sqlx::query_as("SELECT id, rel_path FROM calls ORDER BY id")
            .fetch_all(&self.pool)
            .await
            .unwrap()
    }

    async fn state(&self, name: &str) -> (String, Option<String>) {
        sqlx::query_as("SELECT state, reason FROM ingest_files WHERE rel_path = $1")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The files in the talkgroup's folder.
    fn folder(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.root.path().join(DIR))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

fn bravo() -> Vec<u8> {
    let mut tag = id3::Tag::new();
    tag.set_title("215\"Bravo EMS\"");
    tag.set_artist("4021-Medic 7");
    tag.set_text("TIT1", "SYSB");
    tag.add_frame(id3::frame::Comment {
        lang: "eng".to_string(),
        description: String::new(),
        text: "System:SYSB;Site:South;".to_string(),
    });
    common::tagged(&tag, 28)
}

fn rel() -> String {
    format!("{DIR}/{NAME}")
}

fn outcomes(done: Vec<(store::Waiting, Outcome)>) -> Vec<Outcome> {
    done.into_iter().map(|(_, outcome)| outcome).collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_is_filed_from_start_to_finish(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let data = bravo();
    let source = site.arrive(NAME, &data).await;
    let done = outcomes(site.filer().pass(10).await?);
    let [Outcome::Filed(id)] = done[..] else {
        panic!("{done:?}")
    };
    assert_eq!(fs::read(site.archived(&rel())).unwrap(), data);
    assert!(!source.exists(), "the file must leave its source");
    assert_eq!(site.calls().await, [(id, rel())]);
    assert_eq!(site.state(NAME).await.0, "done");
    // Nothing is left to do.
    assert!(site.filer().pass(10).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_name_taken_by_another_call_gets_a_number(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    fs::create_dir_all(site.archived(DIR)).unwrap();
    fs::write(site.archived(&rel()), b"another call").unwrap();
    site.arrive(NAME, &bravo()).await;
    let done = outcomes(site.filer().pass(10).await?);
    assert!(matches!(done[..], [Outcome::Filed(_)]), "{done:?}");
    let numbered = archive::candidate(NAME, 1);
    assert_eq!(site.folder(), [NAME.to_string(), numbered.clone()]);
    assert_eq!(fs::read(site.archived(&rel())).unwrap(), b"another call");
    assert_eq!(site.calls().await[0].1, format!("{DIR}/{numbered}"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_crash_after_the_copy_leaves_a_part_file_that_is_replaced(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let data = bravo();
    site.arrive(NAME, &data).await;
    archive::write_part(&site.archived(&rel()), &data, &archive::sha256(&data)).unwrap();
    let done = outcomes(site.filer().pass(10).await?);
    assert!(matches!(done[..], [Outcome::Filed(_)]), "{done:?}");
    assert_eq!(site.folder(), [NAME]);
    assert_eq!(fs::read(site.archived(&rel())).unwrap(), data);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_crash_after_the_rename_records_the_copy_without_copying_again(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let data = bravo();
    let source = site.arrive(NAME, &data).await;
    let target = site.archived(&rel());
    archive::write_part(&target, &data, &archive::sha256(&data)).unwrap();
    archive::finish_part(&target).unwrap();
    let done = outcomes(site.filer().pass(10).await?);
    let [Outcome::Filed(id)] = done[..] else {
        panic!("{done:?}")
    };
    assert_eq!(site.folder(), [NAME], "the file must not be filed twice");
    assert_eq!(site.calls().await, [(id, rel())]);
    assert!(!source.exists());
    Ok(())
}

/// Steps 1 to 3 done by hand, as a crash after step 3 leaves them: the file
/// is in the archive and recorded, and still in its source, where the watcher
/// has seen it again. Returns the source path and the call's id.
async fn filed_but_left_in_source(site: &Site) -> sqlx::Result<(PathBuf, i64)> {
    let data = bravo();
    let source = site.arrive(NAME, &data).await;
    let target = site.archived(&rel());
    archive::write_part(&target, &data, &archive::sha256(&data)).unwrap();
    archive::finish_part(&target).unwrap();
    let (call, _) = reader::read_path(&source, ZONE).unwrap();
    store::ensure_partition(&site.pool, call.started_at).await?;
    let row: i64 = sqlx::query_scalar("SELECT id FROM ingest_files")
        .fetch_one(&site.pool)
        .await?;
    let rel = rel();
    let new = NewCall {
        call: &call,
        source_id: site.source_row.id,
        location_id: site.location.id,
        rel_path: &rel,
    };
    let mut conn = site.pool.acquire().await?;
    let (id, _) = store::record(&mut conn, HOLDER, row, &new).await.unwrap();
    site.seen(NAME).await;
    Ok((source, id))
}

#[sqlx::test(migrations = "./migrations")]
async fn a_crash_after_recording_deletes_the_source_on_the_next_pass(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let (source, id) = filed_but_left_in_source(&site).await?;
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done, [Outcome::AlreadyFiled(id)]);
    assert!(!source.exists());
    assert_eq!(site.calls().await, [(id, rel())]);
    assert_eq!(site.state(NAME).await.0, "done");
    Ok(())
}

/// Says "held" this many times, then never again: the lease is lost part way.
struct LosingFence(Mutex<usize>);

impl Fence for LosingFence {
    fn held(&self) -> bool {
        let mut left = self.0.lock().unwrap();
        let held = *left > 0;
        *left = left.saturating_sub(1);
        held
    }
    fn holder(&self) -> &str {
        HOLDER
    }
}

// The repair checks the lease twice before it marks the file done; when the
// lease is lost after that, the source must stay where it is.
#[sqlx::test(migrations = "./migrations")]
async fn a_lease_lost_during_the_repair_keeps_the_source(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let (source, id) = filed_but_left_in_source(&site).await?;
    let mut filer = Filer::new(
        site.pool.clone(),
        Arc::new(LosingFence(Mutex::new(2))),
        site.location.clone(),
        vec![site.source_row.clone()],
        ZONE,
        Duration::from_secs(900),
    );
    let done = outcomes(filer.pass(10).await?);
    assert_eq!(done, [Outcome::AlreadyFiled(id)]);
    assert!(
        source.exists(),
        "without the lease the source must not be deleted"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unreadable_file_is_rejected_and_left_in_place(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let no_audio = site.arrive("20310115_120000_x.mp3", b"not audio").await;
    let no_date = site.arrive("recording.mp3", &bravo()).await;
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done.len(), 2);
    assert!(
        done.iter().all(|o| matches!(o, Outcome::Rejected(_))),
        "{done:?}"
    );
    assert!(no_audio.exists() && no_date.exists());
    let (state, reason) = site.state("20310115_120000_x.mp3").await;
    assert_eq!(state, "rejected");
    assert!(reason.unwrap().contains("frame"));
    assert!(
        site.state("recording.mp3")
            .await
            .1
            .unwrap()
            .contains("date")
    );
    assert!(site.calls().await.is_empty());
    Ok(())
}

/// A row for `NAME` with no file in the source, last seen `ago` ago.
async fn gone(site: &Site, size: usize, ago: &str) {
    let seen = Seen {
        rel_path: NAME.to_string(),
        size: size as i64,
        file_mtime: Utc::now(),
    };
    store::upsert_seen(&site.pool, site.source_row.id, &[seen])
        .await
        .unwrap();
    sqlx::query("UPDATE ingest_files SET seen_at = now() - $1::interval")
        .bind(ago)
        .execute(&site.pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_missing_file_waits_for_the_grace(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    gone(&site, 100, "1 minute").await;
    let mut filer = site.filer();
    let done = outcomes(filer.pass(10).await?);
    assert!(matches!(done[..], [Outcome::Later { .. }]), "{done:?}");
    assert_eq!(site.state(NAME).await.0, "waiting");
    // It is not tried again at once.
    assert!(filer.pass(10).await?.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_missing_file_found_in_the_archive_is_recorded_once(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let data = bravo();
    fs::create_dir_all(site.archived(DIR)).unwrap();
    fs::write(site.archived(&rel()), &data).unwrap();
    gone(&site, data.len(), "20 minutes").await;
    let done = outcomes(site.filer().pass(10).await?);
    let [Outcome::Filed(id)] = done[..] else {
        panic!("{done:?}")
    };
    assert_eq!(site.calls().await, [(id, rel())]);

    // Found again with its call: only marked done.
    gone(&site, data.len(), "20 minutes").await;
    sqlx::query("UPDATE ingest_files SET state = 'waiting'")
        .execute(&site.pool)
        .await?;
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done, [Outcome::AlreadyFiled(id)]);
    assert_eq!(site.calls().await.len(), 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_missing_file_not_in_the_archive_is_rejected(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    gone(&site, 100, "20 minutes").await;
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done, [Outcome::Rejected("missing".to_string())]);
    assert_eq!(
        site.state(NAME).await,
        ("rejected".to_string(), Some("missing".to_string()))
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn without_the_lease_nothing_is_written(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let source = site.arrive(NAME, &bravo()).await;
    site.fence.0.store(false, Ordering::SeqCst);
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done, [Outcome::LeaseLost]);
    assert!(source.exists());
    assert!(!site.archived(DIR).exists());
    assert_eq!(site.state(NAME).await.0, "waiting");
    Ok(())
}

// The local deadline still says "held", but the lease went to another host:
// the transaction's check stops the recording, and the next holder records
// the copy that was left in the archive.
#[sqlx::test(migrations = "./migrations")]
async fn a_lease_lost_before_recording_leaves_the_copy_for_the_next_holder(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let source = site.arrive(NAME, &bravo()).await;
    sqlx::query("UPDATE leases SET holder = 'host-b 7-7'")
        .execute(&site.pool)
        .await?;
    let done = outcomes(site.filer().pass(10).await?);
    assert_eq!(done, [Outcome::LeaseLost]);
    assert_eq!(site.folder(), [NAME]);
    assert!(site.calls().await.is_empty());
    assert!(source.exists());
    assert_eq!(site.state(NAME).await.0, "waiting");

    common::hold_lease(&site.pool, HOLDER).await;
    let done = outcomes(site.filer().pass(10).await?);
    assert!(matches!(done[..], [Outcome::Filed(_)]), "{done:?}");
    assert_eq!(site.folder(), [NAME]);
    assert!(!source.exists());
    Ok(())
}

#[test]
fn a_file_that_changed_after_it_was_read_is_not_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    fs::write(&path, b"audio").unwrap();
    let mtime: DateTime<Utc> = fs::metadata(&path).unwrap().modified().unwrap().into();
    assert!(!filer::remove_if_unchanged(&path, 4, mtime).unwrap());
    assert!(path.exists());
    assert!(filer::remove_if_unchanged(&path, 5, mtime).unwrap());
    assert!(!path.exists());
}

#[test]
fn copies_are_looked_for_on_the_day_and_the_day_on_each_side() {
    let root = tempfile::tempdir().unwrap();
    for (day, folder, size) in [
        ("2031-01-14", "1-A", 5),
        ("2031-01-16", "2-B", 5),
        ("2031-01-17", "3-C", 5),
        ("2031-01-15", "4-D", 6),
    ] {
        let dir = root.path().join(day).join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.mp3"), vec![0u8; size]).unwrap();
    }
    let day = NaiveDate::from_ymd_opt(2031, 1, 15).unwrap();
    let mut found = filer::find_copies(root.path(), day, "a.mp3", 5).unwrap();
    found.sort();
    assert_eq!(
        found,
        [
            root.path().join("2031-01-14/1-A/a.mp3"),
            root.path().join("2031-01-16/2-B/a.mp3"),
        ]
    );
}

// A rescan that lists the file before step 4 deletes it, and records it after,
// puts the done row back to waiting with its file gone. The row keeps its call.
#[sqlx::test(migrations = "./migrations")]
async fn a_filed_file_seen_again_after_its_deletion_stays_filed(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let data = bravo();
    let source = site.arrive(NAME, &data).await;
    let mut filer = site.filer();
    let done = outcomes(filer.pass(10).await?);
    let [Outcome::Filed(id)] = done[..] else {
        panic!("{done:?}")
    };
    assert_eq!(site.state(NAME).await.0, "done");

    fs::write(&source, &data).unwrap();
    site.seen(NAME).await;
    assert_eq!(site.state(NAME).await.0, "waiting");
    fs::remove_file(&source).unwrap();

    let done = outcomes(filer.pass(10).await?);
    assert_eq!(done, [Outcome::AlreadyFiled(id)]);
    assert_eq!(site.state(NAME).await, ("done".to_string(), None));
    assert_eq!(site.calls().await, [(id, rel())]);
    let rejected: i64 =
        sqlx::query_scalar("SELECT count(*) FROM ingest_files WHERE state = 'rejected'")
            .fetch_one(&site.pool)
            .await?;
    assert_eq!(rejected, 0);
    Ok(())
}
