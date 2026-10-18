//! HTTPS: `web` serves TLS with a certificate `cert::make` wrote, and plain
//! HTTP only on this machine's own loopback address.

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use scannerpulse::cert;
use scannerpulse::web::{self, Config, Tls};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn web_serves_https_with_a_made_certificate_and_nothing_in_plain_text() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_path, key_path) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    cert::make(
        &["localhost".into(), "127.0.0.1".into()],
        &cert_path,
        &key_path,
    )
    .unwrap();
    let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&key_path), 0o600, "the key is the owner's alone");
    assert_eq!(mode(&cert_path), 0o644);

    let port = free_port();
    let config = Config {
        database: "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        static_dir: None,
        host: "tls-test".into(),
        oidc_secret: None,
        machine: Default::default(),
        nodes: Vec::new(),
        acme_dir: Default::default(),
        tls: Some(Tls {
            cert: cert_path.clone(),
            key: key_path,
        }),
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(web::run(config, async {
        let _ = stopped.await;
    }));

    let ca = reqwest::Certificate::from_pem(&std::fs::read(&cert_path).unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .add_root_certificate(ca)
        .build()
        .unwrap();
    let mut answer = None;
    for _ in 0..50 {
        if let Ok(response) = client
            .get(format!("https://localhost:{port}/health"))
            .send()
            .await
        {
            answer = Some(response.text().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(answer.as_deref(), Some("ok"), "the name in the certificate");
    let by_address = client
        .get(format!("https://127.0.0.1:{port}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        by_address.text().await.unwrap(),
        "ok",
        "the address in the certificate"
    );
    let plain = reqwest::get(format!("http://127.0.0.1:{port}/health")).await;
    assert!(
        plain.map(|r| !r.status().is_success()).unwrap_or(true),
        "no page in plain text on the HTTPS port"
    );

    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn an_address_off_this_machine_needs_a_certificate() {
    assert!(web::check_listen("127.0.0.1:8080".parse().unwrap(), false).is_ok());
    assert!(web::check_listen("[::1]:8080".parse().unwrap(), false).is_ok());
    let refused = web::check_listen("0.0.0.0:443".parse().unwrap(), false).unwrap_err();
    assert!(refused.contains("SCANNERPULSE_TLS_CERT"), "{refused}");
    assert!(web::check_listen("192.0.2.10:443".parse().unwrap(), false).is_err());
    assert!(web::check_listen("0.0.0.0:443".parse().unwrap(), true).is_ok());
}

#[tokio::test]
async fn run_refuses_plain_http_off_loopback_for_any_caller() {
    let config = Config {
        database: "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
        listen: "192.0.2.10:443".parse().unwrap(),
        static_dir: None,
        host: "tls-test".into(),
        oidc_secret: None,
        machine: Default::default(),
        nodes: Vec::new(),
        acme_dir: Default::default(),
        tls: None,
    };
    let error = web::run(config, std::future::pending()).await.unwrap_err();
    assert!(error.contains("SCANNERPULSE_TLS_CERT"), "{error}");
}

#[tokio::test]
async fn with_lets_encrypt_on_the_kept_certificate_is_served_from_the_start() {
    use scannerpulse::acme::{Proof, Settings};
    let dir = tempfile::tempdir().unwrap();
    let (own, own_key) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    cert::make(&["localhost".into(), "127.0.0.1".into()], &own, &own_key).unwrap();
    let kept = dir.path().join("acme");
    std::fs::create_dir(&kept).unwrap();
    // Kept as Let's Encrypt's would be: it signed itself here, so the
    // renewal asks at once, fails (nothing answers there), and it stays.
    let made = dir.path().join("made");
    std::fs::create_dir(&made).unwrap();
    cert::make(
        &["radio.example.org".into()],
        &made.join("cert.pem"),
        &made.join("key.pem"),
    )
    .unwrap();
    scannerpulse::acme::keep_certificate(
        &kept,
        &Settings {
            proof: Proof::Web,
            names: vec!["radio.example.org".into()],
            directory: "https://127.0.0.1:1/directory".into(),
            cloudflare_api: None,
        },
        &std::fs::read_to_string(made.join("cert.pem")).unwrap(),
        &std::fs::read_to_string(made.join("key.pem")).unwrap(),
    )
    .unwrap();
    let port = free_port();
    let config = Config {
        database: "postgres://nobody@127.0.0.1:1/none".parse().unwrap(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        static_dir: None,
        host: "tls-test".into(),
        oidc_secret: None,
        machine: web::Machine {
            acme: Some(Settings {
                proof: Proof::Web,
                names: vec!["radio.example.org".into()],
                // Nothing answers there: the renewal fails, and the kept
                // certificate stays.
                directory: "https://127.0.0.1:1/directory".into(),
                cloudflare_api: None,
            }),
            ..Default::default()
        },
        nodes: Vec::new(),
        acme_dir: kept.clone(),
        tls: Some(Tls {
            cert: own,
            key: own_key,
        }),
    };
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(web::run(config, async {
        let _ = stopped.await;
    }));
    let ca =
        reqwest::Certificate::from_pem(&std::fs::read(kept.join("cert.pem")).unwrap()).unwrap();
    let client = reqwest::Client::builder()
        .tls_built_in_root_certs(false)
        .add_root_certificate(ca)
        .resolve(
            "radio.example.org",
            format!("127.0.0.1:{port}").parse().unwrap(),
        )
        .build()
        .unwrap();
    let mut answer = None;
    for _ in 0..50 {
        if let Ok(response) = client
            .get(format!("https://radio.example.org:{port}/health"))
            .send()
            .await
        {
            answer = Some(response.text().await.unwrap());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        answer.as_deref(),
        Some("ok"),
        "the kept certificate is served"
    );
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
}
