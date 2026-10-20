mod common;

use common::{admin, app, seed_places, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_adds_lists_and_changes_recorders(pool: PgPool) {
    let (archive, live, east) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    seed_places(&pool, archive.path(), live.path()).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/recorders", &[], None)
            .await
            .status,
        401
    );
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let folder = east.path().to_str().unwrap();
    let added = send(
        &app,
        "POST",
        "/api/admin/recorders",
        &h,
        Some(json!({"name": "east", "folder": folder, "when_asked": true})),
    )
    .await;
    assert_eq!(added.status, 201, "{:?}", added.body);
    let id = added.body["id"].as_i64().unwrap();
    let bad = send(
        &app,
        "POST",
        "/api/admin/recorders",
        &h,
        Some(json!({"name": "west", "folder": "/no/such/folder"})),
    )
    .await;
    assert_eq!(bad.status, 400);
    assert!(
        bad.body["detail"]
            .as_str()
            .unwrap()
            .contains("does not exist on this machine"),
        "{:?}",
        bad.body
    );
    let listed = send(&app, "GET", "/api/admin/recorders", &h, None).await;
    assert_eq!(listed.status, 200);
    let names: Vec<(&str, &str)> = listed.body["recorders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap(), r["state"].as_str().unwrap()))
        .collect();
    assert_eq!(names, [("live", "watched"), ("east", "asked")]);
    assert_eq!(listed.body["hosts"], json!([]));
    let bring = send(
        &app,
        "POST",
        &format!("/api/admin/recorders/{id}/bring-in"),
        &h,
        None,
    )
    .await;
    assert_eq!(bring.status, 204);
    let pause = send(
        &app,
        "POST",
        &format!("/api/admin/recorders/{id}/pause"),
        &h,
        None,
    )
    .await;
    assert_eq!(
        pause.status, 400,
        "a recorder taken in when asked has nothing to pause"
    );
    let unknown = send(
        &app,
        "POST",
        &format!("/api/admin/recorders/{id}/delete"),
        &h,
        None,
    )
    .await;
    assert_eq!(unknown.status, 404);
    let removed = send(
        &app,
        "POST",
        &format!("/api/admin/recorders/{id}/remove"),
        &h,
        None,
    )
    .await;
    assert_eq!(removed.status, 204);
    let gone = send(
        &app,
        "POST",
        &format!("/api/admin/recorders/{id}/watch"),
        &h,
        None,
    )
    .await;
    assert_eq!(gone.status, 404);
    let listed = send(&app, "GET", "/api/admin/recorders", &h, None).await;
    assert_eq!(listed.body["recorders"].as_array().unwrap().len(), 1);
}
