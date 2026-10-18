//! Let's Encrypt: what the settings say, when a certificate is renewed, the
//! certificate a TLS-ALPN-01 check is shown, the one kept on disk, and the
//! DNS-01 records at Cloudflare (against a stand-in for its API).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use scannerpulse::acme::{self, Certs, Proof, Settings};
use scannerpulse::cert;

fn settings(pairs: &[(&str, &str)]) -> Result<Option<Settings>, String> {
    Settings::read(|name| {
        pairs
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v.to_string())
    })
}

#[test]
fn the_settings_name_the_proof_the_names_and_the_authority() {
    assert!(settings(&[]).unwrap().is_none(), "off unless asked");
    let web = settings(&[
        ("SCANNERPULSE_ACME", "web"),
        (
            "SCANNERPULSE_ACME_NAMES",
            "radio.example.org  www.radio.example.org",
        ),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(web.proof, Proof::Web);
    assert_eq!(web.names, ["radio.example.org", "www.radio.example.org"]);
    assert_eq!(web.directory, acme::LETS_ENCRYPT);
    let dns = settings(&[
        ("SCANNERPULSE_ACME", "dns-cloudflare"),
        ("SCANNERPULSE_ACME_NAMES", "radio.example.org"),
        ("SCANNERPULSE_CLOUDFLARE_TOKEN", "cf-token-1"),
        (
            "SCANNERPULSE_ACME_DIRECTORY",
            "https://acme.example.org/dir",
        ),
    ])
    .unwrap()
    .unwrap();
    assert_eq!(
        dns.proof,
        Proof::Cloudflare {
            token: "cf-token-1".into()
        }
    );
    assert_eq!(dns.directory, "https://acme.example.org/dir");
    assert!(
        !format!("{dns:?}").contains("cf-token-1"),
        "the token is never shown"
    );
    let refused = |pairs: &[(&str, &str)], why: &str| {
        let error = settings(pairs).unwrap_err();
        assert!(error.contains(why), "{pairs:?}: {error}");
    };
    refused(&[("SCANNERPULSE_ACME", "http")], "web or dns-cloudflare");
    refused(&[("SCANNERPULSE_ACME", "web")], "SCANNERPULSE_ACME_NAMES");
    refused(
        &[
            ("SCANNERPULSE_ACME", "dns-cloudflare"),
            ("SCANNERPULSE_ACME_NAMES", "radio.example.org"),
        ],
        "SCANNERPULSE_CLOUDFLARE_TOKEN",
    );
    refused(
        &[
            ("SCANNERPULSE_ACME", "web"),
            ("SCANNERPULSE_ACME_NAMES", "192.0.2.10"),
        ],
        "a name such as radio.example.org",
    );
}

#[test]
fn a_name_for_lets_encrypt_is_a_public_dns_name() {
    for good in [
        "radio.example.org",
        "a-1.radio.example.org",
        "Radio.Example.Org",
    ] {
        acme::check_name(good).unwrap();
    }
    for bad in [
        "radio",
        "192.0.2.10",
        "2001:db8::1",
        "*.example.org",
        "-radio.example.org",
        "radio..example.org",
        "radio.example.org.",
        "",
    ] {
        assert!(acme::check_name(bad).is_err(), "{bad:?}");
    }
}

fn found(names: &[&str], days_old: i64, days_left: i64) -> cert::Found {
    let now = Utc::now();
    cert::Found {
        names: names.iter().map(|n| n.to_string()).collect(),
        not_before: now - Duration::days(days_old),
        not_after: now + Duration::days(days_left),
        self_signed: false,
    }
}

#[test]
fn a_certificate_is_renewed_with_a_third_of_its_life_left_or_for_other_names() {
    let names = vec!["radio.example.org".to_string()];
    let now = Utc::now();
    assert!(!acme::due(
        &found(&["radio.example.org"], 10, 80),
        &names,
        now
    ));
    assert!(acme::due(
        &found(&["radio.example.org"], 61, 29),
        &names,
        now
    ));
    // A 45-day certificate: renewed after 30 days, with 15 left.
    assert!(!acme::due(
        &found(&["radio.example.org"], 29, 16),
        &names,
        now
    ));
    assert!(acme::due(
        &found(&["radio.example.org"], 31, 14),
        &names,
        now
    ));
    assert!(acme::due(
        &found(&["radio-a", "192.0.2.10"], 1, 1800),
        &names,
        now
    ));
    let two = vec![
        "radio.example.org".to_string(),
        "www.example.org".to_string(),
    ];
    assert!(acme::due(&found(&["radio.example.org"], 1, 89), &two, now));
    assert!(!acme::due(
        &found(&["www.example.org", "radio.example.org"], 1, 89),
        &two,
        now
    ));
}

fn made(names: &[&str]) -> (tempfile::TempDir, Arc<rustls::sign::CertifiedKey>) {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = (dir.path().join("cert.pem"), dir.path().join("key.pem"));
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    cert::make(&names, &c, &k).unwrap();
    let key = cert::certified(&c, &k).unwrap();
    (dir, Arc::new(key))
}

/// Accepts any certificate: the test reads what the server showed.
#[derive(Debug)]
struct Anything;

impl rustls::client::danger::ServerCertVerifier for Anything {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _: &[u8],
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The certificate and the protocol the server gives a client that asks for
/// `name` with `alpn`.
fn shown(certs: &Arc<Certs>, name: &str, alpn: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let config = Arc::new(acme::server_config(certs.clone()));
    let server = std::thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut tls = rustls::StreamOwned::new(rustls::ServerConnection::new(config).unwrap(), tcp);
        let _ = tls.read(&mut [0u8; 1]);
    });
    let mut client = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Anything))
        .with_no_client_auth();
    client.alpn_protocols = vec![alpn.to_vec()];
    let connection = rustls::ClientConnection::new(
        Arc::new(client),
        rustls::pki_types::ServerName::try_from(name.to_string()).unwrap(),
    )
    .unwrap();
    let mut tls =
        rustls::StreamOwned::new(connection, std::net::TcpStream::connect(address).unwrap());
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).unwrap();
    }
    let cert = tls.conn.peer_certificates().unwrap()[0].to_vec();
    let protocol = tls.conn.alpn_protocol().map(<[u8]>::to_vec);
    let _ = tls.write_all(b"x");
    drop(tls);
    server.join().unwrap();
    (cert, protocol)
}

/// The acmeIdentifier extension (RFC 8737) of `der`, if it has one.
fn acme_identifier(der: &[u8]) -> Option<Vec<u8>> {
    let (_, parsed) = x509_parser::parse_x509_certificate(der).unwrap();
    parsed
        .extensions()
        .iter()
        .find(|e| e.oid.to_id_string() == "1.3.6.1.5.5.7.1.31")
        .map(|e| e.value.to_vec())
}

#[test]
fn a_tls_alpn_check_is_shown_the_challenge_and_everyone_else_the_site() {
    let (_dir, site) = made(&["radio-a", "127.0.0.1"]);
    let certs = Arc::new(Certs::new(site.clone()));
    let digest = [7u8; 32];
    certs.answer("radio.example.org", &digest).unwrap();

    let (cert, protocol) = shown(&certs, "radio.example.org", b"acme-tls/1");
    assert_eq!(protocol.as_deref(), Some(&b"acme-tls/1"[..]));
    let extension = acme_identifier(&cert).expect("the challenge certificate");
    assert!(extension.ends_with(&digest), "{extension:?}");

    let (cert, protocol) = shown(&certs, "radio.example.org", b"http/1.1");
    assert_eq!(protocol.as_deref(), Some(&b"http/1.1"[..]));
    assert_eq!(cert, site.cert[0].to_vec(), "a browser gets the site's own");

    certs.forget("radio.example.org");
    let (cert, _) = shown(&certs, "radio.example.org", b"http/1.1");
    assert!(acme_identifier(&cert).is_none());

    let (_dir2, renewed) = made(&["radio.example.org"]);
    certs.serve(renewed.clone());
    let (cert, _) = shown(&certs, "radio.example.org", b"http/1.1");
    assert_eq!(cert, renewed.cert[0].to_vec(), "a new certificate at once");
}

#[test]
fn the_certificate_kept_on_disk_is_served_only_for_the_names_asked() {
    let dir = tempfile::tempdir().unwrap();
    let names = vec!["radio.example.org".to_string()];
    assert!(acme::stored(dir.path(), &names).is_none(), "none yet");
    cert::make(
        &names,
        &dir.path().join("cert.pem"),
        &dir.path().join("key.pem"),
    )
    .unwrap();
    let kept = acme::stored(dir.path(), &names).expect("kept");
    assert_eq!(cert::found(&kept).unwrap().names, names);
    assert!(acme::stored(dir.path(), &["other.example.org".to_string()]).is_none());
}

/// A stand-in for Cloudflare's API: one zone, `example.org`; it keeps every
/// request it gets.
async fn cloudflare() -> (String, Arc<Mutex<Vec<String>>>) {
    use axum::extract::{Path, Query};
    use axum::http::HeaderMap;
    use axum::routing::{delete, get};
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let auth = |headers: &HeaderMap| {
        headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer cf-token-1")
    };
    let (s1, s2, s3) = (seen.clone(), seen.clone(), seen.clone());
    let app = axum::Router::new()
        .route(
            "/zones",
            get(
                move |headers: HeaderMap, Query(q): Query<std::collections::HashMap<String, String>>| async move {
                    s1.lock().unwrap().push(format!("zones {}", q["name"]));
                    if !auth(&headers) {
                        return axum::Json(serde_json::json!({"success": false, "errors": [{"code": 9109, "message": "Invalid access token"}], "result": null}));
                    }
                    let result = if q["name"] == "example.org" {
                        serde_json::json!([{"id": "zone-1", "name": "example.org"}])
                    } else {
                        serde_json::json!([])
                    };
                    axum::Json(serde_json::json!({"success": true, "errors": [], "result": result}))
                },
            ),
        )
        .route(
            "/zones/{zone}/dns_records",
            axum::routing::post(
                move |Path(zone): Path<String>, axum::Json(body): axum::Json<serde_json::Value>| async move {
                    s2.lock().unwrap().push(format!(
                        "add {zone} {} {} {}",
                        body["type"], body["name"], body["content"]
                    ));
                    axum::Json(serde_json::json!({"success": true, "errors": [], "result": {"id": "record-9"}}))
                },
            ),
        )
        .route(
            "/zones/{zone}/dns_records/{record}",
            delete(move |Path((zone, record)): Path<(String, String)>| async move {
                s3.lock().unwrap().push(format!("remove {zone} {record}"));
                axum::Json(serde_json::json!({"success": true, "errors": [], "result": {"id": record}}))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen)
}

#[tokio::test]
async fn a_dns_record_is_made_in_the_names_zone_and_removed() {
    use scannerpulse::cloudflare::Cloudflare;
    let (api, seen) = cloudflare().await;
    let cf = Cloudflare::with_api(&api, "cf-token-1");
    let zone = cf.zone("radio.west.example.org").await.unwrap();
    assert_eq!(zone, "zone-1");
    let record = cf
        .add_txt(&zone, "_acme-challenge.radio.west.example.org", "abc-123")
        .await
        .unwrap();
    cf.remove(&zone, &record).await.unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        [
            "zones radio.west.example.org",
            "zones west.example.org",
            "zones example.org",
            "add zone-1 \"TXT\" \"_acme-challenge.radio.west.example.org\" \"abc-123\"",
            "remove zone-1 record-9",
        ]
    );
    let error = cf.zone("radio.example.net").await.unwrap_err();
    assert!(
        error.contains("no Cloudflare zone holds radio.example.net"),
        "{error}"
    );
    let wrong = Cloudflare::with_api(&api, "cf-token-2");
    let error = wrong.zone("radio.example.org").await.unwrap_err();
    assert!(error.contains("Invalid access token"), "{error}");
    assert!(!error.contains("cf-token-2"), "{error}");
}
