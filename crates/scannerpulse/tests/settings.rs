use std::collections::HashMap;
use std::time::Duration;

use scannerpulse::settings::{self, IngestSettings, SettingsError};
use serde_json::{Value, json};
use sqlx::PgPool;

fn values(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect()
}

#[test]
fn defaults_apply_when_only_the_zone_is_set() {
    let got = settings::from_values(&values(&[("time_zone", json!("America/Chicago"))])).unwrap();
    assert_eq!(
        got,
        IngestSettings {
            time_zone: chrono_tz::America::Chicago,
            rescan: Duration::from_secs(10),
            missing_source_grace: Duration::from_secs(900),
            done_rows_kept_days: 7,
        }
    );
}

#[test]
fn the_zone_has_no_default() {
    let err = settings::from_values(&HashMap::new()).unwrap_err();
    assert!(matches!(err, SettingsError::Missing("time_zone")), "{err}");
}

#[test]
fn an_unknown_zone_is_refused() {
    for bad in [json!("America/Nowhere"), json!(""), json!(5)] {
        let err = settings::from_values(&values(&[("time_zone", bad.clone())])).unwrap_err();
        assert!(
            matches!(
                err,
                SettingsError::Invalid {
                    key: "time_zone",
                    ..
                }
            ),
            "{bad}: {err}"
        );
    }
}

#[test]
fn the_rescan_must_stay_below_the_grace() {
    let err = settings::from_values(&values(&[
        ("time_zone", json!("UTC")),
        ("ingest.rescan_seconds", json!(60)),
        ("ingest.missing_source_grace_seconds", json!(60)),
    ]))
    .unwrap_err();
    assert!(
        matches!(
            err,
            SettingsError::Invalid {
                key: "ingest.rescan_seconds",
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn a_number_setting_is_a_whole_number_from_one_up() {
    for bad in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!("10"),
        json!(5_000_000_000u64),
    ] {
        let err = settings::from_values(&values(&[
            ("time_zone", json!("UTC")),
            ("ingest.done_rows_kept_days", bad.clone()),
        ]))
        .unwrap_err();
        assert!(
            matches!(
                err,
                SettingsError::Invalid {
                    key: "ingest.done_rows_kept_days",
                    ..
                }
            ),
            "{bad}: {err}"
        );
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_settings_are_read_from_the_table(pool: PgPool) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES \
         ('time_zone', '\"Europe/Paris\"'), ('ingest.rescan_seconds', '5'), ('unrelated', 'true')",
    )
    .execute(&pool)
    .await?;
    let got = settings::load(&pool).await.unwrap();
    assert_eq!(got.time_zone, chrono_tz::Europe::Paris);
    assert_eq!(got.rescan, Duration::from_secs(5));
    assert_eq!(got.missing_source_grace, Duration::from_secs(900));
    Ok(())
}

fn on(extra: &[(&str, Value)]) -> HashMap<String, Value> {
    let mut pairs = vec![
        ("transcription.enabled", json!(true)),
        (
            "transcription.servers",
            json!([{"url": "http://asr-1:8000/v1/audio/transcriptions", "weight": 3},
                   {"url": "https://asr-2/v1/audio/transcriptions"}]),
        ),
        ("transcription.model", json!("model-x")),
        ("transcription.min_duration_ms", json!(1000)),
    ];
    pairs.extend(extra.iter().cloned());
    values(&pairs)
}

#[test]
fn transcription_is_off_unless_it_is_turned_on() {
    assert_eq!(
        settings::transcription_from_values(&values(&[])).unwrap(),
        None
    );
    let off = values(&[
        ("transcription.enabled", json!(false)),
        ("transcription.model", json!(7)),
    ]);
    assert_eq!(settings::transcription_from_values(&off).unwrap(), None);
}

#[test]
fn transcription_on_takes_its_servers_model_and_minimum() {
    let t = settings::transcription_from_values(&on(&[]))
        .unwrap()
        .unwrap();
    assert_eq!(t.servers.len(), 2);
    assert_eq!((t.servers[0].weight, t.servers[1].weight), (3, 1));
    assert_eq!(t.model, "model-x");
    assert_eq!(t.min_duration_ms, 1000);
    assert_eq!(t.request_timeout, Duration::from_secs(120));
    assert_eq!((t.language, t.prompt), (None, None));
}

#[test]
fn transcription_on_needs_servers_a_model_and_a_minimum() {
    for missing in [
        "transcription.servers",
        "transcription.model",
        "transcription.min_duration_ms",
    ] {
        let mut values = on(&[]);
        values.remove(missing);
        match settings::transcription_from_values(&values) {
            Err(SettingsError::Missing(key)) => assert_eq!(key, missing),
            other => panic!("{missing}: {other:?}"),
        }
    }
}

#[test]
fn a_bad_server_list_is_refused() {
    for servers in [
        json!([]),
        json!("http://asr-1"),
        json!([{"url": "asr-1:8000"}]),
        json!([{"url": "http://asr-1", "weight": 0}]),
        json!([{"url": "http://asr-1"}, {"url": "http://asr-1"}]),
        json!([{"url": "http:// host"}]),
        json!([{"url": "http://asr-1", "weight": 65}]),
        json!([{"url": "http://user:key@asr-1:8000/v1/audio/transcriptions"}]),
    ] {
        let values = on(&[("transcription.servers", servers.clone())]);
        assert!(
            matches!(
                settings::transcription_from_values(&values),
                Err(SettingsError::Invalid {
                    key: "transcription.servers",
                    ..
                })
            ),
            "{servers}"
        );
    }
}

#[test]
fn a_key_in_a_server_entry_is_refused_and_not_repeated() {
    for servers in [
        json!([{"url": "http://asr-1/v1?api_key=leak-4417"}]),
        json!([{"url": "http://asr-1/v1#leak-4417"}]),
        json!([{"url": "http://asr-1/v1", "api_key": "leak-4417"}]),
        json!([{"url": "http://asr-1/v1", "weight": "leak-4417"}]),
    ] {
        let values = on(&[("transcription.servers", servers.clone())]);
        let error = settings::transcription_from_values(&values).unwrap_err();
        assert!(
            matches!(
                error,
                SettingsError::Invalid {
                    key: "transcription.servers",
                    ..
                }
            ),
            "{servers}"
        );
        assert!(!error.to_string().contains("leak-4417"), "{error}");
    }
}

#[test]
fn a_server_url_is_http_or_https_without_credentials() {
    assert!(settings::server_url_ok(
        "http://asr-1:8000/v1/audio/transcriptions"
    ));
    for url in [
        "ftp://x",
        "http://user:key@x/",
        "http://user@x/",
        "http://x/?api_key=k",
        "http://x/#k",
    ] {
        assert!(!settings::server_url_ok(url), "{url}");
    }
}

#[test]
fn a_bad_language_is_refused() {
    for bad in ["english", "EN"] {
        let values = on(&[("transcription.language", json!(bad))]);
        assert!(
            matches!(
                settings::transcription_from_values(&values),
                Err(SettingsError::Invalid {
                    key: "transcription.language",
                    ..
                })
            ),
            "{bad}"
        );
    }
    for good in ["en", "eng"] {
        let values = on(&[("transcription.language", json!(good))]);
        let t = settings::transcription_from_values(&values)
            .unwrap()
            .unwrap();
        assert_eq!(t.language.as_deref(), Some(good));
    }
}

#[test]
fn the_prompt_and_the_terms_make_one_prompt() {
    let t = settings::transcription_from_values(&on(&[
        ("transcription.prompt", json!("Fire and police dispatch.")),
        (
            "transcription.terms",
            json!(["Engine 5", " Main Street ", ""]),
        ),
        ("transcription.language", json!("en")),
    ]))
    .unwrap()
    .unwrap();
    assert_eq!(
        t.prompt.as_deref(),
        Some("Fire and police dispatch. Engine 5, Main Street")
    );
    assert_eq!(t.language.as_deref(), Some("en"));
    let terms_only = settings::whisper_prompt("", &["Engine 5".to_string()]);
    assert_eq!(terms_only.as_deref(), Some("Engine 5"));
    assert_eq!(settings::whisper_prompt("  ", &[]), None);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_transcription_settings_are_read_from_the_table(pool: PgPool) -> sqlx::Result<()> {
    assert_eq!(settings::load_transcription(&pool).await.unwrap(), None);
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('transcription.enabled', 'true'), \
         ('transcription.servers', '[{\"url\": \"http://asr-1/v1/audio/transcriptions\"}]'), \
         ('transcription.model', '\"model-x\"'), ('transcription.min_duration_ms', '1000')",
    )
    .execute(&pool)
    .await?;
    let t = settings::load_transcription(&pool).await.unwrap().unwrap();
    assert_eq!(t.servers[0].url, "http://asr-1/v1/audio/transcriptions");
    Ok(())
}

#[test]
fn the_improvement_module_and_the_public_list_are_off_unless_turned_on() {
    let off = settings::improvement_from_values(&values(&[])).unwrap();
    assert_eq!(off, settings::Improvement::default());
    let on = settings::improvement_from_values(&values(&[
        ("improvement_module.enabled", json!(true)),
        ("labeler_list.public", json!(true)),
    ]))
    .unwrap();
    assert!(on.enabled && on.labeler_list_public);
    let bad =
        settings::improvement_from_values(&values(&[("improvement_module.enabled", json!("yes"))]));
    assert!(matches!(
        bad,
        Err(SettingsError::Invalid {
            key: "improvement_module.enabled",
            ..
        })
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_improvement_switches_are_read_from_the_table(pool: PgPool) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', 'true')")
        .execute(&pool)
        .await?;
    let read = settings::load_improvement(&pool).await.unwrap();
    assert!(read.enabled);
    assert!(!read.labeler_list_public);
    Ok(())
}

// Changing a setting: `check` refuses what the reader would refuse.

fn refused(key: &str, value: Option<Value>, stored: &[(&str, Value)]) -> bool {
    settings::check(key, value.as_ref(), &values(stored)).is_err()
}

#[test]
fn a_setting_no_part_reads_is_refused() {
    let error =
        settings::check("oidc.client_secret", Some(&json!("s")), &HashMap::new()).unwrap_err();
    assert!(matches!(error, SettingsError::Unknown(_)), "{error}");
}

#[test]
fn ingest_settings_are_checked_together() {
    let zone = [("time_zone", json!("America/Chicago"))];
    assert!(refused("time_zone", Some(json!("Mars/Olympus")), &[]));
    assert!(!refused("time_zone", Some(json!("UTC")), &[]));
    // The grace period's default is 900: a rescan of 900 is not below it.
    assert!(refused("ingest.rescan_seconds", Some(json!(900)), &zone));
    assert!(!refused("ingest.rescan_seconds", Some(json!(899)), &zone));
    assert!(refused("ingest.rescan_seconds", Some(json!(0)), &zone));
    // The others can be set before the zone, but the zone cannot be removed.
    assert!(!refused("ingest.rescan_seconds", Some(json!(30)), &[]));
    assert!(refused("ingest.rescan_seconds", Some(json!(900)), &[]));
    assert!(refused("time_zone", None, &zone));
}

#[test]
fn transcription_values_are_checked_even_while_it_is_off() {
    assert!(refused(
        "transcription.servers",
        Some(json!([{"url": "ftp://x/"}])),
        &[]
    ));
    assert!(refused(
        "transcription.servers",
        Some(json!([{"url": "http://u:p@x/"}])),
        &[]
    ));
    assert!(!refused(
        "transcription.servers",
        Some(json!([{"url": "http://x:8000/v1/audio/transcriptions"}])),
        &[]
    ));
    assert!(refused(
        "transcription.language",
        Some(json!("English")),
        &[]
    ));
    assert!(!refused(
        "transcription.prompt",
        Some(json!("Fire dispatch.")),
        &[]
    ));
    // Turning it on needs the servers, the model and the minimum length.
    assert!(refused("transcription.enabled", Some(json!(true)), &[]));
    let ready = [
        ("transcription.servers", json!([{"url": "http://x:8000/"}])),
        ("transcription.model", json!("model-x")),
        ("transcription.min_duration_ms", json!(1000)),
    ];
    assert!(!refused("transcription.enabled", Some(json!(true)), &ready));
    assert!(refused("transcription.enabled", Some(json!("yes")), &ready));
    // While it is on, a needed value cannot be removed.
    let mut on = ready.to_vec();
    on.push(("transcription.enabled", json!(true)));
    assert!(refused("transcription.model", None, &on));
}

#[test]
fn the_syslog_server_is_udp_or_tcp_with_a_host() {
    for good in [
        "udp://192.0.2.30:514",
        "tcp://logs.example.org:6514",
        "udp://[2001:db8::30]:514",
        "udp://192.0.2.30",
    ] {
        assert!(!refused("logs.syslog", Some(json!(good)), &[]), "{good}");
    }
    for bad in [
        "192.0.2.30:514",
        "http://192.0.2.30",
        "udp://",
        "tcp://192.0.2.30:0",
        "udp://a@192.0.2.30",
        "udp://192.0.2.30/x",
    ] {
        assert!(refused("logs.syslog", Some(json!(bad)), &[]), "{bad}");
    }
    assert!(refused("logs.syslog", Some(json!(514)), &[]));
    assert!(!refused("logs.syslog", None, &[]));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_service_follows_the_syslog_setting(pool: PgPool) -> sqlx::Result<()> {
    let server = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    server
        .set_read_timeout(Some(Duration::from_millis(10)))
        .unwrap();
    let mut conn = pool.acquire().await?;
    let value = json!(format!("udp://{}", server.local_addr().unwrap()));
    settings::change(&mut conn, "logs.syslog", Some(&value))
        .await
        .unwrap();
    settings::follow_syslog(pool.clone(), "scannerpulse-test");
    let mut got = [0u8; 512];
    let mut heard = None;
    for _ in 0..25 {
        scannerpulse::log::info("following");
        // The follower runs on this test's one thread: give it a turn.
        tokio::time::sleep(Duration::from_millis(200)).await;
        if let Ok(n) = server.recv(&mut got) {
            heard = Some(String::from_utf8_lossy(&got[..n]).into_owned());
            break;
        }
    }
    let heard = heard.expect("a line reached the server");
    assert!(heard.ends_with("- - following"), "{heard}");
    Ok(())
}

#[test]
fn the_web_services_settings_have_their_forms() {
    assert!(!refused("maintenance.enabled", Some(json!(true)), &[]));
    assert!(refused("maintenance.enabled", Some(json!("on")), &[]));
    assert!(!refused(
        "web.trusted_proxies",
        Some(json!(["192.0.2.10", "fd00::2"])),
        &[]
    ));
    assert!(refused(
        "web.trusted_proxies",
        Some(json!(["proxy.lan"])),
        &[]
    ));
    assert!(refused(
        "web.trusted_proxies",
        Some(json!("192.0.2.10")),
        &[]
    ));
    assert!(!refused(
        "web.system_order",
        Some(json!(["SYSB", "SYSA"])),
        &[]
    ));
    assert!(refused("web.system_order", Some(json!([1, 2])), &[]));
    assert!(!refused(
        "purge.holding_path",
        Some(json!("/srv/purged")),
        &[]
    ));
    assert!(refused("purge.holding_path", Some(json!("purged")), &[]));
    assert!(refused(
        "purge.holding_path",
        Some(json!("/srv/../etc")),
        &[]
    ));
    assert!(!refused("auth.password_signin", None, &[]));
    assert!(refused("labeler_list.public", Some(json!(1)), &[]));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_refused_change_leaves_the_stored_value(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    settings::change(&mut conn, "time_zone", Some(&json!("UTC")))
        .await
        .unwrap();
    assert!(
        settings::change(&mut conn, "time_zone", Some(&json!("Nowhere")))
            .await
            .is_err()
    );
    assert!(
        settings::change(&mut conn, "time_zone", None)
            .await
            .is_err()
    );
    let stored = settings::all(&pool).await.unwrap();
    assert_eq!(stored["time_zone"], json!("UTC"));
    settings::change(&mut conn, "ingest.rescan_seconds", Some(&json!(20)))
        .await
        .unwrap();
    settings::change(&mut conn, "ingest.rescan_seconds", None)
        .await
        .unwrap();
    assert!(
        !settings::all(&pool)
            .await
            .unwrap()
            .contains_key("ingest.rescan_seconds")
    );
}

#[test]
fn the_oidc_settings_have_their_forms() {
    assert!(!refused(
        "oidc.issuer",
        Some(json!("https://id.example/application/o/sp/")),
        &[]
    ));
    assert!(!refused(
        "oidc.issuer",
        Some(json!("http://127.0.0.1:9000/")),
        &[]
    ));
    assert!(refused(
        "oidc.issuer",
        Some(json!("http://id.example/")),
        &[]
    ));
    assert!(refused(
        "oidc.issuer",
        Some(json!("https://u:p@id.example/")),
        &[]
    ));
    assert!(refused(
        "oidc.redirect_url",
        Some(json!("https://sp.example/cb?x=1")),
        &[]
    ));
    assert!(!refused(
        "oidc.admin_group",
        Some(json!("scannerpulse-admins")),
        &[]
    ));
    assert!(refused("oidc.client_id", Some(json!(" ")), &[]));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stored_setting_no_part_reads_can_be_removed(pool: PgPool) {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('no.such', '1')")
        .execute(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        settings::change(&mut conn, "no.such", Some(&json!(1)))
            .await
            .is_err()
    );
    settings::change(&mut conn, "no.such", None).await.unwrap();
    assert!(!settings::all(&pool).await.unwrap().contains_key("no.such"));
}
