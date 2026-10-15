//! `scannerpulse web`: the HTTP interface. It reads the database and the
//! archive and never writes to the archive, so any number of hosts can run it
//! side by side behind a load balancer.

mod audio;
mod calls;
mod error;
mod info;
pub mod live;
mod sheet;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};

pub use error::ApiError;

/// The exports, of either kind, that may run at once on this host.
pub const EXPORTS: usize = 2;

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
        Ok(Config {
            database,
            listen,
            static_dir,
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
    /// The host's live feed: every open page listens to it.
    pub live: live::Feed,
    /// Becomes `true` when the host stops; every open live page then closes.
    pub shutdown: tokio::sync::watch::Sender<bool>,
    /// The exports that may run at once on this host.
    pub exports: Arc<tokio::sync::Semaphore>,
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
            exports: Arc::new(tokio::sync::Semaphore::new(EXPORTS)),
            live: live::feed(),
            shutdown: tokio::sync::watch::channel(false).0,
        }
    }
}

/// The routes, without the listener: the tests call it directly.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/healthz", get(healthz))
        .route("/api/calls", get(calls::list))
        .route("/api/calls/{id}", get(info::call))
        .route("/api/filters", get(info::filters))
        .route("/api/favorites", get(info::favorites))
        .route("/api/status", get(info::status))
        .route("/api/labels/todo", get(info::labels_todo))
        .route("/audio/{id}", get(audio::one))
        .route("/api/export/audio", post(audio::export))
        .route("/api/export/xlsx", get(sheet::export))
        .route("/api/live", get(live::socket))
        .with_state(state)
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
    let state = AppState::new(pool, config.database);
    live::spawn(state.pool.clone(), state.live.clone());
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
