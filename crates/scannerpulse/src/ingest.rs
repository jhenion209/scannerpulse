//! `scannerpulse ingest`: the watcher on every host, the filer on the lease
//! holder only, the lease and path health, and a summary in the journal every
//! minute. A database outage pauses the work, which is tried again every 5 s;
//! only a setup error stops the process.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;

use crate::filer::{Filer, Outcome};
use crate::health::{self, Health, HealthTimings};
use crate::lease::{Lease, Timings};
use crate::log;
use crate::settings::{self, IngestSettings, SettingsError};
use crate::store::{self, Location, Source, Waiting, limited};
use crate::watcher::{self, Batch, Watcher};

/// How long ingest waits before it tries the database again.
pub const RETRY: Duration = Duration::from_secs(5);

/// How often the summary goes to the journal.
pub const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// How often the rows of filed files are pruned.
pub const PRUNE_EVERY: Duration = Duration::from_secs(3600);

/// The limit on one round of the lease's own connection: the connect with its
/// `SET`, a renewal, a hand-back to the preferred host or the release. A renewal every 5 s plus this
/// stays inside the lease's lifetime less its margin.
pub const ROUND_LIMIT: Duration = Duration::from_secs(3);

/// The default limit on every call through the pool.
pub const DB_LIMIT: Duration = Duration::from_secs(30);

/// How long a stop waits for the filer before it releases the lease anyway.
pub const STOP_FILER_LIMIT: Duration = Duration::from_secs(20);

/// Files per pass of the filer: a stop waits for one pass at most.
const PASS_SIZE: i64 = 20;

/// What a host needs to run ingest.
#[derive(Debug, Clone)]
pub struct Config {
    pub database: PgConnectOptions,
    /// The host's name in the lease.
    pub host: String,
    /// True on one host of a pair.
    pub preferred: bool,
    pub lease: Timings,
    pub health: HealthTimings,
    pub stable_for: Duration,
    /// The limit on every call through the pool: a call that takes longer
    /// counts as a database error.
    pub db_limit: Duration,
}

impl Config {
    /// The host's settings from the environment, until sub-project 5 sets the
    /// host configuration file: DATABASE_URL (the password in the file that
    /// PGPASSFILE names), SCANNERPULSE_HOST (default: the system's host name)
    /// and SCANNERPULSE_PREFERRED (`true` on one host of a pair).
    pub fn from_env() -> Result<Config, String> {
        let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
        let database = url
            .parse::<PgConnectOptions>()
            .map_err(|error| format!("DATABASE_URL is not a database URL: {error}"))?;
        let host = match std::env::var("SCANNERPULSE_HOST") {
            Ok(host) => host,
            Err(_) => std::fs::read_to_string("/proc/sys/kernel/hostname")
                .map(|name| name.trim().to_string())
                .map_err(|_| "the host name is unknown: set SCANNERPULSE_HOST")?,
        };
        if host.is_empty() || host.contains(char::is_whitespace) {
            return Err(format!("{host:?} cannot be the host's name in the lease"));
        }
        let preferred = match std::env::var("SCANNERPULSE_PREFERRED").as_deref() {
            Err(_) | Ok("false") => false,
            Ok("true") => true,
            Ok(other) => {
                return Err(format!(
                    "SCANNERPULSE_PREFERRED is {other:?}: write true or false"
                ));
            }
        };
        Ok(Config {
            database,
            host,
            preferred,
            lease: Timings::default(),
            health: HealthTimings::default(),
            stable_for: watcher::STABLE_FOR,
            db_limit: DB_LIMIT,
        })
    }
}

struct Setup {
    settings: IngestSettings,
    location: Location,
    sources: Vec<Source>,
}

enum SetupError {
    /// A setting, the archive or the sources are missing or wrong.
    Fatal(String),
    Database(sqlx::Error),
}

async fn setup(pool: &PgPool, limit: Duration) -> Result<Setup, SetupError> {
    let settings = match tokio::time::timeout(limit, settings::load(pool)).await {
        Ok(Ok(settings)) => settings,
        Ok(Err(SettingsError::Database(error))) => return Err(SetupError::Database(error)),
        Ok(Err(error)) => return Err(SetupError::Fatal(error.to_string())),
        Err(_) => return Err(SetupError::Database(store::timed_out(limit))),
    };
    let location = limited(limit, store::primary_location(pool))
        .await
        .map_err(SetupError::Database)?
        .ok_or_else(|| SetupError::Fatal("there is no primary location for the archive".into()))?;
    let sources = limited(limit, store::enabled_sources(pool))
        .await
        .map_err(SetupError::Database)?;
    if sources.is_empty() {
        return Err(SetupError::Fatal("no source is enabled".into()));
    }
    Ok(Setup {
        settings,
        location,
        sources,
    })
}

#[derive(Default)]
struct Counts {
    filed: AtomicU64,
    found: AtomicU64,
    rejected: AtomicU64,
}

/// Runs ingest until `stop` completes. Err only for a setup error. At a stop
/// the filer finishes its pass (for `STOP_FILER_LIMIT` at most), then the
/// lease is released, so that the other host takes it at its next round.
pub async fn run(config: Config, stop: impl Future<Output = ()> + Send) -> Result<(), String> {
    tokio::pin!(stop);
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(RETRY)
        // A connection given back after a cancelled call is pinged before it
        // goes back to the pool, with no limit of its own: on a dead socket it
        // would stay checked out until TCP gives up. Past 2 s it is closed.
        .after_release(|conn, _| {
            Box::pin(async move {
                match tokio::time::timeout(Duration::from_secs(2), conn.ping()).await {
                    Ok(answer) => answer.map(|()| true),
                    Err(_) => Err(store::timed_out(Duration::from_secs(2))),
                }
            })
        })
        .connect_lazy_with(config.database.clone());
    let setup = loop {
        tokio::select! {
            _ = &mut stop => {
                log::info("stopped");
                return Ok(());
            }
            attempt = setup(&pool, config.db_limit) => match attempt {
                Ok(setup) => break setup,
                Err(SetupError::Fatal(why)) => return Err(why),
                Err(SetupError::Database(error)) => log::warn(format!(
                    "the database cannot be read ({error}); trying again in 5 s"
                )),
            },
        }
        tokio::select! {
            _ = &mut stop => {
                log::info("stopped");
                return Ok(());
            }
            _ = sleep(RETRY) => {}
        }
    };
    let Setup {
        settings,
        location,
        sources,
    } = setup;
    log::info(format!(
        "ingest on {}: archive {}, {} source(s)",
        config.host,
        location.root.display(),
        sources.len()
    ));
    let lease = Arc::new(Lease::new(&config.host, config.preferred, config.lease));
    let healthy = Arc::new(AtomicBool::new(true));
    let busy = Arc::new(AtomicBool::new(false));
    let counts = Arc::new(Counts::default());
    let source_paths: Vec<&Path> = sources.iter().map(|s| s.path.as_path()).collect();
    let health = Health::new(
        &location.root,
        &source_paths,
        config.health,
        health::read_dir_probe,
    );
    let watcher = Watcher::new(sources.clone(), config.stable_for);
    let filer = Filer::new(
        pool.clone(),
        lease.clone(),
        location,
        sources,
        settings.time_zone,
        settings.missing_source_grace,
    )
    .with_db_limit(config.db_limit);

    let (stop_tx, stop_rx) = watch::channel(false);
    let (release_tx, release_rx) = watch::channel(false);
    // Health runs until the lease is released, so that the lease never renews
    // on a stale "healthy".
    let tasks = [
        (
            "health",
            tokio::spawn(health_loop(health, healthy.clone(), release_rx.clone())),
        ),
        (
            "watcher",
            tokio::spawn(watch_loop(
                pool.clone(),
                config.db_limit,
                watcher,
                settings.rescan,
                config.stable_for,
                stop_rx.clone(),
            )),
        ),
        (
            "summary",
            tokio::spawn(summary_loop(
                pool.clone(),
                config.db_limit,
                counts.clone(),
                lease.clone(),
                settings.done_rows_kept_days,
                stop_rx.clone(),
            )),
        ),
    ];
    let lease_task = tokio::spawn(lease_loop(
        config.database.clone(),
        lease.clone(),
        healthy.clone(),
        busy.clone(),
        release_rx,
    ));
    let filer_task = tokio::spawn(file_loop(filer, lease, healthy, busy, counts, stop_rx));

    stop.await;
    log::info("stopping: the filer finishes its pass");
    let _ = stop_tx.send(true);
    // A filer stuck on a hung mount must not keep the lease: once released,
    // the fence is false, so a filer thread that wakes later writes nothing.
    ended("filer", filer_task, STOP_FILER_LIMIT).await;
    let _ = release_tx.send(true);
    // One connect and one release.
    ended(
        "lease",
        lease_task,
        2 * ROUND_LIMIT + Duration::from_secs(1),
    )
    .await;
    for (name, task) in tasks {
        ended(name, task, Duration::from_secs(2)).await;
    }
    log::info("stopped");
    Ok(())
}

/// Waits for a task for `limit` at most, and names it when it did not end.
async fn ended(name: &str, task: tokio::task::JoinHandle<()>, limit: Duration) {
    if tokio::time::timeout(limit, task).await.is_err() {
        log::warn(format!("the {name} task did not end within {limit:?}"));
    }
}

/// Waits for `pause`; true when the stop came first.
async fn wait(stop: &mut watch::Receiver<bool>, pause: Duration) -> bool {
    if *stop.borrow() {
        return true;
    }
    tokio::select! {
        _ = stop.changed() => true,
        _ = sleep(pause) => *stop.borrow(),
    }
}

/// The lease's own connection, with short limits, so that a renewal never
/// waits behind a slow query.
async fn connect_lease(options: &PgConnectOptions) -> Result<PgConnection, String> {
    bounded(async {
        let mut conn = PgConnection::connect_with(options).await?;
        sqlx::query("SET statement_timeout = 2000")
            .execute(&mut conn)
            .await?;
        Ok(conn)
    })
    .await
}

/// One round of the lease's connection, under `ROUND_LIMIT`. The server's
/// `statement_timeout` cannot end a round when the server is gone.
async fn bounded<T>(round: impl Future<Output = sqlx::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(ROUND_LIMIT, round).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "the database did not answer within {} s",
            ROUND_LIMIT.as_secs()
        )),
    }
}

async fn lease_loop(
    options: PgConnectOptions,
    lease: Arc<Lease>,
    healthy: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    mut release: watch::Receiver<bool>,
) {
    let mut conn: Option<PgConnection> = None;
    let mut was_held = false;
    loop {
        if conn.is_none() {
            match connect_lease(&options).await {
                Ok(new) => conn = Some(new),
                Err(error) => log::warn(format!("the lease cannot reach the database: {error}")),
            }
        }
        if let Some(c) = conn.as_mut() {
            // A round that fails or times out drops the connection, which
            // closes its socket; the next round connects again.
            let result = match bounded(lease.tick(c, healthy.load(Ordering::SeqCst))).await {
                // The filer is between passes: the lease goes back to the preferred host.
                Ok(()) if lease.yielding() && !busy.load(Ordering::SeqCst) => {
                    bounded(lease.hand_over(c)).await.map(|done| {
                        if done {
                            log::info("the lease went back to the preferred host");
                        }
                    })
                }
                other => other,
            };
            if let Err(error) = result {
                log::warn(format!("the lease could not be renewed: {error}"));
                conn = None;
            }
        }
        let held = lease.held();
        if held != was_held {
            log::info(if held {
                "this host now holds the processing lease"
            } else {
                "this host no longer holds the processing lease"
            });
            was_held = held;
        }
        if wait(&mut release, lease.timings().renew).await {
            break;
        }
    }
    if conn.is_none() {
        conn = connect_lease(&options).await.ok();
    }
    match conn.as_mut() {
        Some(c) => match tokio::time::timeout(ROUND_LIMIT, lease.release(c)).await {
            Ok(Ok(())) => log::info("the lease is released"),
            Ok(Err(error)) => log::warn(format!("the lease could not be released: {error}")),
            Err(_) => log::warn("the lease could not be released; it expires by itself"),
        },
        None => log::warn("the lease could not be released; it expires by itself"),
    }
}

async fn file_loop(
    mut filer: Filer,
    lease: Arc<Lease>,
    healthy: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    counts: Arc<Counts>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        if lease.should_work() && healthy.load(Ordering::SeqCst) {
            busy.store(true, Ordering::SeqCst);
            let result = filer.pass(PASS_SIZE).await;
            busy.store(false, Ordering::SeqCst);
            match result {
                Ok(done) if !done.is_empty() => {
                    for (row, outcome) in &done {
                        report(row, outcome, &counts);
                    }
                    continue;
                }
                Ok(_) => {}
                Err(error) => {
                    log::warn(format!(
                        "filing paused: the database failed ({error}); trying again in 5 s"
                    ));
                    if wait(&mut stop, RETRY).await {
                        break;
                    }
                    continue;
                }
            }
        }
        if wait(&mut stop, Duration::from_secs(1)).await {
            break;
        }
    }
}

fn report(row: &Waiting, outcome: &Outcome, counts: &Counts) {
    match outcome {
        Outcome::Filed(id) => {
            counts.filed.fetch_add(1, Ordering::Relaxed);
            log::debug(format!("filed {} as call {id}", row.rel_path));
        }
        Outcome::AlreadyFiled(id) => {
            counts.found.fetch_add(1, Ordering::Relaxed);
            log::debug(format!("{} had been filed as call {id}", row.rel_path));
        }
        Outcome::Rejected(why) => {
            counts.rejected.fetch_add(1, Ordering::Relaxed);
            log::info(format!("rejected {}: {why}", row.rel_path));
        }
        Outcome::Later { why, .. } => log::warn(format!("{}: {why}", row.rel_path)),
        Outcome::LeaseLost => log::info(format!(
            "the lease was lost while filing; {} stays waiting",
            row.rel_path
        )),
    }
}

async fn watch_loop(
    pool: PgPool,
    db_limit: Duration,
    mut watcher: Watcher,
    rescan_every: Duration,
    stable_for: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let (events_tx, mut events) = mpsc::unbounded_channel::<PathBuf>();
    // Kept alive for as long as the loop runs.
    let _events = start_events(&watcher, events_tx);
    let tick = (stable_for / 2).clamp(Duration::from_millis(100), Duration::from_secs(1));
    let mut next_rescan = Instant::now();
    loop {
        let now = Instant::now();
        while let Ok(path) = events.try_recv() {
            watcher.touch(&path, now);
        }
        let batches = if now >= next_rescan {
            next_rescan = now + rescan_every;
            watcher.rescan(now)
        } else {
            watcher.settle(now)
        };
        if !batches.is_empty() {
            match limited(db_limit, record(&pool, &batches)).await {
                Ok(()) => watcher.recorded(&batches),
                Err(error) => {
                    log::warn(format!(
                        "the watcher cannot write to the database ({error}); trying again in 5 s"
                    ));
                    if wait(&mut stop, RETRY).await {
                        break;
                    }
                    continue;
                }
            }
        }
        if wait(&mut stop, tick).await {
            break;
        }
    }
}

/// File system events for every source; `None` when they cannot be had, and
/// then the rescans alone find the files.
fn start_events(
    watcher: &Watcher,
    events: mpsc::UnboundedSender<PathBuf>,
) -> Option<RecommendedWatcher> {
    let handler = move |result: notify::Result<notify::Event>| {
        if let Ok(event) = result {
            for path in event.paths {
                let _ = events.send(path);
            }
        }
    };
    let mut watch = match notify::recommended_watcher(handler) {
        Ok(watch) => watch,
        Err(error) => {
            log::warn(format!(
                "no file system events ({error}); the sources are listed every rescan"
            ));
            return None;
        }
    };
    for path in watcher.paths() {
        if let Err(error) = watch.watch(path, RecursiveMode::Recursive) {
            log::warn(format!(
                "no file system events for {} ({error}); it is listed every rescan",
                path.display()
            ));
        }
    }
    Some(watch)
}

async fn record(pool: &PgPool, batches: &[Batch]) -> sqlx::Result<()> {
    for batch in batches {
        store::upsert_seen(pool, batch.source_id, &batch.files).await?;
    }
    Ok(())
}

async fn health_loop(
    mut health: Health,
    healthy: Arc<AtomicBool>,
    mut stop: watch::Receiver<bool>,
) {
    let mut was_healthy = true;
    loop {
        health.check().await;
        let now_healthy = health.healthy();
        healthy.store(now_healthy, Ordering::SeqCst);
        if now_healthy != was_healthy {
            if now_healthy {
                log::info("the archive and the sources answer again");
            } else {
                log::error(
                    "the archive, or every source, has failed for too long: \
                     this host gives up the lease",
                );
            }
            was_healthy = now_healthy;
        }
        if wait(&mut stop, health.timings().every).await {
            break;
        }
    }
}

async fn summary_loop(
    pool: PgPool,
    db_limit: Duration,
    counts: Arc<Counts>,
    lease: Arc<Lease>,
    kept_days: u32,
    mut stop: watch::Receiver<bool>,
) {
    let mut pruned: Option<Instant> = None;
    loop {
        if wait(&mut stop, SUMMARY_EVERY).await {
            break;
        }
        let filed = counts.filed.swap(0, Ordering::Relaxed);
        let found = counts.found.swap(0, Ordering::Relaxed);
        let rejected = counts.rejected.swap(0, Ordering::Relaxed);
        let files = match limited(db_limit, store::counts(&pool)).await {
            Ok((waiting, rejected_all)) => {
                format!("{waiting} waiting, {rejected_all} rejected in all")
            }
            Err(error) => format!("the counts cannot be read: {error}"),
        };
        let role = if lease.held() { "holder" } else { "standby" };
        log::info(format!(
            "last minute ({role}): {filed} filed, {found} found filed, {rejected} rejected; {files}"
        ));
        if lease.held() && pruned.is_none_or(|at| at.elapsed() >= PRUNE_EVERY) {
            match limited(db_limit, store::prune_done(&pool, kept_days)).await {
                Ok(n) => log::debug(format!("pruned {n} rows of filed files")),
                Err(error) => log::warn(format!(
                    "the rows of filed files could not be pruned: {error}"
                )),
            }
            pruned = Some(Instant::now());
        }
    }
}
