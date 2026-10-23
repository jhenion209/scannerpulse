mod common;

use common::{PEER, admin, app, send, sign_in};
use scannerpulse::accounts;
use scannerpulse::roles::Role;
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn audited(pool: &PgPool) -> Vec<(String, Option<String>, String)> {
    sqlx::query_as("SELECT actor, address, what FROM audit_log ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn each_change_an_admin_makes_is_recorded_with_who_and_from_where(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let mut conn = pool.acquire().await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    let app = app(&pool);
    // A failed sign-in goes to the journal only.
    let wrong = json!({"login": "ann", "password": "not the password"});
    assert_eq!(
        send(&app, "POST", "/api/auth/login", &[], Some(wrong))
            .await
            .status,
        401
    );
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let order = Some(json!({"value": ["B", "A"]}));
    let set = send(
        &app,
        "PUT",
        "/api/admin/settings/web.system_order",
        &h,
        order,
    )
    .await;
    assert_eq!(set.status, 204, "{:?}", set.body);
    let unset = send(
        &app,
        "DELETE",
        "/api/admin/settings/web.system_order",
        &h,
        None,
    )
    .await;
    assert_eq!(unset.status, 204);
    let on = send(
        &app,
        "PUT",
        "/api/admin/accounts/cy/functions/move",
        &h,
        None,
    )
    .await;
    assert_eq!(on.status, 204);
    let renewed = Some(json!({"password": "a brand new password"}));
    let password = send(&app, "POST", "/api/admin/accounts/cy/password", &h, renewed).await;
    assert_eq!(password.status, 204);
    assert_eq!(
        send(
            &app,
            "POST",
            "/api/auth/logout",
            &[("cookie", &cookie)],
            None
        )
        .await
        .status,
        204
    );
    let from = Some(std::net::IpAddr::from(PEER).to_string());
    let entries = audited(&pool).await;
    let expected = [
        ("ann", "signed in with a password"),
        ("ann", "set web.system_order from nothing to [\"B\",\"A\"]"),
        ("ann", "removed web.system_order (it was [\"B\",\"A\"])"),
        ("ann", "turned move on for cy"),
        ("ann", "gave cy a new password"),
        ("ann", "signed out"),
    ];
    assert_eq!(entries.len(), expected.len(), "{entries:?}");
    for (entry, (actor, what)) in entries.iter().zip(expected) {
        assert_eq!((entry.0.as_str(), entry.2.as_str()), (actor, what));
        assert_eq!(entry.1, from, "{entry:?}");
    }
    assert!(
        !entries
            .iter()
            .any(|e| e.2.contains("brand new") || e.2.contains(PASSWORD)),
        "no entry holds a password"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_refused_change_is_not_recorded(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let mut conn = pool.acquire().await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    let app = app(&pool);
    let (cookie, csrf) = sign_in(&app, "cy", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let refused = send(&app, "POST", "/api/admin/rescan", &h, None).await;
    assert_eq!(refused.status, 403);
    let whats: Vec<String> = audited(&pool).await.into_iter().map(|e| e.2).collect();
    assert_eq!(whats, ["signed in with a password"]);
}
