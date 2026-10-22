mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, send, sign_in};
use scannerpulse::talkgroups::{self, Renamed};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn names(pool: &PgPool, id: i32) -> (String, Option<String>, bool) {
    sqlx::query_as("SELECT name, files_name, files_correct FROM talkgroups WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rename_marks_a_talkgroup_with_calls_as_not_corrected(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    let eight = common::talkgroup(&pool, "SYSA", 8).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();

    // No calls: nothing in the archive carries the old name.
    assert_eq!(
        talkgroups::rename(&mut conn, eight, "Eight").await.unwrap(),
        Renamed::Done {
            files_correct: true
        }
    );
    assert_eq!(
        names(&pool, eight).await,
        ("Eight".into(), Some("Group 8".into()), true)
    );

    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Alpha / Fire")
            .await
            .unwrap(),
        Renamed::Done {
            files_correct: false
        }
    );
    // A second rename keeps the name the files carry.
    talkgroups::rename(&mut conn, seven, "Alpha Fire")
        .await
        .unwrap();
    assert_eq!(
        names(&pool, seven).await,
        ("Alpha Fire".into(), Some("Group 7".into()), false)
    );
    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Alpha Fire")
            .await
            .unwrap(),
        Renamed::Same
    );
    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Eight").await.unwrap(),
        Renamed::Taken(eight, "SYSA/8".into())
    );
    assert_eq!(
        talkgroups::rename(&mut conn, 999_999, "X").await.unwrap(),
        Renamed::NoSuch
    );
    // Another system may have the name.
    let other = common::talkgroup(&pool, "SYSB", 7).await;
    assert!(matches!(
        talkgroups::rename(&mut conn, other, "Eight").await.unwrap(),
        Renamed::Done { .. }
    ));

    let list = talkgroups::uncorrected(&pool).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id, list[0].calls), (seven, 1));
    assert_eq!(list[0].files_name.as_deref(), Some("Group 7"));

    // The files of Eight carry the name it had when it got a call.
    common::call(
        &pool,
        eight,
        Utc.with_ymd_and_hms(2031, 3, 2, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    talkgroups::rename(&mut conn, eight, "Eight 2")
        .await
        .unwrap();
    assert_eq!(
        names(&pool, eight).await,
        ("Eight 2".into(), Some("Eight".into()), false)
    );
}

#[test]
fn a_name_that_cannot_round_trip_is_refused() {
    assert!(talkgroups::name_problem("Alpha / Fire").is_none());
    for bad in [
        "",
        "  ",
        " Alpha",
        "Alpha ",
        "Al\"pha",
        "Al\npha",
        &"x".repeat(201),
    ] {
        assert!(talkgroups::name_problem(bad).is_some(), "{bad:?}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_recorders_different_name_marks_the_files_and_renames_nothing(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    let system: i32 = sqlx::query_scalar("SELECT system_id FROM talkgroups WHERE id = $1")
        .bind(seven)
        .fetch_one(&pool)
        .await
        .unwrap();
    let seen = scannerpulse::reader::Talkgroup {
        number: 7,
        name: "Group Seven".into(),
    };
    scannerpulse::store::talkgroup_id(&mut conn, system, &seen, 1)
        .await
        .unwrap();
    assert_eq!(
        names(&pool, seven).await,
        ("Group 7".into(), Some("Group Seven".into()), false)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_renames_a_talkgroup(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    common::talkgroup(&pool, "SYSA", 8).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let app = app(&pool);
    let uri = format!("/api/admin/talkgroups/{seven}/rename");
    assert_eq!(
        send(&app, "POST", &uri, &[], Some(json!({"name": "A"})))
            .await
            .status,
        401
    );
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let done = send(&app, "POST", &uri, &change, Some(json!({"name": "Alpha"}))).await;
    assert_eq!(done.status, 200);
    assert_eq!(done.body["files_correct"], false);
    let taken = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"name": "Group 8"})),
    )
    .await;
    assert_eq!(taken.status, 409);
    assert!(taken.body["detail"].as_str().unwrap().contains("SYSA/8"));
    let bad = send(&app, "POST", &uri, &change, Some(json!({"name": "A\"B"}))).await;
    assert_eq!(bad.status, 400);
    let missing = send(
        &app,
        "POST",
        "/api/admin/talkgroups/999999/rename",
        &change,
        Some(json!({"name": "X"})),
    )
    .await;
    assert_eq!(missing.status, 404);
    let list = send(
        &app,
        "GET",
        "/api/admin/data/uncorrected",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body[0]["name"], "Alpha");
    assert_eq!(list.body[0]["files_name"], "Group 7");
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

#[sqlx::test(migrations = "./migrations")]
async fn a_correction_moves_each_file_under_the_new_name(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let old_dir = "2031-03-01/127-Alpha Fire";
    let titled_name = "20310301_120000SYSA_SYSA-P25_T-SYSA-North__TO_127_FROM_711351.mp3";
    let titled_data = common::tagged(&tag(Some("127\"Alpha Fire\"")), 3);
    let (titled, talkgroup) = filed(
        &pool,
        places,
        &root,
        &format!("{old_dir}/{titled_name}"),
        Some(titled_data.clone()),
        at(0),
    )
    .await;
    let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_930_000_000);
    std::fs::File::options()
        .write(true)
        .open(root.join(format!("{old_dir}/{titled_name}")))
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let token_name = "2031-03-01_120500_127-Alpha Fire_711351-_SYSA-North_SYSA.mp3";
    let (token, _) = filed(
        &pool,
        places,
        &root,
        &format!("{old_dir}/{token_name}"),
        Some(common::tagged(&tag(None), 2)),
        at(5),
    )
    .await;
    let (missing, _) = filed(
        &pool,
        places,
        &root,
        &format!("{old_dir}/20310301_121000.mp3"),
        None,
        at(10),
    )
    .await;
    let patch_rel = format!("{old_dir}/20310301_121500.mp3");
    let (patch, _) = filed(
        &pool,
        places,
        &root,
        &patch_rel,
        Some(common::tagged(&tag(Some("P:127,128 \"Alpha\"")), 1)),
        at(15),
    )
    .await;

    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha / Fire")
        .await
        .unwrap();
    let preview = scannerpulse::data::correct_preview(&mut conn, talkgroup)
        .await
        .unwrap();
    assert_eq!((preview.calls, preview.read_only), (4, 0));
    assert!(
        scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha Fire", "ann")
            .await
            .is_err(),
        "the old name does not confirm"
    );
    let (job, count) =
        scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha / Fire", "ann")
            .await
            .unwrap();
    assert_eq!(count, 4);
    run_jobs(&pool).await;

    let new_dir = "2031-03-01/127-Alpha _ Fire";
    assert_eq!(
        rel_of(&pool, titled).await,
        format!("{new_dir}/{titled_name}")
    );
    let moved = std::fs::read(root.join(format!("{new_dir}/{titled_name}"))).unwrap();
    assert_eq!(
        scannerpulse::reader::tags(&moved).title.as_deref(),
        Some("127\"Alpha / Fire\"")
    );
    assert_eq!(
        std::fs::metadata(root.join(format!("{new_dir}/{titled_name}")))
            .unwrap()
            .modified()
            .unwrap(),
        mtime,
        "the file keeps its modification time"
    );
    assert!(!root.join(format!("{old_dir}/{titled_name}")).exists());
    // A file name cannot hold the `/`, and this file has no title to carry
    // it: the file is refused and stays as it was.
    assert_eq!(
        rel_of(&pool, token).await,
        format!("{old_dir}/{token_name}")
    );
    // The missing file and the patch stay where they were.
    assert_eq!(
        rel_of(&pool, missing).await,
        format!("{old_dir}/20310301_121000.mp3")
    );
    assert_eq!(rel_of(&pool, patch).await, patch_rel);
    assert!(root.join(&patch_rel).exists());
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done");
    assert!(
        ended
            .message
            .contains("1 files of SYSA/127 corrected; 2 refused, 1 missing"),
        "{}",
        ended.message
    );
    assert!(
        !names(&pool, talkgroup).await.2,
        "three calls still carry the old name"
    );
}

/// Marks the call as filed through a mapped spelling of SYSA.
async fn mapped(pool: &PgPool, call: i64) {
    sqlx::query("INSERT INTO mapped_calls SELECT id, started_at FROM calls WHERE id = $1")
        .bind(call)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE talkgroups SET files_correct = false \
         WHERE id = (SELECT talkgroup_id FROM calls WHERE id = $1)",
    )
    .bind(call)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_correction_rewrites_the_system_of_mapped_calls(pool: PgPool) {
    use id3::TagLike;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let sysb = |title: &str| {
        let mut tag = tag(Some(title));
        tag.set_text("TIT1", "SYSB");
        common::tagged(&tag, 2)
    };
    // In its folder already: rewritten where it is.
    let here = "2031-03-01/127-Alpha Fire/20310301_120000SYSB_South__TO_127_FROM_711351.mp3";
    let (kept, talkgroup) = filed(
        &pool,
        places,
        &root,
        here,
        Some(sysb("127\"Alpha Fire\"")),
        at(0),
    )
    .await;
    let mtime = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_930_000_000);
    std::fs::File::options()
        .write(true)
        .open(root.join(here))
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    // Under the name the recorder sent: moved too.
    let name = "20310301_120500SYSB_South__TO_127_FROM_711351.mp3";
    let (moved, _) = filed(
        &pool,
        places,
        &root,
        &format!("2031-03-01/127-Alpha/{name}"),
        Some(sysb("127\"Alpha\"")),
        at(5),
    )
    .await;
    mapped(&pool, kept).await;
    mapped(&pool, moved).await;

    let mut conn = pool.acquire().await.unwrap();
    let preview = scannerpulse::data::correct_preview(&mut conn, talkgroup)
        .await
        .unwrap();
    assert_eq!(preview.calls, 2);
    scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;

    let system = |rel: &str| {
        let data = std::fs::read(root.join(rel)).unwrap();
        let name = rel.rsplit('/').next().unwrap();
        scannerpulse::reader::describe(name, &scannerpulse::reader::tags(&data)).system
    };
    assert_eq!(rel_of(&pool, kept).await, here, "the file keeps its name");
    assert_eq!(system(here).as_deref(), Some("SYSA"));
    assert_eq!(
        std::fs::metadata(root.join(here))
            .unwrap()
            .modified()
            .unwrap(),
        mtime,
        "the file keeps its modification time"
    );
    let new_rel = format!("2031-03-01/127-Alpha Fire/{name}");
    assert_eq!(rel_of(&pool, moved).await, new_rel);
    assert_eq!(system(&new_rel).as_deref(), Some("SYSA"));
    assert!(!root.join(format!("2031-03-01/127-Alpha/{name}")).exists());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM mapped_calls")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert!(names(&pool, talkgroup).await.2, "the files are correct");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_correction_leaves_every_row_with_its_file(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let n = scannerpulse::data::FILES_PER_STEP + 10;
    let mut calls = Vec::new();
    let mut talkgroup = 0;
    for i in 0..n {
        let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap() + chrono::Duration::minutes(i);
        let name = format!("{}SYSA.mp3", at.format("%Y%m%d_%H%M%S"));
        let (call, id) = filed(
            &pool,
            places,
            &root,
            &format!("2031-03-01/127-Alpha Fire/{name}"),
            Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
            at,
        )
        .await;
        calls.push(call);
        talkgroup = id;
    }
    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha 2")
        .await
        .unwrap();
    let (job, _) = scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    scannerpulse::jobs::stop(&mut conn, job, "ann")
        .await
        .unwrap();
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    assert_eq!(
        scannerpulse::jobs::get(&pool, job)
            .await
            .unwrap()
            .unwrap()
            .state,
        "stopped"
    );
    let mut in_new = 0;
    for &call in &calls {
        let rel = rel_of(&pool, call).await;
        assert!(root.join(&rel).exists(), "{rel} has its file");
        in_new += i64::from(rel.starts_with("2031-03-01/127-Alpha 2/"));
    }
    assert_eq!(in_new, scannerpulse::data::FILES_PER_STEP);
    let old_files = std::fs::read_dir(root.join("2031-03-01/127-Alpha Fire"))
        .unwrap()
        .count();
    assert_eq!(
        old_files as i64,
        n - in_new,
        "the stop removed the replaced files"
    );

    scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    assert!(
        !root.join("2031-03-01/127-Alpha Fire").exists(),
        "the empty folder goes"
    );
    assert_eq!(
        names(&pool, talkgroup).await,
        ("Alpha 2".into(), None, true)
    );
    assert!(talkgroups::uncorrected(&pool).await.unwrap().is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_starts_a_correction_with_its_name_typed(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    let app = app(&pool);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let uri = format!("/api/admin/talkgroups/{seven}/correct");
    let preview = send(&app, "GET", &uri, &[("cookie", &cookie)], None).await;
    assert_eq!(preview.status, 200);
    assert_eq!(preview.body["calls"], 0);
    assert_eq!(preview.body["talkgroup"]["name"], "Group 7");
    let wrong = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"confirm": "Group 8"})),
    )
    .await;
    assert_eq!(wrong.status, 400);
    let started = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"confirm": "Group 7"})),
    )
    .await;
    assert_eq!(started.status, 201);
    let missing = send(
        &app,
        "GET",
        "/api/admin/talkgroups/999999/correct",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(missing.status, 400);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_that_cannot_be_read_is_refused_and_the_job_ends(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let old_dir = "2031-03-01/127-Alpha Fire";
    let (good, talkgroup) = filed(
        &pool,
        places,
        &root,
        &format!("{old_dir}/20310301_120000SYSA.mp3"),
        Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
        at(0),
    )
    .await;
    // A directory where the file should be: reading it fails, even as root.
    let bad_rel = format!("{old_dir}/20310301_120500SYSA.mp3");
    let (bad, _) = filed(&pool, places, &root, &bad_rel, None, at(5)).await;
    std::fs::create_dir_all(root.join(&bad_rel)).unwrap();

    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha / Fire")
        .await
        .unwrap();
    let (job, count) =
        scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha / Fire", "ann")
            .await
            .unwrap();
    assert_eq!(count, 2);
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done");
    assert!(
        ended
            .message
            .contains("1 files of SYSA/127 corrected; 1 refused"),
        "{}",
        ended.message
    );
    assert!(
        rel_of(&pool, good)
            .await
            .starts_with("2031-03-01/127-Alpha _ Fire/")
    );
    assert_eq!(rel_of(&pool, bad).await, bad_rel);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_name_too_long_for_a_file_name_is_refused(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    let rel =
        "2031-03-01/127-Alpha Fire/2031-03-01_120000_127-Alpha Fire_711351-_SYSA-North_SYSA.mp3";
    let (call, talkgroup) = filed(
        &pool,
        places,
        &root,
        rel,
        Some(common::tagged(&tag(None), 2)),
        at,
    )
    .await;

    let mut conn = pool.acquire().await.unwrap();
    // 200 characters, 400 bytes: the name rules take it, the file system not.
    let long = "é".repeat(200);
    talkgroups::rename(&mut conn, talkgroup, &long)
        .await
        .unwrap();
    let (job, count) = scannerpulse::data::start_correct(&mut conn, talkgroup, &long, "ann")
        .await
        .unwrap();
    assert_eq!(count, 1);
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done");
    assert!(ended.message.contains("1 refused"), "{}", ended.message);
    assert_eq!(rel_of(&pool, call).await, rel);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_merge_moves_the_calls_and_favorites_then_corrects_the_files(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let mut calls = Vec::new();
    let mut from = 0;
    for minute in 0..3 {
        let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
        let name = format!("{}SYSA.mp3", at.format("%Y%m%d_%H%M%S"));
        let (call, id) = filed(
            &pool,
            places,
            &root,
            &format!("2031-03-01/127-Alpha Fire/{name}"),
            Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
            at,
        )
        .await;
        calls.push((call, name));
        from = id;
    }
    let into = common::talkgroup(&pool, "SYSA", 128).await;
    sqlx::query("INSERT INTO favorites (id, name) VALUES (1, 'one'), (2, 'both')")
        .execute(&pool)
        .await
        .unwrap();
    for (favorite, talkgroup) in [(1, from), (2, from), (2, into)] {
        sqlx::query("INSERT INTO favorite_talkgroups (favorite_id, talkgroup_id) VALUES ($1, $2)")
            .bind(favorite as i64)
            .bind(talkgroup)
            .execute(&pool)
            .await
            .unwrap();
    }
    let mut conn = pool.acquire().await.unwrap();
    let other_system = common::talkgroup(&pool, "SYSB", 9).await;
    assert!(
        scannerpulse::data::merge_preview(&mut conn, from, other_system)
            .await
            .is_err()
    );
    assert!(
        scannerpulse::data::merge_preview(&mut conn, from, from)
            .await
            .is_err()
    );
    let preview = scannerpulse::data::merge_preview(&mut conn, from, into)
        .await
        .unwrap();
    assert_eq!(preview.calls, 3);
    assert!(
        scannerpulse::data::start_merge(&mut conn, from, into, "Group 128", "ann")
            .await
            .is_err(),
        "the name of the talkgroup that stays does not confirm"
    );
    let (job, _) = scannerpulse::data::start_merge(&mut conn, from, into, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    assert!(
        ended
            .message
            .starts_with("3 calls moved; 3 files of SYSA/128 corrected"),
        "{}",
        ended.message
    );
    for (call, name) in &calls {
        let rel = rel_of(&pool, *call).await;
        assert_eq!(rel, format!("2031-03-01/128-Group 128/{name}"));
        let data = std::fs::read(root.join(&rel)).unwrap();
        assert_eq!(
            scannerpulse::reader::tags(&data).title.as_deref(),
            Some("128\"Group 128\"")
        );
    }
    assert!(!root.join("2031-03-01/127-Alpha Fire").exists());
    let gone: bool =
        sqlx::query_scalar("SELECT NOT EXISTS (SELECT 1 FROM talkgroups WHERE id = $1)")
            .bind(from)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(gone);
    let favorites: Vec<(i64, i32)> =
        sqlx::query_as("SELECT favorite_id, talkgroup_id FROM favorite_talkgroups ORDER BY 1, 2")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(favorites, [(1, into), (2, into)]);
    assert!(names(&pool, into).await.2, "the files are correct again");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_merges_with_the_name_of_the_talkgroup_that_goes(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    let eight = common::talkgroup(&pool, "SYSA", 8).await;
    let app = app(&pool);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let uri = format!("/api/admin/talkgroups/{seven}/merge");
    let preview = send(
        &app,
        "GET",
        &format!("{uri}?into={eight}"),
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(preview.status, 200);
    assert_eq!(preview.body["into"]["name"], "Group 8");
    let wrong = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"into": eight, "confirm": "Group 8"})),
    )
    .await;
    assert_eq!(wrong.status, 400);
    let started = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"into": eight, "confirm": "Group 7"})),
    )
    .await;
    assert_eq!(started.status, 201);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_merge_into_a_talkgroup_that_went_fails_and_frees_the_line(pool: PgPool) {
    let a = common::talkgroup(&pool, "SYSA", 1).await;
    let b = common::talkgroup(&pool, "SYSA", 2).await;
    let c = common::talkgroup(&pool, "SYSA", 3).await;
    let mut conn = pool.acquire().await.unwrap();
    let (first, _) = scannerpulse::data::start_merge(&mut conn, a, b, "Group 1", "ann")
        .await
        .unwrap();
    let (second, _) = scannerpulse::data::start_merge(&mut conn, c, a, "Group 3", "ann")
        .await
        .unwrap();
    let (third, _) = scannerpulse::data::start_correct(&mut conn, b, "Group 2", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let state = |id| {
        let pool = pool.clone();
        async move { scannerpulse::jobs::get(&pool, id).await.unwrap().unwrap() }
    };
    assert_eq!(state(first).await.state, "done");
    let failed = state(second).await;
    assert_eq!(failed.state, "failed");
    assert!(failed.message.contains("is gone"), "{}", failed.message);
    assert_eq!(state(third).await.state, "done");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_merge_leaves_the_talkgroup_marked_from_its_first_move(pool: PgPool) {
    let from = common::talkgroup(&pool, "SYSA", 1).await;
    let into = common::talkgroup(&pool, "SYSA", 2).await;
    common::call(
        &pool,
        from,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_merge(&mut conn, from, into, "Group 1", "ann")
        .await
        .unwrap();
    assert!(names(&pool, into).await.2);
    let context = scannerpulse::jobs::Context::new(None);
    let step = scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    assert_eq!(step, Some((job, "running".to_string())));
    scannerpulse::jobs::stop(&mut conn, job, "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "stopped");
    assert!(!names(&pool, into).await.2, "marked from the first move");
}

/// Sets the holding folder; when it exists, puts the sentinel in it as an
/// administrator would.
async fn set_holding(pool: &PgPool, path: &std::path::Path) {
    if path.is_dir() {
        std::fs::write(path.join(scannerpulse::data::HOLDING_SENTINEL), b"").unwrap();
    }
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('purge.holding_path', to_jsonb($1::text)) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(path.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_keeps_the_files_in_the_holding_folder(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let holding = dir.path().join("holding");
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let rels = [
        "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3",
        "2031-03-01/127-Alpha Fire/20310301_120100SYSA.mp3",
    ];
    let mut talkgroup = 0;
    for (minute, rel) in rels.iter().enumerate() {
        let (_, id) = filed(
            &pool,
            places,
            &root,
            rel,
            Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
            at(minute as u32),
        )
        .await;
        talkgroup = id;
    }
    filed(
        &pool,
        places,
        &root,
        "2031-03-01/127-Alpha Fire/gone.mp3",
        None,
        at(2),
    )
    .await;
    let other = common::talkgroup(&pool, "SYSA", 128).await;
    let kept = common::call(&pool, other, at(3), None).await;

    let mut conn = pool.acquire().await.unwrap();
    let preview = scannerpulse::data::purge_preview(&mut conn, talkgroup)
        .await
        .unwrap();
    assert_eq!((preview.calls, preview.read_only), (3, 0));
    assert_eq!(preview.holding_path, None);
    let refused = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("purge.holding_path"), "{refused}");
    // The folder checks run on the host that runs the job.
    set_holding(&pool, &root.join("2031-03-01")).await;
    let (overlap, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let failed = scannerpulse::jobs::get(&pool, overlap)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert!(failed.message.contains("overlaps"), "{}", failed.message);
    set_holding(&pool, &holding).await;
    assert!(
        scannerpulse::data::start_purge(&mut conn, talkgroup, "alpha fire", "ann")
            .await
            .is_err()
    );
    let (job, count) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    assert_eq!(count, 3);
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done");
    assert!(
        ended.message.starts_with("3 calls of SYSA/127 purged")
            && ended.message.ends_with("1 had no file"),
        "{}",
        ended.message
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
        .bind(talkgroup)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    for rel in rels {
        assert!(holding.join(rel).exists(), "{rel} is kept");
        assert!(!root.join(rel).exists(), "{rel} left the archive");
    }
    let other_left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE id = $1")
        .bind(kept)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(other_left, 1, "another talkgroup's call stays");
    assert_eq!(
        names(&pool, talkgroup).await.0,
        "Alpha Fire",
        "the talkgroup stays"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_takes_only_the_records_of_calls_in_a_read_only_location(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let (root, old, holding) = (
        dir.path().join("archive"),
        dir.path().join("old"),
        dir.path().join("holding"),
    );
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let rel = "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3";
    let (_, talkgroup) = filed(
        &pool,
        places,
        &root,
        rel,
        Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
        at(0),
    )
    .await;
    // An import's call of the same talkgroup: its file is never changed.
    let old_id: i16 = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode) VALUES ('Old', $1, 'readonly') RETURNING id",
    )
    .bind(old.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    let old_rel = "2031-03-01/127-Alpha Fire/20310301_120100SYSA.mp3";
    std::fs::create_dir_all(old.join(old_rel).parent().unwrap()).unwrap();
    std::fs::write(old.join(old_rel), b"old").unwrap();
    sqlx::query(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
         VALUES ($1, 2000, $2, $3, $4, $5)",
    )
    .bind(at(1))
    .bind(talkgroup)
    .bind(places.1)
    .bind(old_id)
    .bind(old_rel)
    .execute(&pool)
    .await
    .unwrap();

    let mut conn = pool.acquire().await.unwrap();
    let preview = scannerpulse::data::purge_preview(&mut conn, talkgroup)
        .await
        .unwrap();
    assert_eq!((preview.calls, preview.read_only), (2, 1));
    set_holding(&pool, &holding).await;
    let (job, count) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    assert_eq!(count, 2);
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    assert!(
        ended
            .message
            .contains("1 in a read-only location lost only its record"),
        "{}",
        ended.message
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
        .bind(talkgroup)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert!(holding.join(rel).exists());
    assert!(!root.join(rel).exists());
    assert!(
        old.join(old_rel).exists(),
        "a read-only location's file stays"
    );
    assert!(!holding.join(old_rel).exists());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_purges_only_with_a_holding_folder_and_the_name(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    let app = app(&pool);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let uri = format!("/api/admin/talkgroups/{seven}/purge");
    let preview = send(&app, "GET", &uri, &[("cookie", &cookie)], None).await;
    assert_eq!(preview.status, 200);
    assert_eq!(preview.body["holding_path"], serde_json::Value::Null);
    let unset = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"confirm": "Group 7"})),
    )
    .await;
    assert_eq!(unset.status, 400);
    let dir = tempfile::tempdir().unwrap();
    set_holding(&pool, dir.path()).await;
    let started = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"confirm": "Group 7"})),
    )
    .await;
    assert_eq!(started.status, 201);
    assert_eq!(
        send(
            &app,
            "POST",
            &uri,
            &[("cookie", &cookie)],
            Some(json!({"confirm": "Group 7"}))
        )
        .await
        .status,
        403,
        "a change needs the CSRF token"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_refuses_an_unreadable_file_and_ends(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let holding = dir.path().join("holding");
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    set_holding(&pool, &holding).await;
    let at = |minute| Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap();
    let good = "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3";
    let bad = "2031-03-01/127-Alpha Fire/20310301_120100SYSA.mp3";
    let (good_call, talkgroup) = filed(
        &pool,
        places,
        &root,
        good,
        Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
        at(0),
    )
    .await;
    // A directory where the file should be cannot be read as a file.
    std::fs::create_dir_all(root.join(bad)).unwrap();
    let (bad_call, _) = filed(&pool, places, &root, bad, None, at(1)).await;

    let mut conn = pool.acquire().await.unwrap();
    let (job, count) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    assert_eq!(count, 2);
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    assert!(ended.message.contains("1 refused"), "{}", ended.message);
    let pool = &pool;
    let rows = |id| async move {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM calls WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    };
    assert_eq!(rows(good_call).await, 0, "the good call is gone");
    assert!(holding.join(good).exists(), "its file is held");
    assert!(!root.join(good).exists(), "and left the archive");
    assert_eq!(rows(bad_call).await, 1, "the refused call stays");
    assert!(root.join(bad).exists(), "and so does its file");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_refuses_a_call_whose_folder_is_not_there(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let holding = dir.path().join("holding");
    std::fs::create_dir_all(&holding).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    set_holding(&pool, &holding).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    // The day folder does not exist, as when a share is not mounted.
    let rel = "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3";
    let (call, talkgroup) = filed(&pool, places, &root, rel, None, at).await;

    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;

    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    assert!(ended.message.contains("1 refused"), "{}", ended.message);
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE id = $1")
        .bind(call)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 1, "the row stays");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_purge_resumes_with_a_new_job(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let holding = dir.path().join("holding");
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    set_holding(&pool, &holding).await;
    let n = scannerpulse::data::FILES_PER_STEP + 10;
    let mut calls = Vec::new();
    let mut talkgroup = 0;
    for i in 0..n {
        let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap() + chrono::Duration::minutes(i);
        let name = format!("{}SYSA.mp3", at.format("%Y%m%d_%H%M%S"));
        let rel = format!("2031-03-01/127-Alpha Fire/{name}");
        let (call, id) = filed(
            &pool,
            places,
            &root,
            &rel,
            Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
            at,
        )
        .await;
        calls.push((call, rel));
        talkgroup = id;
    }
    let mut conn = pool.acquire().await.unwrap();
    let (first, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    scannerpulse::jobs::stop(&mut conn, first, "ann")
        .await
        .unwrap();
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    let stopped = scannerpulse::jobs::get(&pool, first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stopped.state, "stopped");
    let (second, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, second)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    for (call, rel) in &calls {
        assert!(holding.join(rel).exists(), "{rel} is held");
        assert!(!root.join(rel).exists(), "{rel} left the archive");
        assert_eq!(rel_of_opt(&pool, *call).await, None, "its row is gone");
    }
    let rows_missing: i64 =
        sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
            .bind(talkgroup)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows_missing, 0);
    let _ = (first, second);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_needs_the_sentinel_in_the_holding_folder(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let holding = dir.path().join("holding");
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let (call, talkgroup) = filed(
        &pool,
        places,
        &root,
        "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3",
        Some(common::tagged(&tag(Some("127\"Alpha Fire\"")), 1)),
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
    )
    .await;
    // An empty folder, as an unmounted share's mount point is: no sentinel.
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('purge.holding_path', to_jsonb($1::text))",
    )
    .bind(holding.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "failed");
    assert!(
        ended.message.contains(".scannerpulse-holding"),
        "{}",
        ended.message
    );
    assert!(rel_of_opt(&pool, call).await.is_some(), "the call stays");
    assert_eq!(
        std::fs::read_dir(&holding).unwrap().count(),
        0,
        "nothing was copied"
    );
}

async fn rel_of_opt(pool: &PgPool, call: i64) -> Option<String> {
    sqlx::query_scalar("SELECT rel_path FROM calls WHERE id = $1")
        .bind(call)
        .fetch_optional(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_waits_for_a_queued_merge_into_the_talkgroup(pool: PgPool) {
    let a = common::talkgroup(&pool, "SYSA", 1).await;
    let b = common::talkgroup(&pool, "SYSA", 2).await;
    let dir = tempfile::tempdir().unwrap();
    set_holding(&pool, dir.path()).await;
    let mut conn = pool.acquire().await.unwrap();
    scannerpulse::data::start_merge(&mut conn, a, b, "Group 1", "ann")
        .await
        .unwrap();
    let refused = scannerpulse::data::start_purge(&mut conn, b, "Group 2", "ann")
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("merge"), "{refused}");
}

#[sqlx::test(migrations = "./migrations")]
async fn two_renames_to_one_name_at_once_take_turns(pool: PgPool) {
    for round in 0..10 {
        let a = common::talkgroup(&pool, "SYSA", 100 + round * 2).await;
        let b = common::talkgroup(&pool, "SYSA", 101 + round * 2).await;
        let name = format!("Same {round}");
        let mut first = pool.acquire().await.unwrap();
        let mut second = pool.acquire().await.unwrap();
        let (one, two) = tokio::join!(
            talkgroups::rename(&mut first, a, &name),
            talkgroups::rename(&mut second, b, &name)
        );
        let outcomes = [one.unwrap(), two.unwrap()];
        let done = outcomes
            .iter()
            .filter(|r| matches!(r, Renamed::Done { .. }))
            .count();
        let taken = outcomes
            .iter()
            .filter(|r| matches!(r, Renamed::Taken(..)))
            .count();
        assert_eq!((done, taken), (1, 1), "{outcomes:?}");
    }
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
async fn a_rename_during_a_correction_is_finished_by_the_next_correction(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let (calls, talkgroup) = alpha_calls(
        &pool,
        places,
        &root,
        scannerpulse::data::FILES_PER_STEP + 10,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha 2")
        .await
        .unwrap();
    scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    // A rename between two steps: the rest goes to the newest name, and the
    // files the first step moved are left for the next correction.
    talkgroups::rename(&mut conn, talkgroup, "Alpha 3")
        .await
        .unwrap();
    run_jobs(&pool).await;
    assert!(!names(&pool, talkgroup).await.2, "still listed");
    scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 3", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    for call in calls {
        let rel = rel_of(&pool, call).await;
        assert!(rel.starts_with("2031-03-01/127-Alpha 3/"), "{rel}");
        assert!(root.join(&rel).is_file(), "{rel} has its file");
    }
    assert!(names(&pool, talkgroup).await.2, "corrected");
    assert!(!root.join("2031-03-01/127-Alpha 2").exists());
    assert!(!root.join("2031-03-01/127-Alpha Fire").exists());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_correction_refuses_a_call_whose_folder_is_not_there(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let (calls, talkgroup) = alpha_calls(&pool, places, &root, 1).await;
    // Its day's folder is not there, as on a share that is not mounted.
    let (lost, _) = filed(
        &pool,
        places,
        &root,
        "2031-03-02/127-Alpha Fire/20310302_120000SYSA.mp3",
        None,
        Utc.with_ymd_and_hms(2031, 3, 2, 12, 0, 0).unwrap(),
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha 2")
        .await
        .unwrap();
    let (job, _) = scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert!(
        ended
            .message
            .contains("1 files of SYSA/127 corrected; 1 refused, 0 missing"),
        "{}",
        ended.message
    );
    assert!(
        ended.message.contains("its folder is not there"),
        "{}",
        ended.message
    );
    assert_eq!(
        rel_of(&pool, lost).await,
        "2031-03-02/127-Alpha Fire/20310302_120000SYSA.mp3"
    );
    assert!(
        rel_of(&pool, calls[0])
            .await
            .starts_with("2031-03-01/127-Alpha 2/")
    );
}

/// `n` calls of SYSA/7 with no files, one a second from 2031-03-01, put in
/// with one statement; the talkgroup's id.
async fn many_calls(pool: &PgPool, places: (i16, i16), n: i64) -> i32 {
    let from = common::talkgroup(pool, "SYSA", 7).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 0, 0, 0).unwrap();
    scannerpulse::store::ensure_partition(pool, at)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
         SELECT $1 + make_interval(secs => n), 1000, $2, $3, $4, \
                '2031-03-01/7-Group 7/f' || n || '.mp3' \
         FROM generate_series(1, $5) n",
    )
    .bind(at)
    .bind(from)
    .bind(places.1)
    .bind(places.0)
    .bind(n)
    .execute(pool)
    .await
    .unwrap();
    from
}

async fn calls_of(pool: &PgPool, talkgroup: i32) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
        .bind(talkgroup)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_large_merge_stopped_after_one_step_is_finished_by_another(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("archive")).unwrap();
    let places =
        common::seed_places(&pool, &dir.path().join("archive"), &dir.path().join("in")).await;
    let n = scannerpulse::data::MERGE_CALLS_PER_STEP + 1;
    let from = many_calls(&pool, places, n).await;
    let into = common::talkgroup(&pool, "SYSA", 8).await;
    let mut conn = pool.acquire().await.unwrap();
    let (job, count) = scannerpulse::data::start_merge(&mut conn, from, into, "Group 7", "ann")
        .await
        .unwrap();
    assert_eq!(count, n);
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    scannerpulse::jobs::stop(&mut conn, job, "ann")
        .await
        .unwrap();
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    assert_eq!(
        scannerpulse::jobs::get(&pool, job)
            .await
            .unwrap()
            .unwrap()
            .state,
        "stopped"
    );
    assert_eq!(
        calls_of(&pool, into).await,
        scannerpulse::data::MERGE_CALLS_PER_STEP
    );
    assert_eq!(calls_of(&pool, from).await, 1);
    assert!(!names(&pool, into).await.2, "marked from the first move");
    let (again, _) = scannerpulse::data::start_merge(&mut conn, from, into, "Group 7", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, again)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
    assert_eq!(calls_of(&pool, into).await, n);
    assert!(
        scannerpulse::data::named(&pool, from)
            .await
            .unwrap()
            .is_none()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_merge_whose_talkgroup_went_after_its_calls_moved_finishes(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("archive")).unwrap();
    let places =
        common::seed_places(&pool, &dir.path().join("archive"), &dir.path().join("in")).await;
    let from = many_calls(&pool, places, scannerpulse::data::MERGE_CALLS_PER_STEP + 1).await;
    let into = common::talkgroup(&pool, "SYSA", 8).await;
    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_merge(&mut conn, from, into, "Group 7", "ann")
        .await
        .unwrap();
    let context = scannerpulse::jobs::Context::new(None);
    scannerpulse::jobs::step(&pool, "host-a", &context)
        .await
        .unwrap();
    // Another job took the last call and the talkgroup in the meantime.
    sqlx::query("UPDATE calls SET talkgroup_id = $2 WHERE talkgroup_id = $1")
        .bind(from)
        .bind(into)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM talkgroups WHERE id = $1")
        .bind(from)
        .execute(&pool)
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "done", "{}", ended.message);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_holding_folder_in_an_ingest_source_fails_the_purge(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let source = dir.path().join("in");
    std::fs::create_dir_all(source.join("held")).unwrap();
    let places = common::seed_places(&pool, &root, &source).await;
    let (_, talkgroup) = alpha_calls(&pool, places, &root, 1).await;
    set_holding(&pool, &source.join("held")).await;
    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    run_jobs(&pool).await;
    let ended = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(ended.state, "failed");
    assert!(
        ended.message.contains("overlaps the ingest source"),
        "{}",
        ended.message
    );
    assert_eq!(calls_of(&pool, talkgroup).await, 1);
}

#[test]
fn a_relative_holding_folder_is_refused() {
    let none = std::collections::HashMap::new();
    let check = |value: &str| {
        scannerpulse::settings::check("purge.holding_path", Some(&json!(value)), &none)
    };
    assert!(check("held/calls").is_err());
    assert!(check("/srv/held/../calls").is_err());
    assert!(check("/srv/held").is_ok());
}

#[sqlx::test(migrations = "./migrations")]
async fn migration_0007_marks_only_renamed_talkgroups_with_calls(pool: PgPool) {
    let with_calls = common::talkgroup(&pool, "SYSA", 7).await;
    let without = common::talkgroup(&pool, "SYSA", 8).await;
    let never_renamed = common::talkgroup(&pool, "SYSA", 9).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    common::call(&pool, with_calls, at, None).await;
    common::call(&pool, never_renamed, at, None).await;
    sqlx::query(
        "UPDATE talkgroups SET files_name = 'Old', files_correct = true WHERE id IN ($1, $2)",
    )
    .bind(with_calls)
    .bind(without)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0007_files_correct.sql"))
        .execute(&pool)
        .await
        .unwrap();
    assert!(!names(&pool, with_calls).await.2);
    assert!(names(&pool, without).await.2);
    assert!(names(&pool, never_renamed).await.2);
}

/// Runs the jobs in a thread of their own for 20 s at most, with a 2 s limit
/// on each file: Err when they still wait then.
async fn jobs_within(pool: &PgPool) -> Result<(), ()> {
    let mut context = scannerpulse::jobs::Context::new(None);
    context.file_limit = std::time::Duration::from_secs(2);
    let (pool, handle) = (pool.clone(), tokio::runtime::Handle::current());
    let stepping = tokio::task::spawn_blocking(move || {
        handle.block_on(async {
            while scannerpulse::jobs::step(&pool, "host-a", &context)
                .await
                .unwrap()
                .is_some()
            {}
        })
    });
    tokio::time::timeout(std::time::Duration::from_secs(20), stepping)
        .await
        .map(|_| ())
        .map_err(|_| ())
}

/// Lets a reader left waiting on the pipe end, when there is one; opening
/// it to write would otherwise wait for a reader for ever.
fn release(path: &std::path::Path) {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(path);
}

/// The file a hung share would hold: a pipe nobody writes holds its reader
/// for ever.
fn pipe(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_that_stops_answering_fails_a_correction_instead_of_holding_it(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("archive");
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let rel = "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3";
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    let (call, talkgroup) = filed(&pool, places, &root, rel, None, at).await;
    pipe(&root.join(rel));
    let mut conn = pool.acquire().await.unwrap();
    talkgroups::rename(&mut conn, talkgroup, "Alpha 2")
        .await
        .unwrap();
    let (job, _) = scannerpulse::data::start_correct(&mut conn, talkgroup, "Alpha 2", "ann")
        .await
        .unwrap();
    let ended = jobs_within(&pool).await;
    // The reader left waiting ends, whatever happened.
    release(&root.join(rel));
    assert!(ended.is_ok(), "the correction waited on the file for ever");
    let job = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(job.state, "failed");
    assert!(job.message.contains("did not answer"), "{}", job.message);
    assert_eq!(rel_of(&pool, call).await, rel);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_that_stops_answering_fails_a_purge_instead_of_holding_it(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let (root, holding) = (dir.path().join("archive"), dir.path().join("holding"));
    std::fs::create_dir_all(&holding).unwrap();
    let places = common::seed_places(&pool, &root, &dir.path().join("in")).await;
    let rel = "2031-03-01/127-Alpha Fire/20310301_120000SYSA.mp3";
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    let (call, talkgroup) = filed(&pool, places, &root, rel, None, at).await;
    pipe(&root.join(rel));
    set_holding(&pool, &holding).await;
    let mut conn = pool.acquire().await.unwrap();
    let (job, _) = scannerpulse::data::start_purge(&mut conn, talkgroup, "Alpha Fire", "ann")
        .await
        .unwrap();
    let ended = jobs_within(&pool).await;
    release(&root.join(rel));
    assert!(ended.is_ok(), "the purge waited on the file for ever");
    let job = scannerpulse::jobs::get(&pool, job).await.unwrap().unwrap();
    assert_eq!(job.state, "failed");
    assert!(job.message.contains("did not answer"), "{}", job.message);
    assert_eq!(rel_of(&pool, call).await, rel);
}
