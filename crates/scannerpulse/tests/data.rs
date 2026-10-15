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
async fn the_recorders_new_name_marks_the_files_too(pool: PgPool) {
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
    scannerpulse::store::talkgroup_id(&mut conn, system, &seen)
        .await
        .unwrap();
    assert_eq!(
        names(&pool, seven).await,
        ("Group Seven".into(), Some("Group 7".into()), false)
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
