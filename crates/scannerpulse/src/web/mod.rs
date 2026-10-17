//! `scannerpulse web`: the HTTP interface. It reads the database and the
//! archive and never writes to the archive, so any number of hosts can run it
//! side by side behind a load balancer.

mod admin;
mod audio;
pub mod auth;
mod calls;
pub mod client;
mod data;
mod edit;
mod embedded;
mod error;
mod info;
mod jobs;

/// The jobs the admin page and `jobs list` show.
pub use jobs::LISTED as JOBS_LISTED;
pub mod live;
pub mod oidc;
mod sheet;
mod transcription;
pub mod viewers;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};

pub use error::ApiError;

/// The exports, of either kind, that may run at once on this host.
pub const EXPORTS: usize = 2;
/// The password sign-ins that may hash at once on this host.
pub const SIGN_INS: usize = 4;

/// How long `/healthz` waits for the database before it answers 503.
pub const HEALTH_TIMEOUT: Duration = Duration::from_secs(4);

/// What `web` reads from the environment until the host configuration file
/// exists.
#[derive(Clone)]
pub struct Config {
    pub database: PgConnectOptions,
    pub listen: SocketAddr,
    /// The frontend's built files, served for every path the API does not take.
    pub static_dir: Option<PathBuf>,
    /// The host's name in the Active Viewers list.
    pub host: String,
    /// The OIDC client's secret; secrets never go in the database.
    pub oidc_secret: Option<String>,
    /// The certificate and key `web` serves HTTPS with; without them it
    /// serves plain HTTP, on a loopback address only.
    pub tls: Option<Tls>,
}

/// A certificate and its private key, as PEM files.
#[derive(Clone, Debug)]
pub struct Tls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// ScannerPulse serves plain HTTP only on this machine's own loopback
/// address, where a browser treats the page as secure and keeps the session's
/// `Secure` cookie; on any other address it serves HTTPS.
pub fn check_listen(listen: SocketAddr, tls: bool) -> Result<(), String> {
    if tls || listen.ip().is_loopback() {
        Ok(())
    } else {
        Err(format!(
            "SCANNERPULSE_WEB_LISTEN is {listen}, an address other machines reach: \
             ScannerPulse serves HTTPS there; set SCANNERPULSE_TLS_CERT and \
             SCANNERPULSE_TLS_KEY (scannerpulse cert make writes a pair)"
        ))
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database", &self.database)
            .field("listen", &self.listen)
            .field("static_dir", &self.static_dir)
            .field("host", &self.host)
            .field(
                "oidc_secret",
                &self.oidc_secret.as_ref().map(|_| "<hidden>"),
            )
            .field("tls", &self.tls)
            .finish()
    }
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
        let database = url
            .parse::<PgConnectOptions>()
            .map_err(|error| format!("DATABASE_URL is not a database URL: {error}"))?;
        let listen = std::env::var("SCANNERPULSE_WEB_LISTEN")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let listen = listen.parse().map_err(|_| {
            format!(
                "SCANNERPULSE_WEB_LISTEN is {listen:?}: write an address such as 127.0.0.1:8080"
            )
        })?;
        let static_dir = std::env::var_os("SCANNERPULSE_WEB_STATIC").map(PathBuf::from);
        let host = std::env::var("SCANNERPULSE_HOST")
            .or_else(|_| {
                std::fs::read_to_string("/proc/sys/kernel/hostname").map(|n| n.trim().to_string())
            })
            .ok()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "web".to_string());
        let oidc_secret = std::env::var("SCANNERPULSE_OIDC_CLIENT_SECRET")
            .ok()
            .filter(|secret| !secret.is_empty());
        let path = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let tls = match (path("SCANNERPULSE_TLS_CERT"), path("SCANNERPULSE_TLS_KEY")) {
            (Some(cert), Some(key)) => Some(Tls { cert, key }),
            (None, None) => None,
            _ => {
                return Err(
                    "set both SCANNERPULSE_TLS_CERT and SCANNERPULSE_TLS_KEY, or neither".into(),
                );
            }
        };
        check_listen(listen, tls.is_some())?;
        Ok(Config {
            database,
            listen,
            static_dir,
            host,
            oidc_secret,
            tls,
        })
    }
}

/// What every handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    /// `/healthz` opens its own connection with these, so a pool kept busy by
    /// slow requests does not make a healthy host look down.
    pub database: PgConnectOptions,
    /// The exact counts that may run at once on this host.
    pub exact_counts: Arc<tokio::sync::Semaphore>,
    /// The frontend's built files; `None` serves none.
    pub static_dir: Option<PathBuf>,
    /// The host's live feed: every open page listens to it.
    pub live: live::Feed,
    /// Becomes `true` when the host stops; every open live page then closes.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// The exports that may run at once on this host.
    pub exports: Arc<tokio::sync::Semaphore>,
    /// The password sign-ins hashing now on this host.
    pub sign_ins: Arc<tokio::sync::Semaphore>,
    /// This host's name in the viewers' rows.
    pub host: String,
    /// The OIDC client's secret, when sign-in through the provider is set up.
    pub oidc_secret: Option<String>,
    /// Asks the OIDC provider; it follows no redirect.
    pub http: reqwest::Client,
    /// The pages this host holds open.
    pub viewers: viewers::Open,
    /// Each address's transcript saves in its current minute.
    pub saves: edit::Saves,
    /// The traffic report, with the time it was made and the time zone it
    /// was made for.
    pub traffic:
        Arc<tokio::sync::Mutex<Option<(std::time::Instant, chrono_tz::Tz, serde_json::Value)>>>,
    /// The filter choices, with the time they were read.
    pub filters: Arc<tokio::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>>>,
}

impl AppState {
    pub fn new(pool: PgPool, database: PgConnectOptions) -> AppState {
        AppState {
            pool,
            database,
            exact_counts: Arc::new(tokio::sync::Semaphore::new(calls::EXACT_COUNTS)),
            filters: Arc::new(tokio::sync::Mutex::new(None)),
            traffic: Arc::new(tokio::sync::Mutex::new(None)),
            saves: edit::Saves::default(),
            host: "web".to_string(),
            oidc_secret: None,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            viewers: viewers::Open::default(),
            exports: Arc::new(tokio::sync::Semaphore::new(EXPORTS)),
            sign_ins: Arc::new(tokio::sync::Semaphore::new(SIGN_INS)),
            live: live::feed(),
            shutdown: tokio::sync::watch::channel(false).0,
            static_dir: None,
        }
    }
}

/// The routes, without the listener: the tests call it directly.
pub fn router(state: AppState) -> Router {
    let files = state.static_dir.clone();
    let routes = Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz))
        .route("/api/calls", get(calls::list))
        .route("/api/calls/{id}", get(info::call))
        .route("/api/calls/{id}/transcript", patch(edit::save))
        .route("/api/filters", get(info::filters))
        .route("/api/favorites", get(info::favorites))
        .route("/api/status", get(info::status))
        .route("/api/labels/todo", get(info::labels_todo))
        .route("/api/auth/login", post(auth::login))
        .route("/api/auth/logout", post(auth::logout))
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/methods", get(auth::methods))
        .route("/api/auth/oidc", get(oidc::start))
        .route("/api/auth/oidc/callback", get(oidc::callback))
        .route(
            "/api/admin/accounts",
            get(admin::accounts_list).post(admin::accounts_add),
        )
        .route(
            "/api/admin/accounts/{login}/password",
            post(admin::accounts_password),
        )
        .route(
            "/api/admin/accounts/{login}/disable",
            post(admin::accounts_disable),
        )
        .route("/api/admin/settings", get(admin::settings_list))
        .route("/api/admin/viewers", get(viewers::list))
        .route("/api/admin/rescan", post(admin::rescan))
        .route("/api/admin/status", get(admin::status))
        .route("/api/admin/reports/traffic", get(admin::traffic))
        .route("/api/admin/talkgroups", get(transcription::talkgroups))
        .route(
            "/api/admin/talkgroups/{id}/transcribe",
            axum::routing::put(transcription::transcribe),
        )
        .route("/api/admin/queue/retry", post(transcription::retry_failed))
        .route("/api/admin/data/uncorrected", get(data::uncorrected))
        .route("/api/admin/talkgroups/{id}/rename", post(data::rename))
        .route(
            "/api/admin/talkgroups/{id}/correct",
            get(data::correct_preview).post(data::correct_start),
        )
        .route(
            "/api/admin/talkgroups/{id}/merge",
            get(data::merge_preview).post(data::merge_start),
        )
        .route(
            "/api/admin/talkgroups/{id}/purge",
            get(data::purge_preview).post(data::purge_start),
        )
        .route("/api/admin/asr/sets", get(transcription::sets))
        .route("/api/admin/asr/report", get(transcription::report))
        .route("/api/admin/jobs", get(jobs::list))
        .route(
            "/api/admin/jobs/queue",
            get(jobs::queue_preview).post(jobs::queue_start),
        )
        .route("/api/admin/jobs/asr-run", post(jobs::asr_start))
        .route("/api/admin/jobs/{id}", get(jobs::one))
        .route("/api/admin/jobs/{id}/stop", post(jobs::stop))
        .route(
            "/api/admin/favorites/{name}",
            axum::routing::put(admin::favorites_set).delete(admin::favorites_remove),
        )
        .route(
            "/api/admin/settings/{key}",
            axum::routing::put(admin::settings_set).delete(admin::settings_unset),
        )
        .route(
            "/api/admin/accounts/{login}/enable",
            post(admin::accounts_enable),
        )
        .route("/audio/{id}", get(audio::one))
        .route("/api/export/audio", post(audio::export))
        .route("/api/export/xlsx", get(sheet::export))
        .route("/api/live", get(live::socket))
        .route("/api/{*rest}", axum::routing::any(no_route))
        .with_state(state);
    // Every other path is the frontend's: a file it built, or its page,
    // which reads the path itself; from the folder when one is named,
    // otherwise from the files embedded at build time, by the same rules.
    let routes = match files {
        Some(dir) => {
            routes.fallback(move |method, uri| embedded::serve_folder(dir.clone(), method, uri))
        }
        None => routes.fallback(embedded::serve),
    };
    // Last, so that it also sees the answers no handler wrote: a refused
    // parameter, a wrong method, an unknown path, a range past the end.
    routes.layer(axum::middleware::map_response(json_errors))
}

/// The most of a plain error's text that is kept as its detail.
const ERROR_TEXT: usize = 1024;

/// Every error the API gives is `{"error", "detail"}`: an answer of 400 or
/// more that is not JSON becomes one, with its status and headers kept.
async fn json_errors(response: Response) -> Response {
    let json = response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|t| t.as_bytes().starts_with(b"application/json"));
    let status = response.status();
    if status.as_u16() < 400 || json {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    let detail = match axum::body::to_bytes(body, ERROR_TEXT).await {
        Ok(text) => String::from_utf8_lossy(&text).into_owned(),
        Err(_) => String::new(),
    };
    let error = status.canonical_reason().unwrap_or("error").to_lowercase();
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let body = serde_json::json!({ "error": error, "detail": detail }).to_string();
    Response::from_parts(parts, Body::from(body))
}

async fn no_route() -> ApiError {
    ApiError::NotFound
}

/// Serves until `shutdown` completes.
pub async fn run(
    config: Config,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .acquire_timeout(Duration::from_secs(5))
        .connect_lazy_with(config.database.clone());
    let mut state = AppState::new(pool, config.database);
    state.static_dir = config.static_dir;
    state.host = config.host;
    state.oidc_secret = config.oidc_secret;
    live::spawn(state.pool.clone(), state.live.clone());
    viewers::spawn(state.clone());
    let stop_live = state.shutdown.clone();
    let app = router(state).into_make_service_with_connect_info::<SocketAddr>();
    let Some(tls) = config.tls else {
        let listener = tokio::net::TcpListener::bind(config.listen)
            .await
            .map_err(|error| format!("cannot listen on {}: {error}", config.listen))?;
        crate::log::info(format!("web listening on {}", config.listen));
        // Graceful shutdown waits for open connections, and a live page never
        // ends by itself: it is told to close first.
        return axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.await;
                stop_live.send_replace(true);
            })
            .await
            .map_err(|error| error.to_string());
    };
    let rustls = axum_server::tls_rustls::RustlsConfig::from_pem_file(&tls.cert, &tls.key)
        .await
        .map_err(|error| {
            format!(
                "cannot read the certificate {} and its key {}: {error}",
                tls.cert.display(),
                tls.key.display()
            )
        })?;
    let handle = axum_server::Handle::new();
    let stopper = handle.clone();
    tokio::spawn(async move {
        shutdown.await;
        stop_live.send_replace(true);
        stopper.graceful_shutdown(Some(GRACE));
    });
    crate::log::info(format!("web listening on {} (HTTPS)", config.listen));
    axum_server::bind_rustls(config.listen, rustls)
        .handle(handle)
        .serve(app)
        .await
        .map_err(|error| format!("cannot serve on {}: {error}", config.listen))
}

/// How long a stopping HTTPS server waits for open requests.
const GRACE: Duration = Duration::from_secs(10);

/// The process is up.
async fn health() -> &'static str {
    "ok"
}

/// The database answers on a connection of its own within `HEALTH_TIMEOUT`,
/// and it is the primary: a standby cannot take a person's edit.
async fn healthz(State(state): State<AppState>) -> impl IntoResponse {
    let check = async {
        let mut conn = PgConnection::connect_with(&state.database).await?;
        let standby: bool = sqlx::query_scalar("SELECT pg_is_in_recovery()")
            .fetch_one(&mut conn)
            .await?;
        let _ = conn.close().await;
        Ok::<bool, sqlx::Error>(standby)
    };
    match tokio::time::timeout(HEALTH_TIMEOUT, check).await {
        Ok(Ok(false)) => (StatusCode::OK, "ok"),
        Ok(Ok(true)) => (StatusCode::SERVICE_UNAVAILABLE, "the database is a standby"),
        Ok(Err(_)) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "the database does not answer",
        ),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "the database is too slow"),
    }
}
