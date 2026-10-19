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

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_sees_the_installs_layout(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let layout = dir.path().join("layout.json");
    scannerpulse::layout::Layout {
        hub: "192.0.2.10".into(),
        nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
        // A documentation address: it never answers.
        referee: Some("192.0.2.1".into()),
        second: Some("radio-b".into()),
        shared: Some("192.0.2.50/24".into()),
        install_id: "0123456789abcdef".into(),
    }
    .write(&layout)
    .unwrap();
    // The test's own database stands for the primary.
    let options = pool.connect_options();
    let primary = scannerpulse::primary::Node {
        host: options.get_host().to_string(),
        port: options.get_port(),
    };
    sqlx::query(
        "INSERT INTO machines (host, version, holds_shared, seen_at) VALUES \
         ('radio-a', '0.7.0', true, now()), ('radio-b', '0.7.0', true, now() - interval '2 minutes')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.host = "radio-a".into();
    state.machine = scannerpulse::web::Machine {
        layout: Some(layout.clone()),
        nodes: vec![primary.clone()],
        ..Default::default()
    };
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    assert_eq!(
        send(&app, "GET", "/api/admin/layout", &[], None)
            .await
            .status,
        401
    );
    let (cookie, _) = signed(&pool, &app).await;
    let got = send(
        &app,
        "GET",
        "/api/admin/layout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    let body = got.body;
    assert_eq!(body["mode"], "referee");
    assert_eq!(body["second"], "radio-b");
    assert_eq!(body["shared"], "192.0.2.50/24");
    assert_eq!(body["machines"][0]["host"], "radio-a");
    assert_eq!(body["machines"][0]["up"], true);
    assert_eq!(body["machines"][0]["holds_shared"], true);
    // Not heard from for two minutes: down, and holding nothing.
    assert_eq!(body["machines"][1]["up"], false);
    assert_eq!(body["machines"][1]["holds_shared"], false);
    assert_eq!(body["database"][0]["host"], primary.host);
    assert_eq!(body["database"][0]["role"], "primary");
    assert_eq!(body["referee"]["host"], "192.0.2.1");
    assert_eq!(body["referee"]["answers"], false);
    assert_eq!(
        body["switchover"],
        "sudo /usr/local/bin/scannerpulse db switchover"
    );
    // A second that has never said it is up is listed as down, not left
    // out; a machine not heard from for a week is forgotten.
    let silent = dir.path().join("silent.json");
    scannerpulse::layout::Layout {
        second: Some("radio-c".into()),
        ..scannerpulse::layout::Layout::read(&layout).unwrap()
    }
    .write(&silent)
    .unwrap();
    sqlx::query("INSERT INTO machines (host, version, holds_shared, seen_at) VALUES ('radio-old', '0.6.0', false, now() - interval '8 days')")
        .execute(&pool)
        .await
        .unwrap();
    scannerpulse::layout::forget_stale(&pool).await.unwrap();
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.machine = scannerpulse::web::Machine {
        layout: Some(silent),
        nodes: vec![primary.clone()],
        ..Default::default()
    };
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let silent = send(
        &app,
        "GET",
        "/api/admin/layout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    let hosts: Vec<_> = silent.body["machines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["host"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(hosts, ["radio-a", "radio-b", "radio-c"]);
    let never = &silent.body["machines"][2];
    assert_eq!(never["up"], false);
    assert!(never["seen_seconds_ago"].is_null());
    assert!(never["version"].is_null());
    // Database machines whose plan this machine has not been given (an
    // install from before the layout file): said so, the machines shown.
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.machine = scannerpulse::web::Machine {
        layout: Some(dir.path().join("none.json")),
        nodes: vec![primary.clone()],
        ..Default::default()
    };
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let unknown = send(
        &app,
        "GET",
        "/api/admin/layout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(unknown.body["mode"], "unknown");
    assert_eq!(unknown.body["database"][0]["role"], "primary");
    assert!(unknown.body["referee"].is_null());
    // One machine: no layout file and no database machines.
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.machine = scannerpulse::web::Machine::default();
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let one = send(
        &app,
        "GET",
        "/api/admin/layout",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(one.body["mode"], "one");
    assert!(one.body["switchover"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_sees_this_machines_certificate_and_backups(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let (cert, key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    scannerpulse::cert::make(&["radio-a.example.org".into()], &cert, &key).unwrap();
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.host = "radio-a".into();
    state.machine = scannerpulse::web::Machine {
        layout: None,
        nodes: Vec::new(),
        cert: Some(cert.clone()),
        backups: Some("/srv/backups".into()),
        pushes: vec!["dump"],
        acme: None,
    };
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    assert_eq!(
        send(&app, "GET", "/api/admin/machine", &[], None)
            .await
            .status,
        401
    );
    scannerpulse::backup::record(&pool, "dump").await.unwrap();
    let (cookie, _) = signed(&pool, &app).await;
    let got = send(
        &app,
        "GET",
        "/api/admin/machine",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(got.status, 200, "{:?}", got.body);
    let body = got.body;
    assert_eq!(body["host"], "radio-a");
    assert_eq!(body["database"], "here");
    assert_eq!(body["certificate"]["names"], json!(["radio-a.example.org"]));
    assert_eq!(body["certificate"]["self_signed"], true);
    assert!(body["certificate"]["not_before"].is_string());
    assert_eq!(body["backups"]["folder"], "/srv/backups");
    assert!(body["backups"]["kinds"]["dump"]["newest"].is_string());
    assert_eq!(body["backups"]["kinds"]["dump"]["push"], true);
    assert!(body["backups"]["kinds"]["pgbackrest"]["newest"].is_null());
    assert_eq!(body["backups"]["kinds"]["pgbackrest"]["push"], false);
    let text = body.to_string();
    assert!(!text.contains("http"), "no push address is shown: {text}");
    // Without a backups folder the database is not one setup installed,
    // wherever its address points.
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.machine = scannerpulse::web::Machine::default();
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    // The admin made above signs in again, on this second app.
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let got = send(
        &app,
        "GET",
        "/api/admin/machine",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(got.body["database"], "theirs");
    assert!(got.body["backups"].is_null());
    assert!(got.body["acme"].is_null());

    // With Let's Encrypt: the certificate served, how it is proved and its
    // last renewal; never the token.
    let served = dir.path().join("served");
    std::fs::create_dir(&served).unwrap();
    let (c, k) = (served.join("cert.pem"), served.join("key.pem"));
    scannerpulse::cert::make(&["radio.example.org".into()], &c, &k).unwrap();
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.certs = Some(std::sync::Arc::new(scannerpulse::acme::Certs::new(
        std::sync::Arc::new(scannerpulse::cert::certified(&c, &k).unwrap()),
    )));
    state.machine = scannerpulse::web::Machine {
        layout: None,
        nodes: Vec::new(),
        cert: Some(cert),
        acme: Some(scannerpulse::acme::Settings {
            proof: scannerpulse::acme::Proof::Cloudflare {
                token: "cf-token-1".into(),
            },
            names: vec!["radio.example.org".into()],
            directory: scannerpulse::acme::LETS_ENCRYPT.into(),
            cloudflare_api: None,
            host: "radio-a".into(),
        }),
        ..Default::default()
    };
    state.renewal.lock().unwrap().error = Some("radio.example.org: no TXT record".into());
    let app = scannerpulse::web::router(state).layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((common::PEER, 40_000)),
    ));
    let (cookie, _) = sign_in(&app, "ann", PASSWORD).await;
    let got = send(
        &app,
        "GET",
        "/api/admin/machine",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    let body = got.body;
    assert_eq!(body["certificate"]["names"], json!(["radio.example.org"]));
    assert_eq!(body["acme"]["proof"], "dns-cloudflare");
    assert_eq!(body["acme"]["names"], json!(["radio.example.org"]));
    assert_eq!(body["acme"]["error"], "radio.example.org: no TXT record");
    assert!(body["acme"]["renewed"].is_null());
    assert!(!body.to_string().contains("cf-token-1"), "{body}");
}
