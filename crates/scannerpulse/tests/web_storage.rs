mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, insert_call, seed_places, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

#[sqlx::test(migrations = "./migrations")]
async fn the_cards_are_counted_again_once_a_move_has_ended(pool: PgPool) {
    let archive = tempfile::tempdir().unwrap();
    let places = seed_places(&pool, archive.path(), std::path::Path::new("/srv/in/live")).await;
    let app = app(&pool);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    let main_calls = |body: serde_json::Value| body["locations"][0]["calls"].as_i64().unwrap();
    insert_call(&pool, places, "2025-03-01/1-A/a.mp3", at, 2000, false).await;
    let first = send(&app, "GET", "/api/admin/storage", &h, None).await;
    assert_eq!(main_calls(first.body), 1);
    insert_call(&pool, places, "2025-03-01/1-A/b.mp3", at, 2000, false).await;
    // Kept: counting reads every call.
    let kept = send(&app, "GET", "/api/admin/storage", &h, None).await;
    assert_eq!(main_calls(kept.body), 1);
    // A move that ended since moved calls from one location to another.
    sqlx::query(
        "INSERT INTO jobs (kind, state, started_by, finished_at) VALUES ('move', 'done', 'test', now())",
    )
    .execute(&pool)
    .await
    .unwrap();
    let again = send(&app, "GET", "/api/admin/storage", &h, None).await;
    assert_eq!(main_calls(again.body), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_adds_lists_and_removes_locations(pool: PgPool) {
    let (archive, cold, other) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let places = seed_places(&pool, archive.path(), std::path::Path::new("/srv/in/live")).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/storage", &[], None)
            .await
            .status,
        401
    );
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let folder = cold.path().to_str().unwrap();
    let added = send(
        &app,
        "POST",
        "/api/admin/storage",
        &h,
        Some(json!({"name": "Cold 2024", "folder": folder})),
    )
    .await;
    assert_eq!(added.status, 201, "{:?}", added.body);
    let id = added.body["id"].as_i64().unwrap();
    assert!(cold.path().join(scannerpulse::storage::MARKER).is_file());
    // A name taken is refused before the folder is touched.
    let taken = send(
        &app,
        "POST",
        "/api/admin/storage",
        &h,
        Some(json!({"name": "Cold 2024", "folder": other.path().to_str().unwrap()})),
    )
    .await;
    assert_eq!(taken.status, 400);
    assert!(!other.path().join(scannerpulse::storage::MARKER).exists());
    let missing = send(
        &app,
        "POST",
        "/api/admin/storage",
        &h,
        Some(json!({"name": "West", "folder": "/no/such/folder"})),
    )
    .await;
    assert_eq!(missing.status, 400);
    assert!(
        missing.body["detail"]
            .as_str()
            .unwrap()
            .contains("does not exist on this machine"),
        "{:?}",
        missing.body
    );
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    insert_call(&pool, places, "2025-03-01/1-A/a.mp3", at, 2000, false).await;
    let listed = send(&app, "GET", "/api/admin/storage", &h, None).await;
    assert_eq!(listed.status, 200);
    let rows: Vec<(&str, bool, i64)> = listed.body["locations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["name"].as_str().unwrap(),
                l["primary"].as_bool().unwrap(),
                l["calls"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(rows, [("primary", true, 1), ("Cold 2024", false, 0)]);
    assert!(listed.body["counted_at"].is_string());
    let primary = listed.body["locations"][0]["id"].as_i64().unwrap();
    let without = send(
        &app,
        "DELETE",
        &format!("/api/admin/storage/{id}"),
        &h[..1],
        None,
    )
    .await;
    assert_eq!(without.status, 403);
    let main = send(
        &app,
        "DELETE",
        &format!("/api/admin/storage/{primary}"),
        &h,
        None,
    )
    .await;
    assert_eq!(main.status, 400);
    let gone = send(
        &app,
        "DELETE",
        &format!("/api/admin/storage/{id}"),
        &h,
        None,
    )
    .await;
    assert_eq!(gone.status, 204);
    let listed = send(&app, "GET", "/api/admin/storage", &h, None).await;
    assert_eq!(listed.body["locations"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_move_is_counted_then_started_only_with_move_typed(pool: PgPool) {
    let (archive, cold) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let places = seed_places(&pool, archive.path(), std::path::Path::new("/srv/in/live")).await;
    let app = app(&pool);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let added = send(
        &app,
        "POST",
        "/api/admin/storage",
        &h,
        Some(json!({"name": "Cold", "folder": cold.path().to_str().unwrap()})),
    )
    .await;
    let cold_id = added.body["id"].as_i64().unwrap() as i16;
    let rel = "2025-03-01/1-A/a.mp3";
    std::fs::create_dir_all(archive.path().join("2025-03-01/1-A")).unwrap();
    std::fs::write(archive.path().join(rel), [7; 100]).unwrap();
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    insert_call(&pool, places, rel, at, 2000, false).await;
    sqlx::query(
        "INSERT INTO leases (name, holder, expires_at, renewed_at) \
         VALUES ('ingest:host-a', 'host-a', now() + interval '1 minute', now())",
    )
    .execute(&pool)
    .await
    .unwrap();
    scannerpulse::storage::record_health(&pool, "host-a", &[(cold_id, Ok((5 << 40, 10 << 40)))])
        .await
        .unwrap();
    let ask = "/api/admin/storage/move?to=Cold&from=2025-03-01&until=2025-03-31";
    let counted = send(&app, "GET", ask, &h, None).await;
    assert_eq!(counted.status, 200, "{:?}", counted.body);
    assert_eq!(counted.body["calls"], 1);
    assert_eq!(counted.body["bytes_about"], 100);
    assert_eq!(counted.body["refused"], serde_json::Value::Null);
    let body = |confirm: &str| {
        Some(json!({"to": "Cold", "from": "2025-03-01", "until": "2025-03-31", "confirm": confirm}))
    };
    let wrong = send(&app, "POST", "/api/admin/storage/move", &h, body("Move")).await;
    assert_eq!(wrong.status, 400);
    assert_eq!(wrong.body["detail"], "type move to confirm");
    let started = send(&app, "POST", "/api/admin/storage/move", &h, body("move")).await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    assert_eq!(started.body["calls"], 1);
    let job = started.body["job"].as_i64().unwrap();
    let (kind, total, by): (String, Option<i64>, String) =
        sqlx::query_as("SELECT kind, total, started_by FROM jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (kind.as_str(), total, by.as_str()),
        ("move", Some(1), "ann")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_purge_is_counted_then_started_only_with_its_date_typed(pool: PgPool) {
    let archive = tempfile::tempdir().unwrap();
    let places = seed_places(&pool, archive.path(), std::path::Path::new("/srv/in/live")).await;
    let app = app(&pool);
    let ask = "/api/admin/storage/purge?before=2025-03-02";
    assert_eq!(send(&app, "GET", ask, &[], None).await.status, 401);
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let rel = "2025-03-01/1-A/a.mp3";
    std::fs::create_dir_all(archive.path().join("2025-03-01/1-A")).unwrap();
    std::fs::write(archive.path().join(rel), [7; 100]).unwrap();
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    insert_call(&pool, places, rel, at, 2000, false).await;
    let counted = send(&app, "GET", ask, &h, None).await;
    assert_eq!(counted.status, 200, "{:?}", counted.body);
    assert_eq!(counted.body["calls"], 1);
    assert_eq!(counted.body["bytes_about"], 100);
    assert_eq!(counted.body["held"][0]["location"], "primary");
    assert_eq!(counted.body["refused"], serde_json::Value::Null);
    let body = |confirm: &str| Some(json!({"before": "2025-03-02", "confirm": confirm}));
    let wrong = send(&app, "POST", "/api/admin/storage/purge", &h, body("purge")).await;
    assert_eq!(wrong.status, 400);
    assert_eq!(wrong.body["detail"], "type 2025-03-02 to confirm");
    let started = send(
        &app,
        "POST",
        "/api/admin/storage/purge",
        &h,
        body("2025-03-02"),
    )
    .await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    assert_eq!(started.body["calls"], 1);
    let job = started.body["job"].as_i64().unwrap();
    let (kind, total, by): (String, Option<i64>, String) =
        sqlx::query_as("SELECT kind, total, started_by FROM jobs WHERE id = $1")
            .bind(job)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        (kind.as_str(), total, by.as_str()),
        ("purge_before", Some(1), "ann")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_import_is_reviewed_started_and_consolidated(pool: PgPool) {
    use id3::TagLike;
    let archive = tempfile::tempdir().unwrap();
    seed_places(&pool, archive.path(), std::path::Path::new("/srv/in/live")).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"America/Chicago\"')")
        .execute(&pool)
        .await
        .unwrap();
    let old = tempfile::tempdir().unwrap();
    let mut tag = id3::Tag::new();
    tag.set_title("702\"Fire\"");
    tag.set_text("TIT1", "Metro");
    std::fs::write(
        old.path().join("20190301_130000_b.mp3"),
        common::tagged(&tag, 100),
    )
    .unwrap();
    let folder = old.path().to_str().unwrap();
    let app = app(&pool);
    let ask = Some(json!({"name": "Old", "folder": folder}));
    let review = "/api/admin/storage/import/review";
    assert_eq!(
        send(&app, "POST", review, &[], ask.clone()).await.status,
        401
    );
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let run = || async {
        let context = scannerpulse::jobs::Context::new(None);
        while scannerpulse::jobs::step(&pool, "host-a", &context)
            .await
            .unwrap()
            .is_some()
        {}
    };
    // Reviewed first: the review writes nothing.
    let started = send(&app, "POST", review, &h, ask).await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    let job = started.body["job"].as_i64().unwrap();
    run().await;
    let found = send(&app, "GET", &format!("{review}/{job}"), &h, None).await;
    assert_eq!(found.status, 200, "{:?}", found.body);
    assert_eq!(
        (found.body["files"].clone(), found.body["new"].clone()),
        (json!(1), json!(1))
    );
    // Imported with an offset, typed as ±HH:MM.
    let import = |offset: &str| Some(json!({"name": "Old", "folder": folder, "offset": offset}));
    let wrong = send(&app, "POST", "/api/admin/storage/import", &h, import("1h")).await;
    assert_eq!(wrong.status, 400);
    assert!(
        wrong.body["detail"]
            .as_str()
            .unwrap()
            .contains("not an offset"),
        "{:?}",
        wrong.body
    );
    let imported = send(
        &app,
        "POST",
        "/api/admin/storage/import",
        &h,
        import("+01:00"),
    )
    .await;
    assert_eq!(imported.status, 201, "{:?}", imported.body);
    assert_eq!(imported.body["offset"], 60);
    assert_eq!(imported.body["again"], false);
    run().await;
    // Its card says it is read-only and with which offset, and counts its call.
    let listed = send(&app, "GET", "/api/admin/storage", &h, None).await;
    let card = &listed.body["locations"][1];
    assert_eq!(
        (
            &card["name"],
            &card["mode"],
            &card["offset_minutes"],
            &card["calls"]
        ),
        (&json!("Old"), &json!("readonly"), &json!(60), &json!(1))
    );
    // Consolidated only once counted and with move typed.
    let counted = send(
        &app,
        "GET",
        "/api/admin/storage/consolidate?name=Old",
        &h,
        None,
    )
    .await;
    assert_eq!(counted.status, 200, "{:?}", counted.body);
    assert_eq!(
        (&counted.body["calls"], &counted.body["refused"]),
        (&json!(1), &serde_json::Value::Null)
    );
    let body = |confirm: &str| Some(json!({"name": "Old", "confirm": confirm}));
    let wrong = send(
        &app,
        "POST",
        "/api/admin/storage/consolidate",
        &h,
        body("yes"),
    )
    .await;
    assert_eq!(wrong.status, 400);
    assert_eq!(wrong.body["detail"], "type move to confirm");
    let started = send(
        &app,
        "POST",
        "/api/admin/storage/consolidate",
        &h,
        body("move"),
    )
    .await;
    assert_eq!(started.status, 201, "{:?}", started.body);
    assert_eq!(started.body["calls"], 1);
    let job = started.body["job"].as_i64().unwrap();
    let kind: String = sqlx::query_scalar("SELECT kind FROM jobs WHERE id = $1")
        .bind(job)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kind, "consolidate");
}
