//! The admin routes. Each needs a signed-in admin (and, for a change, the
//! session's CSRF token), and each calls the same library function as its
//! command-line twin.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Admin;
use super::{ApiError, AppState};
use crate::accounts::{self, AccountError};

impl From<AccountError> for ApiError {
    fn from(error: AccountError) -> ApiError {
        match error {
            AccountError::Database(error) => ApiError::Database(error),
            AccountError::NoSuch(_) => ApiError::Missing(error.to_string()),
            AccountError::Exists(_) | AccountError::LastAdmin => {
                ApiError::Conflict(error.to_string())
            }
            AccountError::BadName(_) | AccountError::BadPassword(_) => {
                ApiError::BadRequest(error.to_string())
            }
        }
    }
}

/// Logs an admin's change with the admin's name.
fn record(admin: &Admin, what: String) {
    crate::log::info(format!("admin {}: {what}", admin.0.login));
}

/// The password's hash, made on the blocking threads.
async fn hashed(password: String) -> Result<String, ApiError> {
    tokio::task::spawn_blocking(move || accounts::new_hash(&password))
        .await
        .map_err(|error| ApiError::Internal(format!("password hash: {error}")))?
        .map_err(ApiError::from)
}

pub async fn accounts_list(
    _: Admin,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(accounts::list(&state.pool).await?)))
}

#[derive(Debug, Deserialize)]
pub struct NewAccount {
    pub login: String,
    pub password: String,
}

pub async fn accounts_add(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<NewAccount>,
) -> Result<StatusCode, ApiError> {
    accounts::check_login(&body.login)?;
    let hash = hashed(body.password).await?;
    let mut conn = state.pool.acquire().await?;
    accounts::add_hashed(&mut conn, &body.login, &hash).await?;
    record(&admin, format!("added the admin {}", body.login));
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
pub struct NewPassword {
    pub password: String,
}

pub async fn accounts_password(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
    Json(body): Json<NewPassword>,
) -> Result<StatusCode, ApiError> {
    let hash = hashed(body.password).await?;
    let mut conn = state.pool.acquire().await?;
    accounts::set_hash(&mut conn, &login, &hash).await?;
    record(&admin, format!("gave {login} a new password"));
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accounts_disable(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    accounts::disable(&mut conn, &login).await?;
    record(&admin, format!("disabled {login}"));
    Ok(StatusCode::NO_CONTENT)
}

pub async fn accounts_enable(
    admin: Admin,
    State(state): State<AppState>,
    Path(login): Path<String>,
) -> Result<StatusCode, ApiError> {
    accounts::enable(&state.pool, &login).await?;
    record(&admin, format!("enabled {login}"));
    Ok(StatusCode::NO_CONTENT)
}

impl From<crate::settings::SettingsError> for ApiError {
    fn from(error: crate::settings::SettingsError) -> ApiError {
        use crate::settings::SettingsError;
        match error {
            SettingsError::Database(error) => ApiError::Database(error),
            SettingsError::Unknown(_) => ApiError::Missing(error.to_string()),
            SettingsError::Missing(_) | SettingsError::Invalid { .. } => {
                ApiError::BadRequest(error.to_string())
            }
        }
    }
}

/// `GET /api/admin/status`: the Status tab.
pub async fn status(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut status = crate::status::read(&state.pool).await?;
    let found = state
        .machine
        .layout
        .as_deref()
        .and_then(crate::layout::Layout::read);
    status["referee"] = crate::layout::referee(found.as_ref()).await;
    Ok(Json(status))
}

/// How long a traffic report is kept before it is made again.
pub const TRAFFIC_FOR: std::time::Duration = std::time::Duration::from_secs(600);

/// `GET /api/admin/machine`: this machine's certificate (the one served,
/// and Let's Encrypt's renewals when it gives it), its backups' folder, the
/// age of each kind's newest backup, and which kinds push; the admin page
/// shows how to change each with a command run here as root.
pub async fn machine(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let found = match &state.certs {
        Some(certs) => Some(crate::cert::found(&certs.current())),
        None => state.machine.cert.as_deref().map(crate::cert::read),
    };
    let certificate = found.map(|found| match found {
        Ok(found) => serde_json::json!({
            "names": found.names,
            "not_before": found.not_before,
            "not_after": found.not_after,
            "self_signed": found.self_signed,
        }),
        Err(why) => serde_json::json!({ "error": why }),
    });
    let acme = state.machine.acme.as_ref().map(|acme| {
        let renewal = state
            .renewal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        serde_json::json!({
            "proof": acme.proof.name(),
            "names": acme.names,
            "tried": renewal.tried,
            "renewed": renewal.renewed,
            "error": renewal.error,
        })
    });
    let backups = match &state.machine.backups {
        None => Value::Null,
        Some(folder) => {
            let newest = crate::backup::newest(&state.pool).await?;
            let kinds: serde_json::Map<String, Value> = crate::backup::KINDS
                .iter()
                .map(|kind| {
                    let at = newest.iter().find(|(k, _)| k == kind).map(|(_, at)| *at);
                    let push = state.machine.pushes.contains(kind);
                    (
                        kind.to_string(),
                        serde_json::json!({ "newest": at, "push": push }),
                    )
                })
                .collect();
            serde_json::json!({ "folder": folder, "kinds": kinds })
        }
    };
    Ok(Json(serde_json::json!({
        "host": state.host,
        // Setup writes the backups' folder only for a database it installed.
        "database": if state.machine.backups.is_some() { "here" } else { "theirs" },
        "certificate": certificate,
        "acme": acme,
        "backups": backups,
    })))
}

/// `GET /api/admin/layout`: the install's machines, their roles and how
/// they answer (`layout::report`).
pub async fn layout(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let found = state
        .machine
        .layout
        .as_deref()
        .and_then(crate::layout::Layout::read);
    Ok(Json(
        crate::layout::report(&state.pool, found.as_ref(), &state.machine.nodes).await?,
    ))
}

/// The conversion's plan, beside the install's layout file.
fn convert_plan(state: &AppState) -> Option<crate::convert::Convert> {
    let path = state
        .machine
        .layout
        .as_deref()?
        .with_file_name("convert.json");
    crate::convert::Convert::read(&path).ok()
}

/// `GET /api/admin/convert`: the guide through the install's conversion to
/// high availability (`convert::guide`), once setup's planner has saved its
/// plan; `{"plan": null}` before.
pub async fn convert(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let Some(plan) = convert_plan(&state) else {
        return Ok(Json(json!({ "plan": null })));
    };
    let layout = state
        .machine
        .layout
        .as_deref()
        .and_then(crate::layout::Layout::read);
    let report = crate::layout::report(&state.pool, layout.as_ref(), &state.machine.nodes).await?;
    let mut conn = state.pool.acquire().await?;
    let archive = crate::setup::archive_move::current(&mut conn)
        .await?
        .map(|path| path.display().to_string());
    drop(conn);
    let mut asked = Vec::new();
    for kind in crate::agent::Kind::ALL {
        if let Some(request) = crate::agent::last(&state.pool, kind).await? {
            asked.push((kind, request.finished_at.and(request.ok)));
        }
    }
    let seen = crate::convert::Seen {
        archive,
        layout,
        asked,
        up: report["machines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|machine| machine["up"] == true)
            .filter_map(|machine| machine["host"].as_str().map(String::from))
            .collect(),
        standby_current: crate::convert::standby_current(&report["database"]),
        referee_answers: report["referee"]["answers"].as_bool(),
    };
    Ok(Json(json!({
        "plan": plan,
        "guide": crate::convert::guide(&plan, &seen),
    })))
}

/// `GET /api/admin/reports/traffic`: the traffic report, kept for ten
/// minutes on each host, since it reads months of calls; a changed time zone
/// makes it again.
pub async fn traffic(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let zone = crate::settings::load(&state.pool).await?.time_zone;
    let mut kept = state.traffic.lock().await;
    if let Some((when, kept_zone, value)) = kept.as_ref()
        && *kept_zone == zone
        && when.elapsed() < TRAFFIC_FOR
    {
        return Ok(Json(value.clone()));
    }
    let mut conn = state.pool.acquire().await?;
    let report = crate::reports::traffic(&mut conn, zone, chrono::Utc::now()).await?;
    let value = json!(report);
    *kept = Some((std::time::Instant::now(), zone, value.clone()));
    Ok(Json(value))
}

/// `POST /api/admin/rescan`: every running ingest lists its sources at once.
pub async fn rescan(admin: Admin, State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    crate::ingest::request_rescan(&state.pool).await?;
    record(&admin, "asked for a rescan of the sources".into());
    Ok(StatusCode::ACCEPTED)
}

/// Every setting: each stored one with the time it was changed, and each one
/// a part reads that is not stored, with neither (`value` and `updated_at`
/// null). A stored row that no part reads (`known` false) can only be removed.
pub async fn settings_list(
    _: Admin,
    State(state): State<AppState>,
) -> Result<Json<Value>, ApiError> {
    let stored: Vec<(String, Value, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT key, value, updated_at FROM settings")
            .fetch_all(&state.pool)
            .await?;
    let mut rows: std::collections::BTreeMap<String, Value> = crate::settings::known()
        .map(|key| {
            let row = json!({"key": key, "value": null, "updated_at": null, "known": true});
            (key.to_string(), row)
        })
        .collect();
    for (key, value, updated_at) in stored {
        let known = crate::settings::known().any(|k| k == key);
        let row = json!({"key": key, "value": value, "updated_at": updated_at, "known": known});
        rows.insert(key, row);
    }
    Ok(Json(json!(rows.into_values().collect::<Vec<_>>())))
}

#[derive(Debug, Deserialize)]
pub struct NewValue {
    pub value: Value,
}

pub async fn settings_set(
    admin: Admin,
    State(state): State<AppState>,
    Path(key): Path<String>,
    Json(body): Json<NewValue>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::settings::change(&mut conn, &key, Some(&body.value)).await?;
    record(&admin, format!("set {key} to {}", body.value));
    forget_choices(&state, &key).await;
    Ok(StatusCode::NO_CONTENT)
}

/// A new system order shows in this host's filter choices at once; another
/// host's kept copy runs out within `info::FILTERS_FOR`.
async fn forget_choices(state: &AppState, key: &str) {
    if key == "web.system_order" {
        *state.filters.lock().await = None;
    }
}

pub async fn settings_unset(
    admin: Admin,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::settings::change(&mut conn, &key, None).await?;
    record(&admin, format!("removed {key}"));
    forget_choices(&state, &key).await;
    Ok(StatusCode::NO_CONTENT)
}

impl From<crate::favorites::FavoriteError> for ApiError {
    fn from(error: crate::favorites::FavoriteError) -> ApiError {
        use crate::favorites::FavoriteError;
        match error {
            FavoriteError::Database(error) => ApiError::Database(error),
            FavoriteError::NoSuch(_) => ApiError::Missing(error.to_string()),
            _ => ApiError::BadRequest(error.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Favorite {
    pub talkgroups: Vec<i32>,
}

/// Creates the list, or replaces its talkgroups.
pub async fn favorites_set(
    admin: Admin,
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<Favorite>,
) -> Result<StatusCode, ApiError> {
    let mut conn = state.pool.acquire().await?;
    crate::favorites::set(&mut conn, &name, &body.talkgroups).await?;
    record(
        &admin,
        format!(
            "set the favorites {name:?} to {} talkgroups",
            body.talkgroups.len()
        ),
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn favorites_remove(
    admin: Admin,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    crate::favorites::remove(&state.pool, &name).await?;
    record(&admin, format!("removed the favorites {name:?}"));
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/admin/updates`: the version this web runs and each
/// ScannerPulse machine's, the newest release the root helper's daily check
/// found (and whether the check is on), the helper itself, the newest
/// request of each kind an admin made of it, with what it said, and whether
/// one waits or runs.
pub async fn updates(_: Admin, State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let newest: Option<(String, String, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as("SELECT version, notes, checked_at FROM newest_release")
            .fetch_optional(&state.pool)
            .await?;
    let helper: Option<(String, String, f64)> = sqlx::query_as(
        "SELECT host, version, extract(epoch FROM now() - seen_at)::float8 FROM agents \
         ORDER BY seen_at DESC LIMIT 1",
    )
    .fetch_optional(&state.pool)
    .await?;
    let machines: Vec<(String, String)> =
        sqlx::query_as("SELECT host, version FROM machines ORDER BY host")
            .fetch_all(&state.pool)
            .await?;
    let running = crate::update::VERSION;
    let mut requests = serde_json::Map::new();
    for kind in crate::agent::Kind::ALL {
        let last = crate::agent::last(&state.pool, kind).await?;
        requests.insert(kind.name().into(), json!(last));
    }
    Ok(Json(json!({
        "running": running,
        "machines": machines
            .into_iter()
            .map(|(host, version)| json!({"host": host, "version": version}))
            .collect::<Vec<_>>(),
        "newest": newest.map(|(version, notes, checked_at)| json!({
            "version": version,
            "newer": crate::update::newer(running, &version),
            "notes": notes,
            "checked_at": checked_at,
        })),
        "check": crate::agent::checks(&state.pool).await?,
        "helper": helper.map(|(host, version, ago)| json!({
            "host": host,
            "version": version,
            "seen_seconds_ago": ago.round(),
            "up": ago < crate::agent::SILENT_AFTER as f64,
        })),
        "requests": requests,
        "open": crate::agent::open(&state.pool).await?,
    })))
}

#[derive(Deserialize)]
pub struct HelperRequest {
    kind: String,
}

/// `POST /api/admin/helper`: asks the root helper for an update of every
/// machine (`update`), a switchover of the database machines (`switchover`)
/// or a step of the conversion the guide shows (the others, which need its
/// plan). Refused while another request waits or runs, or when no helper is
/// up.
pub async fn helper(
    admin: Admin,
    State(state): State<AppState>,
    Json(body): Json<HelperRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    use crate::agent::{Kind, Refused};
    let kind = Kind::parse(&body.kind).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "ask for one of: {}",
            Kind::ALL.map(Kind::name).join(", ")
        ))
    })?;
    if kind.converts() && convert_plan(&state).is_none() {
        return Err(ApiError::BadRequest(
            "no conversion is planned: run `sudo /usr/local/bin/scannerpulse setup` on this machine first".into(),
        ));
    }
    match crate::agent::ask(&state.pool, kind, &admin.0.login).await {
        Ok(id) => {
            record(&admin, format!("asked the root helper for request {id}: {}", kind.name()));
            Ok((StatusCode::ACCEPTED, Json(json!({"id": id}))))
        }
        Err(Refused::Busy) => Err(ApiError::Conflict(
            "the root helper is busy with another request: wait for it to finish".into(),
        )),
        Err(Refused::NoHelper) => Err(ApiError::Unavailable(
            "the root helper is not running on the first ScannerPulse machine: run `sudo /usr/local/bin/scannerpulse agent install` there once".into(),
        )),
        Err(Refused::Database(error)) => Err(error.into()),
    }
}
