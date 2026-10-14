//! The administrator's settings that ingest reads from the `settings` table.

use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use chrono_tz::Tz;
use serde_json::Value;

/// The keys that ingest reads.
pub const KEYS: [&str; 4] = [
    "time_zone",
    "ingest.rescan_seconds",
    "ingest.missing_source_grace_seconds",
    "ingest.done_rows_kept_days",
];

/// What ingest needs from the settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestSettings {
    /// The zone of the local times in the recorder's file names.
    pub time_zone: Tz,
    /// How often the watcher lists every source, whether or not it had events.
    pub rescan: Duration,
    /// How long a waiting file may be missing from its source before ingest
    /// looks for it in the archive.
    pub missing_source_grace: Duration,
    /// How long the row of a filed file is kept.
    pub done_rows_kept_days: u32,
}

#[derive(Debug)]
pub enum SettingsError {
    /// A setting that has no default is not set.
    Missing(&'static str),
    /// A setting holds a value that ingest cannot use.
    Invalid {
        key: &'static str,
        why: String,
    },
    Database(sqlx::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Missing(key) => write!(f, "the setting {key} is not set"),
            SettingsError::Invalid { key, why } => {
                write!(f, "the setting {key} is not valid: {why}")
            }
            SettingsError::Database(error) => write!(f, "the settings could not be read: {error}"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Reads ingest's settings. A key that is not set takes its default.
pub async fn load(db: impl sqlx::PgExecutor<'_>) -> Result<IngestSettings, SettingsError> {
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key = ANY($1)")
            .bind(&KEYS[..])
            .fetch_all(db)
            .await
            .map_err(SettingsError::Database)?;
    from_values(&rows.into_iter().collect())
}

/// The settings from their stored values. A key that is not set takes its default.
pub fn from_values(values: &HashMap<String, Value>) -> Result<IngestSettings, SettingsError> {
    let time_zone = match values.get("time_zone") {
        None => return Err(SettingsError::Missing("time_zone")),
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                invalid(
                    "time_zone",
                    format!("{value} is not a zone name such as \"America/Chicago\""),
                )
            })?;
            name.parse::<Tz>()
                .map_err(|_| invalid("time_zone", format!("{name:?} is not a known time zone")))?
        }
    };
    let rescan = whole(values, "ingest.rescan_seconds", 10)?;
    let grace = whole(values, "ingest.missing_source_grace_seconds", 900)?;
    // The watcher's rescan is what keeps a waiting file from being judged missing.
    if rescan >= grace {
        return Err(invalid(
            "ingest.rescan_seconds",
            format!("{rescan} must be below ingest.missing_source_grace_seconds, which is {grace}"),
        ));
    }
    let kept = whole(values, "ingest.done_rows_kept_days", 7)?;
    Ok(IngestSettings {
        time_zone,
        rescan: Duration::from_secs(rescan.into()),
        missing_source_grace: Duration::from_secs(grace.into()),
        done_rows_kept_days: kept,
    })
}

/// A whole number from 1 up, or `default` when the key is not set.
fn whole(
    values: &HashMap<String, Value>,
    key: &'static str,
    default: u32,
) -> Result<u32, SettingsError> {
    let Some(value) = values.get(key) else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid(key, format!("{value} is not a whole number from 1 up")))
}

fn invalid(key: &'static str, why: String) -> SettingsError {
    SettingsError::Invalid { key, why }
}
