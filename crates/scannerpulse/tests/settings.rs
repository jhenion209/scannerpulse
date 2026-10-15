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
