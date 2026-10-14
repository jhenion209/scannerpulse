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
