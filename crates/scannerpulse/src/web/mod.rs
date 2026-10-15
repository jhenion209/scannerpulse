//! `scannerpulse web`: the HTTP interface. It reads the database and the
//! archive and never writes to the archive, so any number of hosts can run it
//! side by side behind a load balancer.

mod admin;
mod audio;
pub mod auth;
mod calls;
pub mod client;
mod edit;
mod error;
mod info;
pub mod live;
mod sheet;
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
#[derive(Debug, Clone)]
pub struct Config {
    pub database: PgConnectOptions,
    pub listen: SocketAddr,
    /// The frontend's built files, served for every path the API does not take.
    pub static_dir: Option<PathBuf>,
    /// The host's name in the Active Viewers list.
    pub host: String,
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
        Ok(Config {
            database,
            listen,
            static_dir,
            host,
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
    /// The pages this host holds open.
    pub viewers: viewers::Open,
    /// Each address's transcript saves in its current minute.
    pub saves: edit::Saves,
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
            saves: edit::Saves::default(),
            host: "web".to_string(),
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
    // which reads the path itself.
    let routes = match files {
        Some(dir) => routes.fallback_service(
            tower_http::services::ServeDir::new(&dir)
                .fallback(tower_http::services::ServeFile::new(dir.join("index.html"))),
        ),
        None => routes,
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
    live::spawn(state.pool.clone(), state.live.clone());
    viewers::spawn(state.clone());
    let stop_live = state.shutdown.clone();
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .map_err(|error| format!("cannot listen on {}: {error}", config.listen))?;
    crate::log::info(format!("web listening on {}", config.listen));
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    // Graceful shutdown waits for open connections, and a live page never
    // ends by itself: it is told to close first.
    .with_graceful_shutdown(async move {
        shutdown.await;
        stop_live.send_replace(true);
    })
    .await
    .map_err(|error| error.to_string())
}

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
