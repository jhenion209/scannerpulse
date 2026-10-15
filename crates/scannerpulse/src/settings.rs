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

/// The keys of transcription.
pub const TRANSCRIPTION_KEYS: [&str; 8] = [
    "transcription.enabled",
    "transcription.servers",
    "transcription.model",
    "transcription.language",
    "transcription.min_duration_ms",
    "transcription.request_timeout_seconds",
    "transcription.prompt",
    "transcription.terms",
];

/// One transcription server: the URL that takes the requests, and how many
/// requests it takes at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub url: String,
    pub weight: u32,
}

/// How calls are transcribed, when transcription is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcription {
    pub servers: Vec<Server>,
    pub model: String,
    /// Sent when it is set.
    pub language: Option<String>,
    /// A call shorter than this is not queued.
    pub min_duration_ms: i32,
    pub request_timeout: Duration,
    /// The prompt and the term list as one prompt; `None` when both are empty.
    pub prompt: Option<String>,
}

/// Reads the transcription settings; `None` when transcription is off.
pub async fn load_transcription(
    db: impl sqlx::PgExecutor<'_>,
) -> Result<Option<Transcription>, SettingsError> {
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key = ANY($1)")
            .bind(&TRANSCRIPTION_KEYS[..])
            .fetch_all(db)
            .await
            .map_err(SettingsError::Database)?;
    transcription_from_values(&rows.into_iter().collect())
}

/// The transcription settings from their stored values; `None` when
/// transcription is off. When it is on, the servers, the model and the
/// minimum length must be set.
pub fn transcription_from_values(
    values: &HashMap<String, Value>,
) -> Result<Option<Transcription>, SettingsError> {
    let enabled = match values.get("transcription.enabled") {
        None => false,
        Some(value) => value.as_bool().ok_or_else(|| {
            invalid(
                "transcription.enabled",
                format!("{value} is not true or false"),
            )
        })?,
    };
    if !enabled {
        return Ok(None);
    }
    let servers = servers(values)?;
    let model = text(values, "transcription.model")?
        .ok_or(SettingsError::Missing("transcription.model"))?;
    let language = text(values, "transcription.language")?;
    let min_duration_ms = match values.get("transcription.min_duration_ms") {
        None => return Err(SettingsError::Missing("transcription.min_duration_ms")),
        Some(value) => value
            .as_u64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                invalid(
                    "transcription.min_duration_ms",
                    format!("{value} is not a whole number of milliseconds"),
                )
            })?,
    };
    let timeout = whole(values, "transcription.request_timeout_seconds", 120)?;
    let prompt = text(values, "transcription.prompt")?;
    let terms = match values.get("transcription.terms") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .and_then(|list| {
                list.iter()
                    .map(|term| term.as_str().map(|t| t.trim().to_string()))
                    .collect::<Option<Vec<String>>>()
            })
            .ok_or_else(|| {
                invalid(
                    "transcription.terms",
                    format!("{value} is not a list of words"),
                )
            })?,
    };
    Ok(Some(Transcription {
        servers,
        model,
        language,
        min_duration_ms,
        request_timeout: Duration::from_secs(timeout.into()),
        prompt: whisper_prompt(prompt.as_deref().unwrap_or(""), &terms),
    }))
}

/// The prompt that is sent: the prompt, then the terms separated by commas.
pub fn whisper_prompt(prompt: &str, terms: &[String]) -> Option<String> {
    let terms: Vec<&str> = terms
        .iter()
        .map(|term| term.trim())
        .filter(|term| !term.is_empty())
        .collect();
    let parts: Vec<String> = [prompt.trim().to_string(), terms.join(", ")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// A text setting; `None` when it is not set or empty.
fn text(
    values: &HashMap<String, Value>,
    key: &'static str,
) -> Result<Option<String>, SettingsError> {
    match values.get(key) {
        None => Ok(None),
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| invalid(key, format!("{value} is not text")))?
                .trim();
            Ok((!text.is_empty()).then(|| text.to_string()))
        }
    }
}

fn servers(values: &HashMap<String, Value>) -> Result<Vec<Server>, SettingsError> {
    const KEY: &str = "transcription.servers";
    let list = match values.get(KEY) {
        None => return Err(SettingsError::Missing(KEY)),
        Some(value) => value
            .as_array()
            .ok_or_else(|| invalid(KEY, format!("{value} is not a list of servers")))?,
    };
    if list.is_empty() {
        return Err(invalid(KEY, "the list is empty".into()));
    }
    let mut servers: Vec<Server> = Vec::new();
    for entry in list {
        let url = entry
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| url.starts_with("http://") || url.starts_with("https://"))
            .ok_or_else(|| invalid(KEY, format!("{entry} has no http:// or https:// url")))?;
        let weight = match entry.get("weight") {
            None => 1,
            Some(weight) => weight
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    invalid(
                        KEY,
                        format!("{entry} has a weight that is not a whole number from 1 up"),
                    )
                })?,
        };
        if servers.iter().any(|s| s.url == url) {
            return Err(invalid(KEY, format!("{url} is listed twice")));
        }
        servers.push(Server {
            url: url.to_string(),
            weight,
        });
    }
    Ok(servers)
}
