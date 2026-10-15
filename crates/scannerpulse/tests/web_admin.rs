mod common;

use common::{admin, app, send, sign_in};
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
    assert_eq!(list.body[0]["key"], "time_zone");
    assert_eq!(list.body[0]["value"], "UTC");
    assert!(list.body[0]["updated_at"].is_string());
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
