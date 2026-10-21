mod common;

use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use common::seed_places;
use scannerpulse::recorders::{self, Change, Refused, State};
use sqlx::PgPool;

/// A check that accepts any folder as it is written.
fn any(folder: &Path) -> Result<PathBuf, String> {
    Ok(folder.to_path_buf())
}

fn refused<T: std::fmt::Debug>(result: Result<T, Refused>) -> String {
    match result {
        Err(Refused::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn row(pool: &PgPool, id: i16) -> (bool, String, bool) {
    sqlx::query_as("SELECT enabled, intake, removed_at IS NOT NULL FROM sources WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_is_added_watched_or_taken_in_when_asked(pool: PgPool) {
    seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    let east = recorders::add(&mut conn, "east", Path::new("/srv/in/east"), false, any)
        .await
        .unwrap();
    assert_eq!(row(&pool, east).await, (true, "watched".into(), false));
    let north = recorders::add(&mut conn, "north", Path::new("/srv/in/north"), true, any)
        .await
        .unwrap();
    assert_eq!(row(&pool, north).await, (false, "asked".into(), false));
    let listed = recorders::list(&mut conn, Utc::now() - Duration::hours(1))
        .await
        .unwrap();
    let states: Vec<(&str, &str, State)> = listed
        .iter()
        .map(|r| (r.name.as_str(), r.folder.as_str(), r.state))
        .collect();
    assert_eq!(
        states,
        [
            ("live", "/srv/in/live", State::Watched),
            ("east", "/srv/in/east", State::Watched),
            ("north", "/srv/in/north", State::Asked),
        ]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_is_refused_a_name_or_folder_it_cannot_have(pool: PgPool) {
    seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    for (name, folder, expected) in [
        ("", "/srv/in/x", "the name is empty"),
        ("a/b", "/srv/in/x", "slash"),
        ("live", "/srv/in/x", "a recorder named live exists already"),
        ("x", "/srv/archive/in", "the archive /srv/archive"),
        ("x", "/srv", "the archive /srv/archive"),
        ("x", "/srv/in/live/sub", "the recorder live, /srv/in/live"),
        ("x", "/srv/in", "the recorder live, /srv/in/live"),
    ] {
        let why = refused(recorders::add(&mut conn, name, Path::new(folder), false, any).await);
        assert!(why.contains(expected), "{name} {folder}: {why}");
    }
    let why = refused(
        recorders::add(&mut conn, "x", Path::new("/srv/in/x"), false, |_| {
            Err("it is not there".into())
        })
        .await,
    );
    assert_eq!(why, "it is not there");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sources")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "nothing refused was added");
}

#[sqlx::test(migrations = "./migrations")]
async fn changes_move_a_recorder_between_watched_paused_and_asked(pool: PgPool) {
    let (_, live) = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    recorders::change(&mut conn, live, Change::Pause)
        .await
        .unwrap();
    assert_eq!(row(&pool, live).await, (false, "watched".into(), false));
    let why = refused(recorders::change(&mut conn, live, Change::BringIn).await);
    assert!(why.contains("taken as they arrive"), "{why}");
    recorders::change(&mut conn, live, Change::Ask)
        .await
        .unwrap();
    assert_eq!(row(&pool, live).await, (false, "asked".into(), false));
    let why = refused(recorders::change(&mut conn, live, Change::Pause).await);
    assert!(why.contains("nothing to pause"), "{why}");
    recorders::change(&mut conn, live, Change::BringIn)
        .await
        .unwrap();
    let listed = recorders::list(&mut conn, Utc::now()).await.unwrap();
    assert!(listed[0].bringing_in, "asked and not yet listed");
    sqlx::query("UPDATE sources SET brought_in_at = bring_in_at")
        .execute(&pool)
        .await
        .unwrap();
    let listed = recorders::list(&mut conn, Utc::now()).await.unwrap();
    assert!(!listed[0].bringing_in, "listed, and no file waits");
    recorders::change(&mut conn, live, Change::Watch)
        .await
        .unwrap();
    assert_eq!(row(&pool, live).await, (true, "watched".into(), false));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_removed_recorder_keeps_its_row_and_forgets_its_waiting_files(pool: PgPool) {
    let (_, live) = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    sqlx::query(
        "INSERT INTO ingest_files (source_id, rel_path, size, first_seen, seen_at, state) VALUES \
         ($1, 'a.mp3', 1, now(), now(), 'waiting'), ($1, 'b.mp3', 1, now(), now(), 'done')",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO source_health (host, source_id, ok, files, checked_at) \
         VALUES ('host-a', $1, true, 1, now())",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    recorders::change(&mut conn, live, Change::Remove)
        .await
        .unwrap();
    assert_eq!(row(&pool, live).await, (false, "watched".into(), true));
    let states: Vec<String> =
        sqlx::query_scalar("SELECT state FROM ingest_files ORDER BY rel_path")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(states, ["done"]);
    assert!(
        recorders::list(&mut conn, Utc::now())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(matches!(
        recorders::change(&mut conn, live, Change::Watch).await,
        Err(Refused::NoSuch(_))
    ));
    assert!(matches!(
        recorders::find(&mut conn, "live").await,
        Err(Refused::NoSuch(_))
    ));
    let why =
        refused(recorders::add(&mut conn, "live", Path::new("/srv/in/live"), false, any).await);
    assert!(why.contains("a removed recorder was named live"), "{why}");
    let again = recorders::add(&mut conn, "live-2", Path::new("/srv/in/live"), false, any).await;
    assert!(
        again.is_ok(),
        "a removed recorder's folder can be used again: {again:?}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn the_list_counts_each_recorders_files_of_the_day_and_its_folders_health(pool: PgPool) {
    let (_, live) = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    sqlx::query(
        "INSERT INTO ingest_files (source_id, rel_path, size, first_seen, seen_at, state) VALUES \
         ($1, 'old.mp3', 1, now() - interval '2 days', now(), 'done'), \
         ($1, 'a.mp3', 1, now() - interval '1 hour', now(), 'done'), \
         ($1, 'b.mp3', 1, now() - interval '1 hour', now(), 'done'), \
         ($1, 'c.mp3', 1, now() - interval '1 hour', now(), 'rejected'), \
         ($1, 'd.mp3', 1, now() - interval '1 minute', now(), 'waiting'), \
         ($1, 'e.mp3', 1, now() - interval '1 minute', now(), 'staged')",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO source_health (host, source_id, ok, problem, files, checked_at) VALUES \
         ('host-b', $1, false, 'cannot be listed', NULL, now()), \
         ('host-a', $1, true, NULL, 1, now())",
    )
    .bind(live)
    .execute(&pool)
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let listed = recorders::list(&mut conn, Utc::now() - Duration::hours(3))
        .await
        .unwrap();
    let live = &listed[0];
    assert_eq!(
        (
            live.filed_today,
            live.rejected_today,
            live.staged_today,
            live.waiting
        ),
        (2, 1, 1, 1)
    );
    assert!(live.last_file.unwrap() > Utc::now() - Duration::minutes(2));
    let hosts: Vec<(&str, bool, Option<i32>)> = live
        .hosts
        .iter()
        .map(|h| (h.host.as_str(), h.ok, h.files))
        .collect();
    assert_eq!(hosts, [("host-a", true, Some(1)), ("host-b", false, None)]);
}

#[test]
fn a_folder_is_checked_on_this_machine_before_it_is_used() {
    let dir = tempfile::tempdir().unwrap();
    let real = std::fs::canonicalize(dir.path()).unwrap();
    assert_eq!(recorders::check_here(dir.path()).unwrap(), real);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "the probe file is gone"
    );
    let why = recorders::check_here(Path::new("in/east")).unwrap_err();
    assert!(why.contains("not a full path"), "{why}");
    let why = recorders::check_here(&dir.path().join("none")).unwrap_err();
    assert!(why.contains("does not exist on this machine"), "{why}");
    std::fs::write(dir.path().join("file"), b"").unwrap();
    let why = recorders::check_here(&dir.path().join("file")).unwrap_err();
    assert!(why.contains("is a file"), "{why}");
}

#[test]
fn changes_are_named_as_the_command_line_and_the_page_name_them() {
    for (word, change) in [
        ("watch", Change::Watch),
        ("ask", Change::Ask),
        ("pause", Change::Pause),
        ("bring-in", Change::BringIn),
        ("remove", Change::Remove),
    ] {
        assert_eq!(word.parse::<Change>(), Ok(change));
    }
    assert!("delete".parse::<Change>().is_err());
}

#[test]
fn the_counts_of_the_day_start_at_the_sites_midnight() {
    let zone: chrono_tz::Tz = "America/Chicago".parse().unwrap();
    let at = |text: &str| chrono::DateTime::parse_from_rfc3339(text).unwrap().to_utc();
    assert_eq!(
        recorders::start_of_day(at("2031-01-15T03:00:00Z"), zone),
        at("2031-01-14T06:00:00Z"),
        "9 p.m. the day before, in Chicago"
    );
    assert_eq!(
        recorders::start_of_day(at("2031-07-15T12:00:00Z"), zone),
        at("2031-07-15T05:00:00Z")
    );
    let havana: chrono_tz::Tz = "America/Havana".parse().unwrap();
    // Havana's clocks go from midnight to 1 a.m. on 2031-03-09.
    assert_eq!(
        recorders::start_of_day(at("2031-03-09T12:00:00Z"), havana),
        at("2031-03-09T05:00:00Z")
    );
}

#[test]
fn root_checks_a_folder_as_the_service_account_and_says_why_it_fails() {
    let dir = tempfile::tempdir().unwrap();
    let why = recorders::reach_as("no-such-account", dir.path()).unwrap_err();
    assert!(
        why.contains("no-such-account account cannot list or write") || why.contains("runuser"),
        "{why}"
    );
}
