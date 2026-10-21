mod common;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use id3::TagLike;
use scannerpulse::archive;
use scannerpulse::filer::{Filer, Outcome};
use scannerpulse::names;
use scannerpulse::recorders::Refused;
use scannerpulse::store::{self, Location, Seen};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 100-1900000000";
const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;
const NAME: &str = "20310115_120000SYSB_South_Bravo__TO_215_FROM_4021.mp3";

/// A file of system SYSB, talkgroup 215 "Bravo EMS" (invented values).
fn bravo() -> Vec<u8> {
    let mut tag = id3::Tag::new();
    tag.set_title("215\"Bravo EMS\"");
    tag.set_artist("4021-Medic 7");
    tag.set_text("TIT1", "SYSB");
    common::tagged(&tag, 28)
}

struct Site {
    pool: PgPool,
    root: TempDir,
    source: TempDir,
    location: Location,
    source_id: i16,
}

impl Site {
    /// An archive, a recorder `live`, the lease, and the system SYSA unless
    /// the database is to stay without one.
    async fn new(pool: PgPool, with_sysa: bool) -> Site {
        let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (location, source_id) = common::seed_places(&pool, root.path(), source.path()).await;
        common::hold_lease(&pool, HOLDER).await;
        if with_sysa {
            sqlx::query("INSERT INTO systems (name) VALUES ('SYSA')")
                .execute(&pool)
                .await
                .unwrap();
        }
        Site {
            location: Location {
                id: location,
                root: root.path().to_path_buf(),
            },
            pool,
            root,
            source,
            source_id,
        }
    }

    fn filer(&self) -> Filer {
        Filer::new(
            self.pool.clone(),
            common::HeldFence::new(HOLDER),
            self.location.clone(),
            ZONE,
            Duration::from_secs(900),
        )
    }

    /// The file arrives in the recorder's folder and the watcher sees it.
    async fn arrive(&self) -> PathBuf {
        let path = self.source.path().join(NAME);
        fs::write(&path, bravo()).unwrap();
        self.seen().await;
        path
    }

    async fn seen(&self) {
        let meta = fs::metadata(self.source.path().join(NAME)).unwrap();
        let seen = Seen {
            rel_path: NAME.to_string(),
            size: meta.len() as i64,
            file_mtime: meta.modified().unwrap().into(),
        };
        store::upsert_seen(&self.pool, self.source_id, &[seen])
            .await
            .unwrap();
    }

    fn staged(&self) -> PathBuf {
        archive::staging_path(self.root.path(), self.source_id, NAME)
    }

    async fn pass(&self) -> Vec<Outcome> {
        self.filer()
            .pass(10)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, outcome)| outcome)
            .collect()
    }

    async fn row(&self) -> (String, Option<String>, Option<String>) {
        sqlx::query_as("SELECT state, staged_system, staged_talkgroup FROM ingest_files")
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }

    /// The system of the one call filed, and whether its talkgroup's files
    /// are correct.
    async fn filed_under(&self) -> (String, bool) {
        sqlx::query_as(
            "SELECT y.name, t.files_correct FROM calls c JOIN talkgroups t ON t.id = c.talkgroup_id \
             JOIN systems y ON y.id = t.system_id",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_of_an_unknown_system_leaves_its_recorder_and_waits_in_the_staging_folder(
    pool: PgPool,
) {
    let site = Site::new(pool, true).await;
    let path = site.arrive().await;
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    assert_eq!(site.pass().await, [Outcome::Staged("SYSB".into())]);
    assert!(!path.exists(), "nothing stays in a recorder's folder");
    assert_eq!(fs::read(site.staged()).unwrap(), bravo());
    assert_eq!(
        fs::metadata(site.staged()).unwrap().modified().unwrap(),
        mtime,
        "the staged copy keeps the file's time"
    );
    assert_eq!(
        site.row().await,
        (
            "staged".into(),
            Some("SYSB".into()),
            Some("215 Bravo EMS".into())
        )
    );
    let mut conn = site.pool.acquire().await.unwrap();
    let unknown = names::unknown_systems(&mut conn).await.unwrap();
    assert_eq!(unknown.len(), 1);
    assert_eq!(
        (
            unknown[0].system.as_str(),
            unknown[0].files,
            unknown[0].recorders.clone(),
            unknown[0].talkgroups.clone()
        ),
        (
            "SYSB",
            1,
            vec!["live".to_string()],
            vec!["215 Bravo EMS".to_string()]
        )
    );
    assert!(site.pass().await.is_empty(), "it waits for a name");
    // Seen again (left behind), it stays staged.
    fs::write(&path, bravo()).unwrap();
    site.seen().await;
    assert_eq!(site.row().await.0, "staged");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_system_named_for_it_files_its_staged_files(pool: PgPool) {
    let site = Site::new(pool, true).await;
    site.arrive().await;
    site.pass().await;
    let mut conn = site.pool.acquire().await.unwrap();
    names::new_system(&mut conn, "SYSB").await.unwrap();
    assert!(matches!(site.pass().await[..], [Outcome::Filed(..)]));
    assert!(!site.staged().exists());
    assert_eq!(site.row().await.0, "done");
    assert_eq!(site.filed_under().await, ("SYSB".to_string(), true));
    assert!(names::unknown_systems(&mut conn).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_name_mapped_to_a_system_files_under_it_and_waits_for_the_correction(pool: PgPool) {
    let site = Site::new(pool, true).await;
    site.arrive().await;
    site.pass().await;
    let mut conn = site.pool.acquire().await.unwrap();
    let refused = names::map_system(&mut conn, "SYSB", "SYSZ").await;
    assert!(matches!(refused, Err(Refused::Refused(_))), "{refused:?}");
    names::map_system(&mut conn, "SYSB", "SYSA").await.unwrap();
    assert!(matches!(site.pass().await[..], [Outcome::Filed(..)]));
    assert_eq!(site.filed_under().await, ("SYSA".to_string(), false));
    let mapped: i64 = sqlx::query_scalar("SELECT count(*) FROM mapped_calls")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(mapped, 1);
    assert!(
        store::system_known(&site.pool, "SYSB").await.unwrap(),
        "the spelling is known for good"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn discarding_a_name_deletes_its_staged_files_once_it_is_typed(pool: PgPool) {
    let site = Site::new(pool, true).await;
    site.arrive().await;
    site.pass().await;
    let mut conn = site.pool.acquire().await.unwrap();
    let refused = names::discard(&mut conn, "SYSB", "sysb").await;
    assert!(matches!(refused, Err(Refused::Refused(_))), "{refused:?}");
    assert!(site.staged().exists());
    assert_eq!(names::discard(&mut conn, "SYSB", "SYSB").await.unwrap(), 1);
    assert!(!site.staged().exists());
    let (state, reason): (String, Option<String>) =
        sqlx::query_as("SELECT state, reason FROM ingest_files")
            .fetch_one(&site.pool)
            .await
            .unwrap();
    assert_eq!(state, "rejected");
    assert!(reason.unwrap().contains("discarded"));
    assert!(matches!(
        names::new_system(&mut conn, "SYSB").await,
        Err(Refused::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn an_installs_first_system_is_taken_at_once(pool: PgPool) {
    let site = Site::new(pool, false).await;
    let path = site.arrive().await;
    assert!(matches!(site.pass().await[..], [Outcome::Filed(..)]));
    assert!(!path.exists());
    assert_eq!(site.filed_under().await.0, "SYSB");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_removed_recorders_staged_files_are_filed_once_their_system_is_decided(pool: PgPool) {
    let site = Site::new(pool, true).await;
    site.arrive().await;
    site.pass().await;
    let mut conn = site.pool.acquire().await.unwrap();
    scannerpulse::recorders::change(
        &mut conn,
        site.source_id,
        scannerpulse::recorders::Change::Remove,
    )
    .await
    .unwrap();
    names::new_system(&mut conn, "SYSB").await.unwrap();
    assert!(matches!(site.pass().await[..], [Outcome::Filed(..)]));
    assert!(!site.staged().exists());
    let credited: i16 = sqlx::query_scalar("SELECT source_id FROM calls")
        .fetch_one(&site.pool)
        .await
        .unwrap();
    assert_eq!(credited, site.source_id, "credited to its recorder");
}
