mod common;

use common::{admin, app, send, sign_in};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn try_sign_in(app: &axum::Router, login: &str, password: &str) -> common::Reply {
    send(
        app,
        "POST",
        "/api/auth/login",
        &[],
        Some(json!({"login": login, "password": password})),
    )
    .await
}

async fn failures(pool: &PgPool, kind: &str) -> Vec<String> {
    sqlx::query_scalar("SELECT value FROM sign_in_failures WHERE kind = $1 ORDER BY at")
        .bind(kind)
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn signing_in_sets_a_session_cookie(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let answer = try_sign_in(&app, "ann", PASSWORD).await;
    assert_eq!(answer.status, 200);
    let cookie = answer.headers["set-cookie"].to_str().unwrap();
    for part in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(cookie.contains(part), "{cookie}");
    }
    let token = cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("scannerpulse_session=")
        .unwrap();
    assert_eq!(token.len(), 64);
    // Only the token's SHA-256 is kept.
    let kept: Vec<u8> = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let digest: Vec<u8> = sqlx::query_scalar("SELECT sha256(convert_to($1, 'UTF8'))")
        .bind(token)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, digest);
    let csrf = answer.body["csrf"].as_str().unwrap();
    let pair = format!("scannerpulse_session={token}");
    let me = send(&app, "GET", "/api/auth/me", &[("cookie", &pair)], None).await;
    assert_eq!(me.status, 200);
    assert_eq!(me.body, json!({"login": "ann", "csrf": csrf}));
    let last: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT last_sign_in FROM admins")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(last.is_some());
}

#[sqlx::test(migrations = "./migrations")]
async fn a_wrong_password_and_an_unknown_name_answer_alike(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let wrong = try_sign_in(&app, "ann", "not the password at all").await;
    let unknown = try_sign_in(&app, "nobody", PASSWORD).await;
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.body, unknown.body);
    assert!(wrong.headers.get("set-cookie").is_none());
    assert_eq!(failures(&pool, "name").await, ["ann", "nobody"]);
    assert_eq!(failures(&pool, "address").await, ["198.51.100.7"; 2]);
    let me = send(&app, "GET", "/api/auth/me", &[], None).await;
    assert_eq!(me.status, 401);
}

#[sqlx::test(migrations = "./migrations")]
async fn five_failures_lock_the_name_for_fifteen_minutes(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    for _ in 0..4 {
        assert_eq!(
            try_sign_in(&app, "ann", "wrong password one").await.status,
            401
        );
    }
    // Four failures lock nothing.
    sign_in(&app, "ann", PASSWORD).await;
    assert_eq!(
        try_sign_in(&app, "ann", "wrong password one").await.status,
        401
    );
    // The fifth locks the name: even the right password is refused, and a
    // locked attempt is not counted.
    let locked = try_sign_in(&app, "ann", PASSWORD).await;
    assert_eq!(locked.status, 429, "{:?}", locked.body);
    assert_eq!(failures(&pool, "name").await.len(), 5);
    // Fifteen minutes on, the name is free again.
    sqlx::query("UPDATE sign_in_failures SET at = at - interval '15 minutes 1 second'")
        .execute(&pool)
        .await
        .unwrap();
    sign_in(&app, "ann", PASSWORD).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn failures_spread_over_more_than_fifteen_minutes_lock_nothing(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    sqlx::query(
        "INSERT INTO sign_in_failures (kind, value, at) \
         SELECT 'name', 'ann', now() - n * interval '4 minutes' FROM generate_series(0, 4) n",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Five failures over 16 minutes: no five within 15.
    sign_in(&app(&pool), "ann", PASSWORD).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn five_failures_from_one_address_lock_the_address(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    for name in ["a1", "a2", "a3", "a4", "a5"] {
        assert_eq!(try_sign_in(&app, name, PASSWORD).await.status, 401);
    }
    assert_eq!(try_sign_in(&app, "ann", PASSWORD).await.status, 429);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_address_behind_a_trusted_proxy_is_the_forwarded_one(pool: PgPool) {
    let app = app(&pool);
    let forwarded = [("x-forwarded-for", "203.0.113.9")];
    let login = Some(json!({"login": "nobody", "password": PASSWORD}));
    send(&app, "POST", "/api/auth/login", &forwarded, login.clone()).await;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('web.trusted_proxies', '[\"198.51.100.7\"]')",
    )
    .execute(&pool)
    .await
    .unwrap();
    send(&app, "POST", "/api/auth/login", &forwarded, login).await;
    assert_eq!(
        failures(&pool, "address").await,
        ["198.51.100.7", "203.0.113.9"],
        "the header is believed only from the proxy"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_change_needs_the_sessions_csrf_token(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let new = Some(json!({"login": "bob", "password": PASSWORD}));
    let none = send(&app, "POST", "/api/admin/accounts", &[], new.clone()).await;
    assert_eq!(none.status, 401);
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let without = send(
        &app,
        "POST",
        "/api/admin/accounts",
        &[("cookie", &cookie)],
        new.clone(),
    )
    .await;
    assert_eq!(without.status, 403);
    let wrong = [("cookie", cookie.as_str()), ("x-csrf-token", "0123")];
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &wrong, new.clone())
            .await
            .status,
        403
    );
    let right = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let added = send(&app, "POST", "/api/admin/accounts", &right, new).await;
    assert_eq!(added.status, 201, "{:?}", added.body);
    // Reading needs the session but not the token.
    let list = send(
        &app,
        "GET",
        "/api/admin/accounts",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(list.status, 200);
    let logins: Vec<&str> = list
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["login"].as_str().unwrap())
        .collect();
    assert_eq!(logins, ["ann", "bob"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_accounts_routes_refuse_what_the_command_refuses(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let last = send(&app, "POST", "/api/admin/accounts/ann/disable", &h, None).await;
    assert_eq!(last.status, 409, "{:?}", last.body);
    let short = Some(json!({"login": "bob", "password": "short"}));
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &h, short)
            .await
            .status,
        400
    );
    let twice = Some(json!({"login": "ann", "password": PASSWORD}));
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &h, twice)
            .await
            .status,
        409
    );
    let unknown = send(&app, "POST", "/api/admin/accounts/cy/enable", &h, None).await;
    assert_eq!(unknown.status, 404);
    // A new password ends the account's sessions, the admin's own included.
    let new = Some(json!({"password": "a different long one"}));
    let changed = send(&app, "POST", "/api/admin/accounts/ann/password", &h, new).await;
    assert_eq!(changed.status, 204);
    assert_eq!(
        send(&app, "GET", "/api/auth/me", &[("cookie", &cookie)], None)
            .await
            .status,
        401
    );
    sign_in(&app, "ann", "a different long one").await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_session_ends_when_idle_too_old_or_disabled(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    admin(&pool, "bob", PASSWORD).await;
    let app = app(&pool);
    let me = |cookie: String| {
        let app = app.clone();
        async move {
            send(&app, "GET", "/api/auth/me", &[("cookie", &cookie)], None)
                .await
                .status
        }
    };
    let (idle, _) = sign_in(&app, "ann", PASSWORD).await;
    sqlx::query("UPDATE sessions SET last_seen = now() - interval '12 hours 1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(me(idle).await, 401);
    let (old, _) = sign_in(&app, "ann", PASSWORD).await;
    sqlx::query("UPDATE sessions SET created_at = now() - interval '7 days 1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(me(old).await, 401);
    // A request keeps the session going.
    let (used, _) = sign_in(&app, "ann", PASSWORD).await;
    sqlx::query("UPDATE sessions SET last_seen = now() - interval '11 hours'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(me(used.clone()).await, 200);
    let seen: bool = sqlx::query_scalar(
        "SELECT bool_and(last_seen > now() - interval '1 minute') FROM sessions \
         WHERE created_at > now() - interval '1 day'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(seen);
    sqlx::query("UPDATE admins SET disabled = true WHERE login = 'ann'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        me(used).await,
        401,
        "a disabled account's session no longer works"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn signing_out_ends_the_session(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let out = send(
        &app,
        "POST",
        "/api/auth/logout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(out.status, 204);
    assert!(
        out.headers["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(
        send(&app, "GET", "/api/auth/me", &[("cookie", &cookie)], None)
            .await
            .status,
        401
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn password_sign_in_can_be_switched_off(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/auth/methods", &[], None).await.body["password"],
        true
    );
    scannerpulse::accounts::set_password_signin(&pool, false)
        .await
        .unwrap();
    assert_eq!(try_sign_in(&app, "ann", PASSWORD).await.status, 403);
    assert_eq!(
        send(&app, "GET", "/api/auth/methods", &[], None).await.body["password"],
        false
    );
    assert!(
        failures(&pool, "name").await.is_empty(),
        "a refused sign-in is not a failure"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_sees_the_labelers_list_when_it_is_not_public(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    let app = app(&pool);
    let visitor = send(&app, "GET", "/api/labels/todo?set=s1", &[], None).await;
    assert_eq!(visitor.status, 403);
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    // Past the gate: the set does not exist.
    let admin = send(
        &app,
        "GET",
        "/api/labels/todo?set=s1",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(admin.status, 404);
}

#[sqlx::test(migrations = "./migrations")]
async fn parallel_guesses_check_at_most_five_passwords(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    // A wide hashing limit, so that only the per-name lock can hold the
    // guesses back.
    let mut state =
        scannerpulse::web::AppState::new(pool.clone(), (*pool.connect_options()).clone());
    state.sign_ins = std::sync::Arc::new(tokio::sync::Semaphore::new(100));
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    let guesses: Vec<_> = (0..20)
        .map(|n| {
            let app = app.clone();
            tokio::spawn(async move {
                try_sign_in(&app, "ann", &format!("wrong password {n:02}"))
                    .await
                    .status
            })
        })
        .collect();
    let mut checked = 0;
    for guess in guesses {
        let status = guess.await.unwrap();
        assert!(status == 401 || status == 429, "{status}");
        if status == 401 {
            checked += 1;
        }
    }
    assert!(checked <= 5, "{checked} passwords were checked");
    let counted: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sign_in_failures WHERE kind = 'name' AND value = 'ann'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counted, checked, "every checked password is counted");
}

#[sqlx::test(migrations = "./migrations")]
async fn a_password_session_outlives_twelve_hours_while_used(pool: PgPool) {
    admin(&pool, "ann", "correct horse battery").await;
    let app = app(&pool);
    let (cookie, _) = sign_in(&app, "ann", "correct horse battery").await;
    // Signed in 12 hours and a minute ago, and used just now: still open.
    sqlx::query(
        "UPDATE sessions SET created_at = now() - interval '12 hours 1 minute', last_seen = now() \
         WHERE NOT oidc",
    )
    .execute(&pool)
    .await
    .unwrap();
    let me = send(&app, "GET", "/api/auth/me", &[("cookie", &cookie)], None).await;
    assert_eq!(me.status, 200, "{:?}", me.body);
}
