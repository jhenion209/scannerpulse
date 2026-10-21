mod common;

use common::{admin, app, seed_places, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

/// Signs `ann` in; the headers a change needs.
async fn signed_in(pool: &PgPool, app: &axum::Router) -> (String, String) {
    admin(pool, "ann", PASSWORD).await;
    sign_in(app, "ann", PASSWORD).await
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_approves_and_dismisses_name_proposals(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    for name in ["Group Seven", "Grp 7"] {
        sqlx::query("INSERT INTO name_proposals (talkgroup_id, name, calls) VALUES ($1, $2, 1)")
            .bind(seven)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
    }
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/names", &[], None)
            .await
            .status,
        401
    );
    let (cookie, csrf) = signed_in(&pool, &app).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let listed = send(&app, "GET", "/api/admin/names", &h, None).await;
    assert_eq!(listed.status, 200);
    assert_eq!(listed.body["waiting"], 2);
    assert_eq!(listed.body["unknown"], json!([]));
    let proposed: Vec<&str> = listed.body["proposals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["proposed"].as_str().unwrap())
        .collect();
    assert_eq!(proposed, ["Group Seven", "Grp 7"]);
    assert_eq!(listed.body["proposals"][0]["current"], "Group 7");

    let approve = json!({"talkgroup_id": seven, "name": "Group Seven"});
    let done = send(
        &app,
        "POST",
        "/api/admin/names/approve",
        &h,
        Some(approve.clone()),
    )
    .await;
    assert_eq!(done.status, 204, "{:?}", done.body);
    let again = send(&app, "POST", "/api/admin/names/approve", &h, Some(approve)).await;
    assert_eq!(again.status, 404);
    let dismiss = json!({"talkgroup_id": seven, "name": "Grp 7"});
    let done = send(&app, "POST", "/api/admin/names/dismiss", &h, Some(dismiss)).await;
    assert_eq!(done.status, 204, "{:?}", done.body);
    let listed = send(&app, "GET", "/api/admin/names", &h, None).await;
    assert_eq!(listed.body["waiting"], 0);
    let name: String = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(seven)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Group Seven");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_decides_unknown_system_names(pool: PgPool) {
    let (archive, live) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_, source) = seed_places(&pool, archive.path(), live.path()).await;
    common::talkgroup(&pool, "SYSA", 7).await;
    for system in ["SYSB", "SYSC", "SYSD"] {
        sqlx::query(
            "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, staged_system, staged_talkgroup) \
             VALUES ($1, $2 || '.mp3', now(), now(), 'staged', $2, '7 Group 7')",
        )
        .bind(source)
        .bind(system)
        .execute(&pool)
        .await
        .unwrap();
    }
    let app = app(&pool);
    let (cookie, csrf) = signed_in(&pool, &app).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let listed = send(&app, "GET", "/api/admin/names", &h, None).await;
    assert_eq!(listed.body["waiting"], 3);
    assert_eq!(
        listed.body["systems"],
        json!(["SYSA"]),
        "the systems a name may be the same as"
    );
    assert_eq!(listed.body["unknown"][0]["system"], "SYSB");
    assert_eq!(listed.body["unknown"][0]["recorders"], json!(["live"]));

    let post = async |what: &str, body: serde_json::Value| {
        send(
            &app,
            "POST",
            &format!("/api/admin/staging/{what}"),
            &h,
            Some(body),
        )
        .await
    };
    assert_eq!(
        post("new-system", json!({"system": "SYSB"})).await.status,
        204
    );
    assert_eq!(
        post("new-system", json!({"system": "SYSB"})).await.status,
        404
    );
    assert_eq!(
        post("map", json!({"system": "SYSC", "to": "SYSZ"}))
            .await
            .status,
        400
    );
    assert_eq!(
        post("map", json!({"system": "SYSC", "to": "SYSA"}))
            .await
            .status,
        204
    );
    assert_eq!(
        post("discard", json!({"system": "SYSD", "confirm": "sysd"}))
            .await
            .status,
        400
    );
    let discarded = post("discard", json!({"system": "SYSD", "confirm": "SYSD"})).await;
    assert_eq!(discarded.status, 200, "{:?}", discarded.body);
    assert_eq!(discarded.body["discarded"], 1);
    assert_eq!(post("unknown", json!({"system": "SYSD"})).await.status, 404);
    let listed = send(&app, "GET", "/api/admin/names", &h, None).await;
    assert_eq!(listed.body["unknown"], json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_previews_a_recorder(pool: PgPool) {
    let (archive, live) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_, source) = seed_places(&pool, archive.path(), live.path()).await;
    std::fs::write(live.path().join("20310115_120000.mp3"), common::frames(3)).unwrap();
    let app = app(&pool);
    let uri = format!("/api/admin/recorders/{source}/preview");
    assert_eq!(send(&app, "GET", &uri, &[], None).await.status, 401);
    let (cookie, _) = signed_in(&pool, &app).await;
    let h = [("cookie", cookie.as_str())];
    let seen = send(&app, "GET", &uri, &h, None).await;
    assert_eq!(seen.status, 200, "{:?}", seen.body);
    assert_eq!(seen.body["recorder"], "live");
    assert_eq!(seen.body["files"], 1);
    assert_eq!(seen.body["counts"]["filed"], 1);
    assert_eq!(seen.body["sample"][0]["verdict"]["kind"], "filed");
    assert_eq!(
        send(&app, "GET", "/api/admin/recorders/99/preview", &h, None)
            .await
            .status,
        404
    );
    assert!(std::fs::read_dir(live.path()).unwrap().count() == 1);
}
