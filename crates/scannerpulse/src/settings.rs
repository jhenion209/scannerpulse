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
    /// No part of ScannerPulse reads a setting of this name.
    Unknown(String),
    Database(sqlx::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Missing(key) => write!(f, "the setting {key} is not set"),
            SettingsError::Invalid { key, why } => {
                write!(f, "the setting {key} is not valid: {why}")
            }
            SettingsError::Unknown(key) => {
                write!(f, "no part of ScannerPulse reads a setting {key:?}")
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
    if !flag(values, "transcription.enabled")? {
        return Ok(None);
    }
    let servers = servers(values)?;
    let model = text(values, "transcription.model")?
        .ok_or(SettingsError::Missing("transcription.model"))?;
    let language = text(values, "transcription.language")?;
    if let Some(code) = &language
        && (!(2..=3).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_lowercase()))
    {
        return Err(invalid(
            "transcription.language",
            format!("{code} is not a code of 2 or 3 lowercase letters"),
        ));
    }
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

/// A transcription server's URL: http or https, and no user name, password,
/// query or fragment in it, because settings are kept in the database and a
/// key is not.
pub fn server_url_ok(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
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
    for (index, entry) in list.iter().enumerate() {
        // Name the entry by position: its text may hold a key, and a refusal
        // is shown and logged.
        let at = index + 1;
        if entry
            .as_object()
            .is_some_and(|fields| fields.keys().any(|k| k != "url" && k != "weight"))
        {
            return Err(invalid(
                KEY,
                format!(
                    "server {at} has a field other than url and weight; a key goes in SCANNERPULSE_TRANSCRIPTION_API_KEY"
                ),
            ));
        }
        let url = entry
            .get("url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|url| server_url_ok(url))
            .ok_or_else(|| {
                invalid(
                    KEY,
                    format!(
                        "server {at} has no http:// or https:// url without a user name, password, query or fragment; a key goes in SCANNERPULSE_TRANSCRIPTION_API_KEY"
                    ),
                )
            })?;
        let weight = match entry.get("weight") {
            None => 1,
            Some(weight) => weight
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .filter(|n| (1..=64).contains(n))
                .ok_or_else(|| {
                    invalid(
                        KEY,
                        format!("server {at} has a weight that is not a whole number from 1 to 64"),
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

/// The keys of the transcription improvement module.
pub const IMPROVEMENT_KEYS: [&str; 2] = ["improvement_module.enabled", "labeler_list.public"];

/// The improvement module's switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Improvement {
    /// Turns on the evaluation commands.
    pub enabled: bool,
    /// Whether the labelers' list is shown to visitors, not only to admins.
    pub labeler_list_public: bool,
}

/// Reads the improvement module's switches; both are off when not set.
pub async fn load_improvement(db: impl sqlx::PgExecutor<'_>) -> Result<Improvement, SettingsError> {
    let rows: Vec<(String, Value)> =
        sqlx::query_as("SELECT key, value FROM settings WHERE key = ANY($1)")
            .bind(&IMPROVEMENT_KEYS[..])
            .fetch_all(db)
            .await
            .map_err(SettingsError::Database)?;
    improvement_from_values(&rows.into_iter().collect())
}

/// The improvement module's switches from their stored values.
pub fn improvement_from_values(
    values: &HashMap<String, Value>,
) -> Result<Improvement, SettingsError> {
    Ok(Improvement {
        enabled: flag(values, "improvement_module.enabled")?,
        labeler_list_public: flag(values, "labeler_list.public")?,
    })
}

/// A true-or-false setting; false when it is not set.
fn flag(values: &HashMap<String, Value>, key: &'static str) -> Result<bool, SettingsError> {
    match values.get(key) {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| invalid(key, format!("{value} is not true or false"))),
    }
}

/// The settings that only the web service reads, each with its check.
pub const WEB_KEYS: [&str; 9] = [
    "maintenance.enabled",
    "auth.password_signin",
    "web.trusted_proxies",
    "web.system_order",
    "purge.holding_path",
    "oidc.issuer",
    "oidc.client_id",
    "oidc.admin_group",
    "oidc.redirect_url",
];

/// The settings every service reads: the syslog server its lines also go
/// to (`udp://host:port` or `tcp://host:port`; unset, the journal only).
pub const LOG_KEYS: [&str; 1] = ["logs.syslog"];

/// Keeps this process's syslog server in step with `logs.syslog`, read now
/// and each minute after; a database that does not answer leaves it as it
/// was.
pub fn follow_syslog(pool: sqlx::PgPool, app: &'static str) {
    tokio::spawn(async move {
        loop {
            let value: sqlx::Result<Option<Value>> =
                sqlx::query_scalar("SELECT value FROM settings WHERE key = 'logs.syslog'")
                    .fetch_optional(&pool)
                    .await;
            if let Ok(value) = value {
                crate::log::syslog(value.as_ref().and_then(Value::as_str), app);
            }
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    });
}

/// Every setting some part of the program reads.
pub fn known() -> impl Iterator<Item = &'static str> {
    KEYS.iter()
        .chain(&TRANSCRIPTION_KEYS)
        .chain(&IMPROVEMENT_KEYS)
        .chain(&WEB_KEYS)
        .chain(&LOG_KEYS)
        .copied()
}

/// An address of the OIDC provider or of this site's sign-in callback:
/// https, or http to this machine only (a provider under test).
pub fn oidc_url_ok(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        let local = matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        (u.scheme() == "https" || (u.scheme() == "http" && local))
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
    })
}

/// Whether the settings would be valid with `key` set to `value`, or
/// removed when `value` is `None`, given the other stored `values`: the same
/// checks as the parts that read them. A group that has a switch is checked
/// whole when its switch is on; while it is off, a value that is not set yet
/// does not stop another from being saved, but a malformed one does.
pub fn check(
    key: &str,
    value: Option<&Value>,
    values: &HashMap<String, Value>,
) -> Result<(), SettingsError> {
    let mut after = values.clone();
    match value {
        Some(value) => after.insert(key.to_string(), value.clone()),
        None => after.remove(key),
    };
    if KEYS.contains(&key) {
        return match from_values(&after) {
            // Ingest cannot start without a zone, but the others can be set first.
            Err(SettingsError::Missing("time_zone")) if key != "time_zone" => {
                after.insert("time_zone".into(), Value::from("UTC"));
                from_values(&after).map(drop)
            }
            other => other.map(drop),
        };
    }
    if TRANSCRIPTION_KEYS.contains(&key) {
        let enabled = flag(&after, "transcription.enabled")?;
        if enabled {
            return transcription_from_values(&after).map(drop);
        }
        // Off: each value present must still be one transcription can use.
        let mut trial = after;
        trial.insert("transcription.enabled".into(), Value::Bool(true));
        trial
            .entry("transcription.servers".into())
            .or_insert_with(|| serde_json::json!([{"url": "http://placeholder.invalid/"}]));
        trial
            .entry("transcription.model".into())
            .or_insert_with(|| Value::from("placeholder"));
        trial
            .entry("transcription.min_duration_ms".into())
            .or_insert_with(|| Value::from(0));
        return transcription_from_values(&trial).map(drop);
    }
    if IMPROVEMENT_KEYS.contains(&key) {
        return improvement_from_values(&after).map(drop);
    }
    if LOG_KEYS.contains(&key) {
        return match after.get(key) {
            None => Ok(()),
            Some(value) if value.as_str().and_then(crate::log::target).is_some() => Ok(()),
            Some(value) => Err(invalid(
                "logs.syslog",
                format!("{value} is not udp://host:port or tcp://host:port"),
            )),
        };
    }
    let Some(&key) = WEB_KEYS.iter().find(|k| **k == key) else {
        // A row no part reads (left by SQL or an older version) may be cleared.
        if value.is_none() {
            return Ok(());
        }
        return Err(SettingsError::Unknown(key.to_string()));
    };
    let Some(value) = after.get(key) else {
        return Ok(());
    };
    let ok = match key {
        "maintenance.enabled" | "auth.password_signin" => value.is_boolean(),
        "web.trusted_proxies" => value.as_array().is_some_and(|list| {
            list.iter().all(|v| {
                v.as_str()
                    .is_some_and(|ip| ip.trim().parse::<std::net::IpAddr>().is_ok())
            })
        }),
        "web.system_order" => value
            .as_array()
            .is_some_and(|list| list.iter().all(Value::is_string)),
        "oidc.issuer" | "oidc.redirect_url" => value.as_str().is_some_and(oidc_url_ok),
        "oidc.client_id" | "oidc.admin_group" => value
            .as_str()
            .is_some_and(|text| !text.trim().is_empty() && text.len() <= 200),
        _ => value
            .as_str()
            .is_some_and(|path| path.starts_with('/') && !path.split('/').any(|part| part == "..")),
    };
    if ok {
        Ok(())
    } else {
        let what = match key {
            "maintenance.enabled" | "auth.password_signin" => "true or false",
            "web.trusted_proxies" => "a list of addresses such as [\"192.0.2.10\"]",
            "web.system_order" => "a list of system names",
            "oidc.issuer" | "oidc.redirect_url" => {
                "an https:// address (http:// only to this machine), with no query"
            }
            "oidc.client_id" | "oidc.admin_group" => "text of 1 to 200 bytes",
            _ => "an absolute folder path without ..",
        };
        Err(invalid(key, format!("{value} is not {what}")))
    }
}

/// Every stored setting, by name.
pub async fn all(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<HashMap<String, Value>> {
    let rows: Vec<(String, Value)> = sqlx::query_as("SELECT key, value FROM settings")
        .fetch_all(db)
        .await?;
    Ok(rows.into_iter().collect())
}

/// Sets `key` to `value`, or removes it when `value` is `None`, after
/// `check`. Two changes at once are taken one after the other, so neither
/// is checked against values the other is replacing.
pub async fn change(
    conn: &mut sqlx::PgConnection,
    key: &str,
    value: Option<&Value>,
) -> Result<(), SettingsError> {
    use sqlx::Connection;
    let mut tx = conn.begin().await.map_err(SettingsError::Database)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('scannerpulse settings'))")
        .execute(&mut *tx)
        .await
        .map_err(SettingsError::Database)?;
    let values = all(&mut *tx).await.map_err(SettingsError::Database)?;
    check(key, value, &values)?;
    match value {
        Some(value) => sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES ($1, $2, now()) \
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
        )
        .bind(key)
        .bind(value),
        None => sqlx::query("DELETE FROM settings WHERE key = $1").bind(key),
    }
    .execute(&mut *tx)
    .await
    .map_err(SettingsError::Database)?;
    tx.commit().await.map_err(SettingsError::Database)?;
    Ok(())
}
