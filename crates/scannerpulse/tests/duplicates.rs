//! The same call from two recorders: the longer recording is kept (spec
//! Part A, Section 5). All names and numbers are invented.

mod common;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use id3::TagLike;
use scannerpulse::archive;
use scannerpulse::filer::{Fence, Filer, Outcome};
use scannerpulse::reader;
use scannerpulse::store::{self, Location, NewCall, Seen};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 100-1900000000";
const OTHER_HOLDER: &str = "host-b 200-1900000000";
const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;
const DIR: &str = "2031-01-15/215-Bravo EMS";

/// A file of SYSB talkgroup 215 "Bravo EMS" from `radio`, `frames` frames
/// of 36 ms long, and its name for the start `stamp` (`HHMMSS`).
fn bravo(stamp: &str, frames: usize, radio: i64) -> (String, Vec<u8>) {
    let mut tag = id3::Tag::new();
    tag.set_title("215\"Bravo EMS\"");
    tag.set_artist(format!("{radio}-Medic 7"));
    tag.set_text("TIT1", "SYSB");
    tag.add_frame(id3::frame::Comment {
        lang: "eng".to_string(),
        description: String::new(),
        text: "System:SYSB;Site:South;".to_string(),
    });
    (
        format!("20310115_{stamp}SYSB_South_Bravo__TO_215_FROM_{radio}.mp3"),
        common::tagged(&tag, frames),
    )
}

/// The same file under another name, as a second recording of one minute.
fn renamed(file: (String, Vec<u8>), suffix: &str) -> (String, Vec<u8>) {
    (file.0.replace(".mp3", &format!("{suffix}.mp3")), file.1)
}

/// An archive and two recorders, `north` and `south`, and the lease.
struct Site {
    pool: PgPool,
    root: TempDir,
    north: TempDir,
    south: TempDir,
    location: Location,
    north_id: i16,
    south_id: i16,
}

type CallRow = (i64, DateTime<Utc>, i32, i16, String, Option<Vec<i16>>);

impl Site {
    async fn new(pool: PgPool) -> Site {
        let (root, north, south) = (
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
            tempfile::tempdir().unwrap(),
        );
        let (location, north_id) = common::seed_places(&pool, root.path(), north.path()).await;
        sqlx::query("UPDATE sources SET name = 'north' WHERE id = $1")
            .bind(north_id)
            .execute(&pool)
            .await
            .unwrap();
        let south_id: i16 = sqlx::query_scalar(
            "INSERT INTO sources (name, kind, path) VALUES ('south', 'sdrtrunk', $1) RETURNING id",
        )
        .bind(south.path().to_str().unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        common::hold_lease(&pool, HOLDER).await;
        Site {
            location: Location {
                id: location,
                root: root.path().to_path_buf(),
            },
            pool,
            root,
            north,
            south,
            north_id,
            south_id,
        }
    }

    fn filer_with(&self, fence: Arc<dyn Fence>) -> Filer {
        Filer::new(
            self.pool.clone(),
            fence,
            self.location.clone(),
            ZONE,
            Duration::from_secs(900),
        )
    }

    fn folder(&self, source_id: i16) -> &std::path::Path {
        if source_id == self.north_id {
            self.north.path()
        } else {
            self.south.path()
        }
    }

    /// A file arrives in a recorder's folder and the watcher sees it.
    async fn arrive(&self, source_id: i16, file: &(String, Vec<u8>)) -> PathBuf {
        let path = self.folder(source_id).join(&file.0);
        fs::write(&path, &file.1).unwrap();
        self.seen(source_id, &file.0).await;
        path
    }

    async fn seen(&self, source_id: i16, name: &str) {
        let meta = fs::metadata(self.folder(source_id).join(name)).unwrap();
        let seen = Seen {
            rel_path: name.to_string(),
            size: meta.len() as i64,
            file_mtime: meta.modified().unwrap().into(),
        };
        store::upsert_seen(&self.pool, source_id, &[seen])
            .await
            .unwrap();
    }

    async fn pass(&self) -> Vec<Outcome> {
        let mut filer = self.filer_with(common::HeldFence::new(HOLDER));
        outcomes(filer.pass(10).await.unwrap())
    }

    /// Each call: id, start, length, recorder, archive path, the other
    /// recorders it had a copy from.
    async fn calls(&self) -> Vec<CallRow> {
        sqlx::query_as(
            "SELECT id, started_at, duration_ms, source_id, rel_path, copies_from FROM calls ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap()
    }

    async fn row(&self, source_id: i16, name: &str) -> (String, Option<String>, Option<i64>) {
        sqlx::query_as(
            "SELECT state, reason, call_id FROM ingest_files WHERE source_id = $1 AND rel_path = $2",
        )
        .bind(source_id)
        .bind(name)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    /// The files in the talkgroup's folder of the day.
    fn archived(&self) -> Vec<String> {
        let mut names: Vec<String> = match fs::read_dir(self.root.path().join(DIR)) {
            Ok(entries) => entries
                .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        names
    }

    /// The call south's file at `path` is a copy of.
    async fn same_call(&self, path: &std::path::Path) -> (reader::FileCall, store::SameCall) {
        let (call, _) = reader::read_path(path, ZONE).unwrap();
        let talkgroup = store::talkgroup_of(&self.pool, "SYSB", 215)
            .await
            .unwrap()
            .unwrap();
        let same = store::same_call(&self.pool, talkgroup, &call, self.south_id)
            .await
            .unwrap()
            .unwrap();
        (call, same)
    }

    async fn south_row(&self) -> i64 {
        sqlx::query_scalar("SELECT id FROM ingest_files WHERE source_id = $1")
            .bind(self.south_id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

fn outcomes(done: Vec<(store::Waiting, Outcome)>) -> Vec<Outcome> {
    done.into_iter().map(|(_, outcome)| outcome).collect()
}

/// North's call of 12:00:00, 28 frames (1008 ms) long, filed.
async fn filed_by_north(site: &Site) -> i64 {
    site.arrive(site.north_id, &bravo("120000", 28, 4021)).await;
    let done = site.pass().await;
    let [Outcome::Filed(id, _)] = done[..] else {
        panic!("{done:?}")
    };
    id
}

#[sqlx::test(migrations = "./migrations")]
async fn a_shorter_copy_is_deleted_and_the_call_remembers_its_recorder(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    // South's clock runs a second ahead.
    let copy = bravo("120001", 20, 4021);
    let path = site.arrive(site.south_id, &copy).await;

    assert_eq!(site.pass().await, [Outcome::Duplicate(id)]);
    assert!(!path.exists(), "the copy stays in south's folder");
    assert_eq!(site.archived().len(), 1, "{:?}", site.archived());
    let calls = site.calls().await;
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].2, calls[0].3), (1008, site.north_id));
    assert_eq!(calls[0].5, Some(vec![site.south_id]));
    let (state, reason, call) = site.row(site.south_id, &copy.0).await;
    assert_eq!((state.as_str(), call), ("duplicate", Some(id)));
    assert_eq!(
        reason.as_deref(),
        Some(format!("a copy of call {id} from north: 0.7 s long, the call's 1.0 s").as_str())
    );

    // The watcher lists the deleted copy once more, unchanged: it stays a
    // duplicate.
    let mtime: DateTime<Utc> =
        sqlx::query_scalar("SELECT file_mtime FROM ingest_files WHERE rel_path = $1")
            .bind(&copy.0)
            .fetch_one(&site.pool)
            .await?;
    let seen = Seen {
        rel_path: copy.0.clone(),
        size: copy.1.len() as i64,
        file_mtime: mtime,
    };
    store::upsert_seen(&site.pool, site.south_id, &[seen])
        .await
        .unwrap();
    assert_eq!(site.row(site.south_id, &copy.0).await.0, "duplicate");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_copy_of_equal_length_leaves_the_copy_filed_first(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    site.arrive(site.south_id, &bravo("120000", 28, 4021)).await;
    assert_eq!(site.pass().await, [Outcome::Duplicate(id)]);
    assert_eq!(site.calls().await[0].3, site.north_id);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_longer_copy_replaces_the_recording_and_the_call_keeps_its_id_and_start(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    let before = site.calls().await;
    let old = before[0].4.clone();
    // The machine's transcript of the shorter recording.
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin, model, updated_at) \
         VALUES ($1, $2, 'engine 7 respond', 'machine', 'm', now())",
    )
    .bind(id)
    .bind(before[0].1)
    .execute(&site.pool)
    .await?;
    sqlx::query("UPDATE calls SET transcript_state = 'text' WHERE id = $1")
        .bind(id)
        .execute(&site.pool)
        .await?;
    // South's clock runs a second ahead.
    let copy = bravo("120001", 40, 4021);
    let path = site.arrive(site.south_id, &copy).await;

    assert_eq!(site.pass().await, [Outcome::Replaced(id)]);
    assert!(!path.exists(), "the copy stays in south's folder");
    let calls = site.calls().await;
    assert_eq!(calls.len(), 1);
    let (got, started_at, duration, source, rel, copies) = calls[0].clone();
    assert_eq!(
        (got, started_at),
        (id, before[0].1),
        "the call's identity changed"
    );
    assert_eq!((duration, source), (1440, site.south_id));
    assert_eq!(copies, Some(vec![site.north_id]));
    // The call's start, then the rest of the copy's own name; the old
    // recording held the first name until the transaction.
    assert_eq!(
        rel,
        format!("{DIR}/20310115_120000SYSB_South_Bravo__TO_215_FROM_4021_1.mp3")
    );
    assert_eq!(
        site.archived(),
        [rel.rsplit('/').next().unwrap().to_string()]
    );
    assert!(
        !site.root.path().join(&old).exists(),
        "the old recording stays"
    );
    assert_eq!(fs::read(site.root.path().join(&rel)).unwrap(), copy.1);
    // The archive still describes the call: its file gives the call's start.
    let (read, _) = reader::read_path(&site.root.path().join(&rel), ZONE).unwrap();
    assert_eq!(read.started_at, started_at);
    let leftovers: i64 = sqlx::query_scalar("SELECT count(*) FROM archive_leftovers")
        .fetch_one(&site.pool)
        .await?;
    assert_eq!(leftovers, 0);
    // Transcribed again; the old transcript stays until the new one comes.
    let queued: Option<String> =
        sqlx::query_scalar("SELECT state FROM transcription_queue WHERE call_id = $1")
            .bind(id)
            .fetch_optional(&site.pool)
            .await?;
    assert_eq!(queued.as_deref(), Some("queued"));
    let text: String = sqlx::query_scalar("SELECT text FROM transcripts WHERE call_id = $1")
        .bind(id)
        .fetch_one(&site.pool)
        .await?;
    assert_eq!(text, "engine 7 respond");
    assert_eq!(
        site.row(site.south_id, &copy.0).await,
        ("done".to_string(), None, Some(id))
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn two_transmissions_in_a_row_from_two_recorders_stay_two_calls(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    // One radio keys up twice: 12:00:00 for 3996 ms, then 12:00:04 for 2988 ms.
    site.arrive(site.north_id, &bravo("120000", 111, 4021))
        .await;
    site.arrive(site.north_id, &bravo("120004", 83, 4021)).await;
    let done = site.pass().await;
    let [Outcome::Filed(first, _), Outcome::Filed(second, _)] = done[..] else {
        panic!("{done:?}")
    };
    // South heard both: the first a frame longer, so that it overlaps the
    // second call by 32 ms; the second shorter.
    site.arrive(site.south_id, &bravo("120000", 112, 4021))
        .await;
    site.arrive(site.south_id, &bravo("120004", 80, 4021)).await;
    assert_eq!(
        site.pass().await,
        [Outcome::Replaced(first), Outcome::Duplicate(second)]
    );
    assert_eq!(site.calls().await.len(), 2);
    Ok(())
}

// The files one listing finds share their first sight. They are filed in the
// order of their names, which is time order, whatever the listing's order:
// a recorder's earlier copy then finds its call first.
#[sqlx::test(migrations = "./migrations")]
async fn a_recorders_files_found_together_are_filed_in_time_order(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    // One radio keys up twice: 12:00:00 for 2988 ms, then 12:00:03 for 2016 ms.
    site.arrive(site.north_id, &bravo("120000", 83, 4021)).await;
    let done = site.pass().await;
    let [Outcome::Filed(first, _)] = done[..] else {
        panic!("{done:?}")
    };
    // South heard both, the first a frame longer; one listing finds the two,
    // the later first. North's second is found after.
    let (a, b) = (bravo("120000", 84, 4021), bravo("120003", 55, 4021));
    let listed: Vec<Seen> = [&b, &a]
        .into_iter()
        .map(|file| {
            let path = site.south.path().join(&file.0);
            fs::write(&path, &file.1).unwrap();
            let meta = fs::metadata(&path).unwrap();
            Seen {
                rel_path: file.0.clone(),
                size: meta.len() as i64,
                file_mtime: meta.modified().unwrap().into(),
            }
        })
        .collect();
    store::upsert_seen(&site.pool, site.south_id, &listed)
        .await
        .unwrap();
    site.arrive(site.north_id, &bravo("120003", 56, 4021)).await;

    let done = site.pass().await;
    assert!(
        matches!(done[..], [Outcome::Replaced(id), Outcome::Filed(..), Outcome::Replaced(_)] if id == first),
        "{done:?}"
    );
    let starts: Vec<DateTime<Utc>> = site.calls().await.iter().map(|call| call.1).collect();
    assert_eq!(starts.len(), 2);
    assert_ne!(
        starts[0], starts[1],
        "both calls hold the first transmission"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorders_own_calls_are_never_its_copies(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    filed_by_north(&site).await;
    site.arrive(site.north_id, &renamed(bravo("120000", 30, 4021), "_b"))
        .await;
    let done = site.pass().await;
    assert!(matches!(done[..], [Outcome::Filed(..)]), "{done:?}");
    assert_eq!(site.calls().await.len(), 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_that_had_a_copy_of_a_call_sends_no_other(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    site.arrive(site.south_id, &bravo("120000", 20, 4021)).await;
    assert_eq!(site.pass().await, [Outcome::Duplicate(id)]);
    // Another file of south's overlapping the call is another call.
    site.arrive(site.south_id, &renamed(bravo("120000", 20, 4021), "_b"))
        .await;
    let done = site.pass().await;
    assert!(matches!(done[..], [Outcome::Filed(..)]), "{done:?}");
    assert_eq!(site.calls().await.len(), 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn another_radio_is_another_call(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    filed_by_north(&site).await;
    site.arrive(site.south_id, &bravo("120000", 20, 4099)).await;
    let done = site.pass().await;
    assert!(matches!(done[..], [Outcome::Filed(..)]), "{done:?}");
    assert_eq!(site.calls().await.len(), 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_copy_without_a_radio_matches_a_call_with_one(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    let mut tag = id3::Tag::new();
    tag.set_title("215\"Bravo EMS\"");
    tag.set_text("TIT1", "SYSB");
    let copy = (
        "20310115_120000SYSB_South_Bravo__TO_215.mp3".to_string(),
        common::tagged(&tag, 20),
    );
    site.arrive(site.south_id, &copy).await;
    assert_eq!(site.pass().await, [Outcome::Duplicate(id)]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn calls_that_do_not_overlap_within_a_second_are_other_calls(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    // North's call ends at 12:00:01.008; south's starts at 12:00:03.
    filed_by_north(&site).await;
    site.arrive(site.south_id, &bravo("120003", 20, 4021)).await;
    let done = site.pass().await;
    assert!(matches!(done[..], [Outcome::Filed(..)]), "{done:?}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_a_person_worked_on_keeps_its_recording(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    let started_at = site.calls().await[0].1;
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, machine_text, source) \
         VALUES ($1, $2, 'engine seven respond', 'engine 7 respond', 'admin')",
    )
    .bind(id)
    .bind(started_at)
    .execute(&site.pool)
    .await?;
    let copy = bravo("120000", 40, 4021);
    let path = site.arrive(site.south_id, &copy).await;
    assert_eq!(site.pass().await, [Outcome::Duplicate(id)]);
    assert!(!path.exists());
    let calls = site.calls().await;
    assert_eq!((calls[0].2, calls[0].3), (1008, site.north_id));
    assert_eq!(
        site.row(site.south_id, &copy.0).await.1,
        Some(format!(
            "a copy of call {id} from north, which keeps its recording: a person worked on it"
        ))
    );
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

// The replacement checks the lease at the start, before the copy is written,
// before it is named and before the transaction: lost there, the copy is in
// the archive and nothing else changed. The next holder, on another host,
// finds the copy and finishes.
#[sqlx::test(migrations = "./migrations")]
async fn a_lease_lost_after_the_copy_is_written_is_finished_by_the_next_holder(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    let old = site.calls().await[0].4.clone();
    let path = site.arrive(site.south_id, &bravo("120000", 40, 4021)).await;
    let mut losing = site.filer_with(Arc::new(LosingFence(Mutex::new(3))));
    assert_eq!(outcomes(losing.pass(10).await?), [Outcome::LeaseLost]);
    assert_eq!(site.archived().len(), 2, "{:?}", site.archived());
    assert_eq!(site.calls().await[0].4, old);
    assert!(path.exists());

    common::hold_lease(&site.pool, OTHER_HOLDER).await;
    let mut next = site.filer_with(common::HeldFence::new(OTHER_HOLDER));
    assert_eq!(outcomes(next.pass(10).await?), [Outcome::Replaced(id)]);
    assert_eq!(site.archived().len(), 1, "{:?}", site.archived());
    assert!(!site.root.path().join(&old).exists());
    assert!(!path.exists());
    Ok(())
}

// A stop after the transaction: the call names the copy, and the old
// recording and the file are still there. The next pass deletes both and
// files nothing again.
#[sqlx::test(migrations = "./migrations")]
async fn a_stop_after_the_transaction_is_finished_by_the_next_pass(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let id = filed_by_north(&site).await;
    let old = site.calls().await[0].4.clone();
    let copy = bravo("120000", 40, 4021);
    let path = site.arrive(site.south_id, &copy).await;
    let (call, same) = site.same_call(&path).await;
    let name = archive::copy_name(old.rsplit('/').next().unwrap(), &copy.0).unwrap();
    let rel = format!("{DIR}/{}", archive::candidate(&name, 1));
    let target = site.root.path().join(&rel);
    archive::write_part(&target, &copy.1, &archive::sha256(&copy.1)).unwrap();
    archive::finish_part(&target).unwrap();
    let new = NewCall {
        call: &call,
        source_id: site.south_id,
        location_id: site.location.id,
        rel_path: &rel,
        transcribe_from_ms: None,
    };
    let row = site.south_row().await;
    let mut conn = site.pool.acquire().await?;
    let replaced = store::replace(&mut conn, HOLDER, row, &same, &new, copy.1.len() as i64)
        .await
        .unwrap();
    assert!(replaced);
    drop(conn);
    site.seen(site.south_id, &copy.0).await;

    assert_eq!(site.pass().await, [Outcome::AlreadyFiled(id)]);
    assert!(
        !site.root.path().join(&old).exists(),
        "the old recording stays"
    );
    assert!(target.exists());
    assert!(!path.exists(), "the file stays in south's folder");
    assert_eq!(site.calls().await.len(), 1);
    Ok(())
}

// A call that changed between the match and the transaction (a correction
// moved its file) is left as it is, and so is the file's row.
#[sqlx::test(migrations = "./migrations")]
async fn a_call_that_changed_since_the_match_is_left_as_it_is(pool: PgPool) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    filed_by_north(&site).await;
    let copy = bravo("120000", 40, 4021);
    let path = site.arrive(site.south_id, &copy).await;
    let (call, same) = site.same_call(&path).await;
    sqlx::query("UPDATE calls SET rel_path = 'moved/elsewhere.mp3'")
        .execute(&site.pool)
        .await?;
    let new = NewCall {
        call: &call,
        source_id: site.south_id,
        location_id: site.location.id,
        rel_path: "2031-01-15/215-Bravo EMS/new.mp3",
        transcribe_from_ms: None,
    };
    let row = site.south_row().await;
    let mut conn = site.pool.acquire().await?;
    let replaced = store::replace(&mut conn, HOLDER, row, &same, &new, copy.1.len() as i64)
        .await
        .unwrap();
    assert!(!replaced);
    let calls = site.calls().await;
    assert_eq!(
        (calls[0].3, calls[0].4.as_str()),
        (site.north_id, "moved/elsewhere.mp3")
    );
    assert_eq!(site.row(site.south_id, &copy.0).await.0, "waiting");
    Ok(())
}

// North's file was filed, but left in its folder; south's longer copy then
// replaced the recording. North's file, seen again unchanged, is done with
// the call: no second call.
#[sqlx::test(migrations = "./migrations")]
async fn a_filed_file_seen_again_after_its_recording_was_replaced_is_not_filed_again(
    pool: PgPool,
) -> sqlx::Result<()> {
    let site = Site::new(pool).await;
    let first = bravo("120000", 28, 4021);
    let north = site.arrive(site.north_id, &first).await;
    let mtime = fs::metadata(&north).unwrap().modified().unwrap();
    let done = site.pass().await;
    let [Outcome::Filed(id, _)] = done[..] else {
        panic!("{done:?}")
    };
    site.arrive(site.south_id, &bravo("120000", 40, 4021)).await;
    assert_eq!(site.pass().await, [Outcome::Replaced(id)]);

    // Its deletion had failed: north's file is back, unchanged.
    fs::write(&north, &first.1).unwrap();
    fs::File::options()
        .write(true)
        .open(&north)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    site.seen(site.north_id, &first.0).await;
    assert_eq!(site.pass().await, [Outcome::AlreadyFiled(id)]);
    assert!(!north.exists());
    assert_eq!(site.calls().await.len(), 1);
    Ok(())
}
