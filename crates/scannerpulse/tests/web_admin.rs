mod common;

use common::{admin, app, send, sign_in, talkgroup};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

/// A signed-in admin's cookie and CSRF token.
async fn signed(pool: &PgPool, app: &axum::Router) -> (String, String) {
    admin(pool, "ann", PASSWORD).await;
    sign_in(app, "ann", PASSWORD).await
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_changes_settings_through_their_checks(pool: PgPool) {
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/settings", &[], None)
            .await
            .status,
        401
    );
    let (cookie, csrf) = signed(&pool, &app).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let bad = send(
        &app,
        "PUT",
        "/api/admin/settings/time_zone",
        &h,
        Some(json!({"value": "Nowhere"})),
    )
    .await;
    assert_eq!(bad.status, 400, "{:?}", bad.body);
    assert!(bad.body["detail"].as_str().unwrap().contains("time_zone"));
    let good = send(
        &app,
        "PUT",
        "/api/admin/settings/time_zone",
        &h,
        Some(json!({"value": "UTC"})),
    )
    .await;
    assert_eq!(good.status, 204);
    let unknown = send(
        &app,
        "PUT",
        "/api/admin/settings/no.such",
        &h,
        Some(json!({"value": 1})),
    )
    .await;
    assert_eq!(unknown.status, 404);
    let list = send(
        &app,
        "GET",
        "/api/admin/settings",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(list.status, 200);
    let row = |key: &str| {
        list.body
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == key)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {key}: {:?}", list.body))
    };
    assert_eq!(row("time_zone")["value"], "UTC");
    assert!(row("time_zone")["updated_at"].is_string());
    assert_eq!(row("time_zone")["known"], true);
    // Every setting a part reads is listed, stored or not.
    assert_eq!(
        list.body.as_array().unwrap().len(),
        scannerpulse::settings::known().count()
    );
    assert!(row("oidc.issuer")["value"].is_null());
    assert!(row("oidc.issuer")["updated_at"].is_null());
    // A row left by an older version is listed, to be removed.
    sqlx::query("INSERT INTO settings (key, value) VALUES ('old.key', '1')")
        .execute(&pool)
        .await
        .unwrap();
    let list = send(
        &app,
        "GET",
        "/api/admin/settings",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    let old = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["key"] == "old.key")
        .unwrap();
    assert_eq!(old["known"], false);
    assert_eq!(old["value"], 1);
    // Removing it is refused: ingest cannot run without a zone.
    let removed = send(&app, "DELETE", "/api/admin/settings/time_zone", &h, None).await;
    assert_eq!(removed.status, 400);
    send(
        &app,
        "PUT",
        "/api/admin/settings/maintenance.enabled",
        &h,
        Some(json!({"value": true})),
    )
    .await;
    let removed = send(
        &app,
        "DELETE",
        "/api/admin/settings/maintenance.enabled",
        &h,
        None,
    )
    .await;
    assert_eq!(removed.status, 204);
    // A change needs the CSRF token.
    let without = send(
        &app,
        "PUT",
        "/api/admin/settings/time_zone",
        &[("cookie", &cookie)],
        Some(json!({"value": "UTC"})),
    )
    .await;
    assert_eq!(without.status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_sets_and_removes_favorite_lists(pool: PgPool) {
    let one = common::talkgroup(&pool, "SYSA", 1).await;
    let two = common::talkgroup(&pool, "SYSA", 2).await;
    let app = app(&pool);
    let lists = || {
        let app = app.clone();
        async move { send(&app, "GET", "/api/favorites", &[], None).await.body }
    };
    let body = Some(json!({"talkgroups": [two, one, two]}));
    let visitor = send(&app, "PUT", "/api/admin/favorites/Fire", &[], body.clone()).await;
    assert_eq!(visitor.status, 401);
    let (cookie, csrf) = signed(&pool, &app).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    assert_eq!(
        send(&app, "PUT", "/api/admin/favorites/Fire", &h, body)
            .await
            .status,
        204
    );
    let mut ids = [one, two];
    ids.sort();
    assert_eq!(
        lists().await,
        json!([{"id": lists().await[0]["id"], "name": "Fire", "talkgroups": ids}])
    );
    // Setting it again replaces its talkgroups.
    let only = Some(json!({"talkgroups": [one]}));
    send(&app, "PUT", "/api/admin/favorites/Fire", &h, only).await;
    assert_eq!(lists().await[0]["talkgroups"], json!([one]));
    let unknown = send(
        &app,
        "PUT",
        "/api/admin/favorites/Fire",
        &h,
        Some(json!({"talkgroups": [one, 999_999]})),
    )
    .await;
    assert_eq!(unknown.status, 400);
    assert!(unknown.body["detail"].as_str().unwrap().contains("999999"));
    assert_eq!(
        lists().await[0]["talkgroups"],
        json!([one]),
        "a refused change keeps the list"
    );
    let spaced = send(
        &app,
        "PUT",
        "/api/admin/favorites/%20Fire",
        &h,
        Some(json!({"talkgroups": []})),
    )
    .await;
    assert_eq!(spaced.status, 400);
    assert_eq!(
        send(&app, "DELETE", "/api/admin/favorites/Fire", &h, None)
            .await
            .status,
        204
    );
    assert_eq!(lists().await, json!([]));
    assert_eq!(
        send(&app, "DELETE", "/api/admin/favorites/Fire", &h, None)
            .await
            .status,
        404
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_system_order_shows_in_the_filter_choices_at_once(pool: PgPool) {
    let app = app(&pool);
    talkgroup(&pool, "ALPHA", 1).await;
    talkgroup(&pool, "BRAVO", 2).await;
    let systems = |body: &serde_json::Value| -> Vec<String> {
        body["systems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    // The choices are read once, and kept.
    let before = send(&app, "GET", "/api/filters", &[], None).await;
    assert_eq!(systems(&before.body), ["ALPHA", "BRAVO"]);
    let (cookie, csrf) = signed(&pool, &app).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let saved = send(
        &app,
        "PUT",
        "/api/admin/settings/web.system_order",
        &h,
        Some(json!({"value": ["BRAVO"]})),
    )
    .await;
    assert_eq!(saved.status, 204);
    let after = send(&app, "GET", "/api/filters", &[], None).await;
    assert_eq!(
        systems(&after.body),
        ["BRAVO", "ALPHA"],
        "this host's kept choices are dropped"
    );
    let removed = send(
        &app,
        "DELETE",
        "/api/admin/settings/web.system_order",
        &h,
        None,
    )
    .await;
    assert_eq!(removed.status, 204);
    let again = send(&app, "GET", "/api/filters", &[], None).await;
    assert_eq!(systems(&again.body), ["ALPHA", "BRAVO"]);
}
