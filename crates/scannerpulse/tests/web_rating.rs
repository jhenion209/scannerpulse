mod common;

use chrono::{TimeZone, Utc};
use common::{app, send};
use serde_json::json;
use sqlx::PgPool;

async fn machine_call(pool: &PgPool) -> i64 {
    let talkgroup = common::talkgroup(pool, "SYSA", 127).await;
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    common::call(pool, talkgroup, at, Some("engine for responding")).await
}

fn uri(id: i64) -> String {
    format!("/api/calls/{id}/rating")
}

async fn rows(pool: &PgPool) -> Vec<(i16, String)> {
    sqlx::query_as("SELECT stars, rater FROM transcript_ratings ORDER BY rater")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// The `name=value` part of the answer's `Set-Cookie`.
fn cookie_of(reply: &common::Reply) -> String {
    let set = reply.headers.get("set-cookie").unwrap().to_str().unwrap();
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax"),
        "{set}"
    );
    set.split(';').next().unwrap().to_string()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_browser_rates_changes_and_clears(pool: PgPool) {
    let id = machine_call(&pool).await;
    let app = app(&pool);
    let first = send(&app, "PUT", &uri(id), &[], Some(json!({"stars": 4}))).await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({"id": id, "stars": 4}));
    let cookie = cookie_of(&first);
    assert!(cookie.starts_with("sp_rater="));

    // The same browser changes its rating; it does not add one.
    let again = send(
        &app,
        "PUT",
        &uri(id),
        &[("cookie", &cookie)],
        Some(json!({"stars": 2})),
    )
    .await;
    assert_eq!(again.status, 200);
    let stored = rows(&pool).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].0, 2);
    // Only a hash of the token is stored.
    assert!(!cookie.contains(&stored[0].1));

    // A browser with no cookie is another rater.
    send(&app, "PUT", &uri(id), &[], Some(json!({"stars": 5}))).await;
    assert_eq!(rows(&pool).await.len(), 2);

    let cleared = send(&app, "DELETE", &uri(id), &[("cookie", &cookie)], None).await;
    assert_eq!(cleared.status, 200);
    assert_eq!(cleared.body, json!({"id": id, "stars": null}));
    assert_eq!(rows(&pool).await.len(), 1);
    // Clearing with no rating, or with no cookie, is not an error.
    assert_eq!(
        send(&app, "DELETE", &uri(id), &[("cookie", &cookie)], None)
            .await
            .status,
        200
    );
    assert_eq!(send(&app, "DELETE", &uri(id), &[], None).await.status, 200);
}

#[sqlx::test(migrations = "./migrations")]
async fn stars_must_be_one_to_five(pool: PgPool) {
    let id = machine_call(&pool).await;
    let app = app(&pool);
    for bad in [0, 6, -3] {
        let answer = send(&app, "PUT", &uri(id), &[], Some(json!({"stars": bad}))).await;
        assert_eq!(answer.status, 400, "stars {bad}");
    }
    assert!(rows(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_call_or_one_without_text_is_not_found(pool: PgPool) {
    let app = app(&pool);
    assert_eq!(
        send(&app, "PUT", &uri(99), &[], Some(json!({"stars": 3})))
            .await
            .status,
        404
    );
    let talkgroup = common::talkgroup(&pool, "SYSA", 127).await;
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    let silent = common::call(&pool, talkgroup, at, None).await;
    assert_eq!(
        send(&app, "PUT", &uri(silent), &[], Some(json!({"stars": 3})))
            .await
            .status,
        404
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_malformed_cookie_is_replaced_not_trusted(pool: PgPool) {
    let id = machine_call(&pool).await;
    let app = app(&pool);
    let answer = send(
        &app,
        "PUT",
        &uri(id),
        &[("cookie", "sp_rater=short")],
        Some(json!({"stars": 3})),
    )
    .await;
    assert_eq!(answer.status, 200);
    let fresh = cookie_of(&answer);
    assert_ne!(fresh, "sp_rater=short");
    assert_eq!(fresh.len(), "sp_rater=".len() + 32);
}

#[sqlx::test(migrations = "./migrations")]
async fn maintenance_pauses_ratings(pool: PgPool) {
    let id = machine_call(&pool).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('maintenance.enabled', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    let app = app(&pool);
    let put = send(&app, "PUT", &uri(id), &[], Some(json!({"stars": 3}))).await;
    assert_eq!(put.status, 503);
    assert_eq!(send(&app, "DELETE", &uri(id), &[], None).await.status, 503);
    assert!(rows(&pool).await.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn sixty_ratings_a_minute_per_address(pool: PgPool) {
    let id = machine_call(&pool).await;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('web.trusted_proxies', '[\"198.51.100.7\"]')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let app = app(&pool);
    let from = |ip: &'static str| [("x-forwarded-for", ip)];
    for n in 0..60 {
        let answer = send(
            &app,
            "PUT",
            &uri(id),
            &from("203.0.113.9"),
            Some(json!({"stars": 3})),
        )
        .await;
        assert_eq!(answer.status, 200, "rating {n}");
    }
    let over = send(
        &app,
        "PUT",
        &uri(id),
        &from("203.0.113.9"),
        Some(json!({"stars": 3})),
    )
    .await;
    assert_eq!(over.status, 429);
    let other = send(
        &app,
        "PUT",
        &uri(id),
        &from("203.0.113.10"),
        Some(json!({"stars": 3})),
    )
    .await;
    assert_eq!(other.status, 200);
}
