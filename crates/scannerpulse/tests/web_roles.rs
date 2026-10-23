mod common;

use common::{admin, app, send, sign_in};
use scannerpulse::accounts;
use scannerpulse::roles::{Function, Role};
use serde_json::{Value, json};
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

/// The super admin ann and the associate cy, with `functions` on; cy's
/// cookie and CSRF token.
async fn associate(pool: &PgPool, functions: &[Function]) -> (axum::Router, String, String) {
    admin(pool, "ann", PASSWORD).await;
    let mut conn = pool.acquire().await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    accounts::switch(pool, "cy", functions, true).await.unwrap();
    let app = app(pool);
    let (cookie, csrf) = sign_in(&app, "cy", PASSWORD).await;
    (app, cookie, csrf)
}

/// Every change an associate may be given, as one request each, with the
/// function it needs. A body the handler then refuses is fine: only a 403
/// says the function was missing.
fn changes() -> Vec<(&'static str, &'static str, Option<Value>, Function)> {
    use Function as F;
    vec![
        ("POST", "/api/admin/rescan", None, F::Rescan),
        (
            "PUT",
            "/api/admin/talkgroups/1/transcribe",
            Some(json!({"on": true})),
            F::Transcribe,
        ),
        ("POST", "/api/admin/queue/retry", None, F::Queue),
        (
            "POST",
            "/api/admin/recorders",
            Some(json!({"name": "x", "folder": "/srv/x"})),
            F::Recorders,
        ),
        ("POST", "/api/admin/recorders/1/pause", None, F::Recorders),
        (
            "POST",
            "/api/admin/names/approve",
            Some(json!({})),
            F::Names,
        ),
        (
            "POST",
            "/api/admin/staging/discard",
            Some(json!({})),
            F::Names,
        ),
        (
            "POST",
            "/api/admin/storage",
            Some(json!({"name": "x", "folder": "/srv/x"})),
            F::Locations,
        ),
        ("DELETE", "/api/admin/storage/9", None, F::Locations),
        ("POST", "/api/admin/storage/move", Some(json!({})), F::Move),
        (
            "POST",
            "/api/admin/storage/purge",
            Some(json!({})),
            F::Purge,
        ),
        (
            "POST",
            "/api/admin/storage/import/review",
            Some(json!({})),
            F::Import,
        ),
        (
            "POST",
            "/api/admin/storage/import",
            Some(json!({})),
            F::Import,
        ),
        (
            "POST",
            "/api/admin/storage/consolidate",
            Some(json!({})),
            F::Consolidate,
        ),
        (
            "POST",
            "/api/admin/talkgroups/1/rename",
            Some(json!({"name": "x"})),
            F::Rename,
        ),
        (
            "POST",
            "/api/admin/talkgroups/1/correct",
            Some(json!({})),
            F::Correct,
        ),
        (
            "POST",
            "/api/admin/talkgroups/1/merge",
            Some(json!({})),
            F::Merge,
        ),
        (
            "POST",
            "/api/admin/talkgroups/1/purge",
            Some(json!({})),
            F::PurgeTalkgroup,
        ),
        ("POST", "/api/admin/jobs/queue", Some(json!({})), F::Queue),
        (
            "POST",
            "/api/admin/jobs/asr-run",
            Some(json!({})),
            F::Evaluation,
        ),
        ("POST", "/api/admin/jobs/999/stop", None, F::StopJobs),
        (
            "PUT",
            "/api/admin/favorites/x",
            Some(json!({"talkgroups": []})),
            F::Favorites,
        ),
        ("DELETE", "/api/admin/favorites/x", None, F::Favorites),
        (
            "PUT",
            "/api/admin/settings/web.system_order",
            Some(json!({"value": []})),
            F::Settings,
        ),
        (
            "DELETE",
            "/api/admin/settings/web.system_order",
            None,
            F::Settings,
        ),
        (
            "PUT",
            "/api/admin/settings/maintenance.enabled",
            Some(json!({"value": false})),
            F::Maintenance,
        ),
        (
            "PUT",
            "/api/admin/settings/transcription.model",
            Some(json!({"value": 5})),
            F::TranscriptionSettings,
        ),
        ("DELETE", "/api/admin/settings/logs.syslog", None, F::Syslog),
        (
            "POST",
            "/api/admin/helper",
            Some(json!({"kind": "update"})),
            F::Update,
        ),
        (
            "POST",
            "/api/admin/helper",
            Some(json!({"kind": "switchover"})),
            F::Switchover,
        ),
        (
            "POST",
            "/api/admin/helper",
            Some(json!({"kind": "archive_check"})),
            F::Convert,
        ),
        (
            "POST",
            "/api/admin/helper",
            Some(json!({"kind": "db_move"})),
            F::Convert,
        ),
    ]
}

#[sqlx::test(migrations = "./migrations")]
async fn an_associate_is_refused_every_change_whose_function_is_off(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &[]).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    for (method, uri, body, function) in changes() {
        let answer = send(&app, method, uri, &h, body).await;
        assert_eq!(answer.status, 403, "{method} {uri}: {:?}", answer.body);
        let (_, label) = function.shown();
        let said = answer.body["detail"].as_str().unwrap();
        assert!(said.contains(label), "{method} {uri}: {said}");
    }
    // Every function is in the list at least once.
    for function in Function::ALL {
        assert!(changes().iter().any(|c| c.3 == function), "{function:?}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn an_associate_may_each_change_whose_function_is_on(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &Function::ALL).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    for (method, uri, body, _) in changes() {
        let answer = send(&app, method, uri, &h, body).await;
        assert_ne!(answer.status, 403, "{method} {uri}: {:?}", answer.body);
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn reading_is_never_limited(pool: PgPool) {
    let (app, cookie, _) = associate(&pool, &[]).await;
    let h = [("cookie", cookie.as_str())];
    for uri in [
        "/api/admin/status",
        "/api/admin/storage",
        "/api/admin/jobs",
        "/api/admin/settings",
        "/api/admin/recorders",
        "/api/admin/layout",
        "/api/admin/storage/move?from=2026-01-01&to=2026-01-02&to_location=1",
    ] {
        let answer = send(&app, "GET", uri, &h, None).await;
        assert_ne!(answer.status, 403, "{uri}: {:?}", answer.body);
        assert_ne!(answer.status, 401, "{uri}");
    }
    // The accounts list holds only cy's own account, and every function.
    let listed = send(&app, "GET", "/api/admin/accounts", &h, None).await;
    assert_eq!(listed.status, 200);
    let logins: Vec<&str> = listed.body["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["login"].as_str().unwrap())
        .collect();
    assert_eq!(logins, ["cy"]);
    assert_eq!(
        listed.body["functions"].as_array().unwrap().len(),
        Function::ALL.len()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn me_and_the_sign_in_say_the_role_and_the_functions_on(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let mut conn = pool.acquire().await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    accounts::switch(&pool, "cy", &[Function::Move], true)
        .await
        .unwrap();
    let app = app(&pool);
    let signed = send(
        &app,
        "POST",
        "/api/auth/login",
        &[],
        Some(json!({"login": "cy", "password": PASSWORD})),
    )
    .await;
    assert_eq!(signed.body["role"], "associate");
    assert_eq!(signed.body["functions"], json!(["move"]));
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let me = send(&app, "GET", "/api/auth/me", &[("cookie", &cookie)], None).await;
    assert_eq!(me.body["role"], "super");
    assert_eq!(
        me.body["functions"].as_array().unwrap().len(),
        Function::ALL.len()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_function_turned_on_or_off_counts_from_the_next_request(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &[]).await;
    let cy = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let (boss, boss_csrf) = sign_in(&app, "ann", PASSWORD).await;
    let ann = [
        ("cookie", boss.as_str()),
        ("x-csrf-token", boss_csrf.as_str()),
    ];
    let path = "/api/admin/accounts/cy/functions/rescan";
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &cy, None)
            .await
            .status,
        403
    );
    assert_eq!(send(&app, "PUT", path, &ann, None).await.status, 204);
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &cy, None)
            .await
            .status,
        202
    );
    assert_eq!(send(&app, "DELETE", path, &ann, None).await.status, 204);
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &cy, None)
            .await
            .status,
        403
    );
    // Made a super admin, cy may everything at the next request.
    let role = Some(json!({"role": "super"}));
    let made = send(&app, "PUT", "/api/admin/accounts/cy/role", &ann, role).await;
    assert_eq!(made.status, 204, "{:?}", made.body);
    assert_eq!(
        send(&app, "POST", "/api/admin/rescan", &cy, None)
            .await
            .status,
        202
    );
    // A super admin has no switch to turn.
    let refused = send(&app, "PUT", path, &ann, None).await;
    assert_eq!(refused.status, 409, "{:?}", refused.body);
    let unknown = "/api/admin/accounts/cy/functions/delete-everything";
    assert_eq!(send(&app, "PUT", unknown, &ann, None).await.status, 404);
}

#[sqlx::test(migrations = "./migrations")]
async fn accounts_and_sign_in_settings_are_a_super_admins_only(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &Function::ALL).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let new = Some(json!({"login": "dan", "password": PASSWORD}));
    for (method, uri, body) in [
        ("POST", "/api/admin/accounts", new),
        ("POST", "/api/admin/accounts/ann/disable", None),
        ("POST", "/api/admin/accounts/ann/enable", None),
        (
            "POST",
            "/api/admin/accounts/ann/password",
            Some(json!({"password": PASSWORD})),
        ),
        (
            "PUT",
            "/api/admin/accounts/cy/role",
            Some(json!({"role": "super"})),
        ),
        ("PUT", "/api/admin/accounts/cy/functions/move", None),
        ("DELETE", "/api/admin/accounts/cy/functions/move", None),
        (
            "PUT",
            "/api/admin/settings/auth.password_signin",
            Some(json!({"value": false})),
        ),
        (
            "PUT",
            "/api/admin/settings/oidc.admin_group",
            Some(json!({"value": "x"})),
        ),
        ("DELETE", "/api/admin/settings/web.trusted_proxies", None),
    ] {
        let answer = send(&app, method, uri, &h, body).await;
        assert_eq!(answer.status, 403, "{method} {uri}: {:?}", answer.body);
    }
    // cy's own password is cy's to change.
    let own = Some(json!({"password": "a different long one"}));
    let changed = send(&app, "POST", "/api/admin/accounts/cy/password", &h, own).await;
    assert_eq!(changed.status, 204, "{:?}", changed.body);
}

#[sqlx::test(migrations = "./migrations")]
async fn whoever_started_a_job_may_always_stop_it(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &[]).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let job = |by: &'static str| {
        let pool = pool.clone();
        async move {
            let id: i64 = sqlx::query_scalar(
                "INSERT INTO jobs (kind, started_by) VALUES ('queue', $1) RETURNING id",
            )
            .bind(by)
            .fetch_one(&pool)
            .await
            .unwrap();
            format!("/api/admin/jobs/{id}/stop")
        }
    };
    let own = send(&app, "POST", &job("cy").await, &h, None).await;
    assert_eq!(own.status, 200, "{:?}", own.body);
    let other = send(&app, "POST", &job("ann").await, &h, None).await;
    assert_eq!(other.status, 403, "{:?}", other.body);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_super_admin_adds_an_associate_by_default_and_a_super_admin_when_asked(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let app = app(&pool);
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let cy = Some(json!({"login": "cy", "password": PASSWORD}));
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &h, cy)
            .await
            .status,
        201
    );
    let dan = Some(json!({"login": "dan", "password": PASSWORD, "role": "super"}));
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &h, dan)
            .await
            .status,
        201
    );
    let bad = Some(json!({"login": "eve", "password": PASSWORD, "role": "boss"}));
    assert_eq!(
        send(&app, "POST", "/api/admin/accounts", &h, bad)
            .await
            .status,
        400
    );
    let listed = send(
        &app,
        "GET",
        "/api/admin/accounts",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    let roles: Vec<(String, String)> = listed.body["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["login"].as_str().unwrap().into(),
                a["role"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(
        roles,
        [
            ("ann".to_string(), "super".to_string()),
            ("cy".into(), "associate".into()),
            ("dan".into(), "super".into())
        ]
    );
    // The last enabled super admin cannot be made an associate.
    let dan_off = send(&app, "POST", "/api/admin/accounts/dan/disable", &h, None).await;
    assert_eq!(dan_off.status, 204);
    let last = send(
        &app,
        "PUT",
        "/api/admin/accounts/ann/role",
        &h,
        Some(json!({"role": "associate"})),
    )
    .await;
    assert_eq!(last.status, 409, "{:?}", last.body);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_associate_who_signs_in_through_oidc_cannot_give_themselves_a_password(pool: PgPool) {
    admin(&pool, "ann", PASSWORD).await;
    let mut conn = pool.acquire().await.unwrap();
    let (id, login, _) =
        accounts::oidc_account(&mut conn, "https://id.example.org", "s-1", Some("sso"))
            .await
            .unwrap();
    // Signed in through the provider: an associate with no password.
    let (token, csrf) = scannerpulse::sessions::create(&mut conn, id, true)
        .await
        .unwrap();
    let cookie = format!("scannerpulse_session={token}");
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let app = app(&pool);
    let path = format!("/api/admin/accounts/{login}/password");
    let own = send(&app, "POST", &path, &h, Some(json!({"password": PASSWORD}))).await;
    assert_eq!(own.status, 403, "{:?}", own.body);
    let listed = accounts::list(&pool).await.unwrap();
    let sso = listed.iter().find(|a| a.login == login).unwrap();
    assert!(!sso.password, "the provider stays the only way in");
    // A super admin may still give it one.
    let (boss, boss_csrf) = sign_in(&app, "ann", PASSWORD).await;
    let ann = [
        ("cookie", boss.as_str()),
        ("x-csrf-token", boss_csrf.as_str()),
    ];
    let given = send(
        &app,
        "POST",
        &path,
        &ann,
        Some(json!({"password": PASSWORD})),
    )
    .await;
    assert_eq!(given.status, 204, "{:?}", given.body);
}

#[sqlx::test(migrations = "./migrations")]
async fn transcription_and_syslog_settings_need_their_own_functions(pool: PgPool) {
    let (app, cookie, csrf) = associate(&pool, &[Function::Settings]).await;
    let h = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let servers =
        json!({"value": [{"url": "https://192.0.2.1/v1/audio/transcriptions", "weight": 1}]});
    for (method, uri, body) in [
        (
            "PUT",
            "/api/admin/settings/transcription.servers",
            Some(servers),
        ),
        (
            "PUT",
            "/api/admin/settings/transcription.enabled",
            Some(json!({"value": true})),
        ),
        ("DELETE", "/api/admin/settings/transcription.model", None),
        (
            "PUT",
            "/api/admin/settings/logs.syslog",
            Some(json!({"value": "udp://192.0.2.1:514"})),
        ),
    ] {
        let answer = send(&app, method, uri, &h, body).await;
        assert_eq!(answer.status, 403, "{method} {uri}: {:?}", answer.body);
    }
    let other = Some(json!({"value": ["B", "A"]}));
    let order = send(
        &app,
        "PUT",
        "/api/admin/settings/web.system_order",
        &h,
        other,
    )
    .await;
    assert_ne!(order.status, 403, "{:?}", order.body);
}
