mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, insert_call, seed_places, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

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
