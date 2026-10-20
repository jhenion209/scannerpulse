//! `scannerpulse ingest`: the watcher on every host, the filer and the
//! transcriber on the lease holder only, the lease and path health, and a
//! summary in the journal every minute. A database outage pauses the work, which is tried again every 5 s;
//! only a setup error stops the process.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection, PgPool, PgPoolOptions};
use tokio::sync::{mpsc, watch};
use tokio::time::sleep;

use crate::asr::Client;
use crate::filer::{Filer, Outcome};
use crate::health::{self, Health, HealthTimings};
use crate::lease::{Lease, Timings};
use crate::localtime::TimeHow;
use crate::log;
use crate::queue;
use crate::settings::{self, IngestSettings, SettingsError, Transcription};
use crate::store::{self, Listed, Location, Source, Waiting, limited};
use crate::transcriber::{self, Transcriber};
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

/// How often the transcription settings are read again, so that a change on
/// the admin page takes effect without a restart.
pub const SETTINGS_EVERY: Duration = Duration::from_secs(60);

/// How long a stop waits for the filer before it releases the lease anyway.
pub const STOP_FILER_LIMIT: Duration = Duration::from_secs(20);

/// How often each host reads the maintenance switch.
pub const MAINTENANCE_EVERY: Duration = Duration::from_secs(2);

/// The notice that asks every host's watcher to list its sources now.
pub const RESCAN_CHANNEL: &str = "scannerpulse_rescan";

/// Asks the watcher of every running ingest to list its sources at once.
pub async fn request_rescan(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_notify($1, '')")
        .bind(RESCAN_CHANNEL)
        .execute(db)
        .await?;
    Ok(())
}

/// Files per pass of the filer: a stop waits for one pass at most.
const PASS_SIZE: i64 = 20;

/// What a host needs to run ingest.
#[derive(Debug, Clone)]
pub struct Config {
    pub database: PgConnectOptions,
    /// Every database node, when there is more than one: ingest works on
    /// the primary.
    pub nodes: Vec<crate::primary::Node>,
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
    /// The key of the transcription servers, for servers that need one.
    pub api_key: Option<String>,
}

impl Config {
    /// The host's settings from the environment, until a host configuration
    /// file takes their place: DATABASE_URL (the password in the file that
    /// PGPASSFILE names), SCANNERPULSE_HOST (default: the system's host name)
    /// and SCANNERPULSE_PREFERRED (`true` on one host of a pair), and
    /// SCANNERPULSE_TRANSCRIPTION_API_KEY for servers that need a key.
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
            nodes: crate::primary::from_env()?,
            host,
            preferred,
            lease: Timings::default(),
            health: HealthTimings::default(),
            stable_for: watcher::STABLE_FOR,
            db_limit: DB_LIMIT,
            api_key: std::env::var("SCANNERPULSE_TRANSCRIPTION_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
        })
    }
}

struct Setup {
    settings: IngestSettings,
    transcription: Option<Transcription>,
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
    let transcription = match tokio::time::timeout(limit, settings::load_transcription(pool)).await
    {
        Ok(Ok(transcription)) => transcription,
        Ok(Err(SettingsError::Database(error))) => return Err(SetupError::Database(error)),
        Ok(Err(error)) => return Err(SetupError::Fatal(error.to_string())),
        Err(_) => return Err(SetupError::Database(store::timed_out(limit))),
    };
    let location = limited(limit, store::primary_location(pool))
        .await
        .map_err(SetupError::Database)?
        .ok_or_else(|| SetupError::Fatal("there is no primary location for the archive".into()))?;
    // No watched recorder is no fault: one taken in when asked may be added
    // or watched later, and the watcher follows the recorders as they change.
    let sources = limited(limit, store::enabled_sources(pool))
        .await
        .map_err(SetupError::Database)?;
    Ok(Setup {
        settings,
        transcription,
        location,
        sources,
    })
}

/// What the filer did in one summary period.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Minute {
    pub filed: u64,
    pub found: u64,
    pub rejected: u64,
    /// Filed calls whose local time lay in the spring gap.
    pub shifted: u64,
    /// Filed calls in the repeated hour whose instant was a guess.
    pub guessed: u64,
}

impl Minute {
    /// Counts one file's outcome.
    pub fn add(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Filed(_, how) => {
                self.filed += 1;
                match how {
                    TimeHow::Gap => self.shifted += 1,
                    TimeHow::EarlierDefault => self.guessed += 1,
                    TimeHow::Plain | TimeHow::ByMtime => {}
                }
            }
            Outcome::AlreadyFiled(_) => self.found += 1,
            Outcome::Rejected(_) => self.rejected += 1,
            Outcome::Later { .. } | Outcome::LeaseLost => {}
        }
    }

    /// The summary line; `role` is holder or standby, `files` the counts of
    /// the table.
    pub fn line(&self, role: &str, files: &str) -> String {
        let Minute {
            filed,
            found,
            rejected,
            shifted,
            guessed,
        } = self;
        let times = if shifted + guessed > 0 {
            format!(", {shifted} shifted (spring gap), {guessed} guessed (repeated hour)")
        } else {
            String::new()
        };
        format!(
            "last minute ({role}): {filed} filed, {found} found filed, {rejected} rejected{times}; {files}"
        )
    }
}

/// What the transcriber did in one summary period.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Heard {
    pub text: u64,
    pub no_speech: u64,
    /// Results not written because a person's text was there.
    pub kept: u64,
    /// Calls that failed and are tried again later.
    pub retry: u64,
    /// Calls that failed for good.
    pub failed: u64,
    /// Requests that a server failed.
    pub server_failures: u64,
}

impl Heard {
    /// Counts one call's outcome.
    pub fn add(&mut self, outcome: &transcriber::Outcome) {
        use crate::queue::Written;
        use transcriber::Outcome as T;
        match outcome {
            T::Written(_, Written::Text) => self.text += 1,
            T::Written(_, Written::NoSpeech) => self.no_speech += 1,
            T::Written(_, Written::KeptHuman) => self.kept += 1,
            T::Retry { .. } => self.retry += 1,
            T::Failed { .. } => self.failed += 1,
            T::ServerDown { .. } => self.server_failures += 1,
            T::LeaseLost(_) | T::Gone(_) | T::Written(_, Written::CallGone) => {}
        }
    }

    /// The summary line; `queue` is the queue in numbers.
    pub fn line(&self, queue: &str) -> String {
        let Heard {
            text,
            no_speech,
            kept,
            retry,
            failed,
            server_failures,
        } = self;
        let kept = if *kept > 0 {
            format!(", {kept} kept a person's text")
        } else {
            String::new()
        };
        format!(
            "transcription: {text} transcribed, {no_speech} no speech{kept}, {retry} to try again, \
             {failed} failed, {server_failures} server failures; {queue}"
        )
    }
}

type Counts = Mutex<Minute>;

/// Plain numbers: a panic elsewhere cannot leave them half written.
fn lock<T>(counts: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    counts
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Runs ingest until `stop` completes. Err only for a setup error. At a stop
/// the filer finishes its pass (for `STOP_FILER_LIMIT` at most), then the
/// lease is released, so that the other host takes it at its next round.
pub async fn run(config: Config, stop: impl Future<Output = ()> + Send) -> Result<(), String> {
    tokio::pin!(stop);
    let pool = crate::primary::pool(
        &config.database,
        &config.nodes,
        PgPoolOptions::new()
            // Held at once: the lease, the rescan listener (for its whole life),
            // a job step (up to the request limit), and one each for the file,
            // transcribe, watch and maintenance loops; the eighth is headroom
            // (and the syslog setting's check, a moment each minute).
            .max_connections(8)
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
            }),
    );
    crate::settings::follow_syslog(pool.clone(), "scannerpulse-ingest");
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
        transcription,
        location,
        sources,
    } = setup;
    log::info(format!(
        "ingest on {}: archive {}, {} watched recorder(s)",
        config.host,
        location.root.display(),
        sources.len()
    ));
    let lease = Arc::new(Lease::new(&config.host, config.preferred, config.lease));
    let healthy = Arc::new(AtomicBool::new(true));
    let busy = Arc::new(AtomicBool::new(false));
    let counts = Arc::new(Counts::default());
    let heard = Arc::new(Mutex::new(Heard::default()));
    let rule = Arc::new(AtomicI32::new(
        transcription.as_ref().map_or(-1, |t| t.min_duration_ms),
    ));
    if transcription.is_none() {
        log::info("transcription is off");
    }
    let source_paths: Vec<&Path> = sources.iter().map(|s| s.path.as_path()).collect();
    let health = Health::new(
        &location.root,
        &source_paths,
        config.health,
        health::read_dir_probe,
    );
    let watcher = Watcher::new(sources.clone(), config.stable_for);
    let (folders_tx, folders_rx) = watch::channel(
        sources
            .iter()
            .map(|s| s.path.clone())
            .collect::<Vec<PathBuf>>(),
    );
    let filer = Filer::new(
        pool.clone(),
        lease.clone(),
        location,
        settings.time_zone,
        settings.missing_source_grace,
    )
    .with_db_limit(config.db_limit)
    .with_transcription(rule.clone());
    let mut transcriber = Transcriber::new(
        pool.clone(),
        lease.clone(),
        Client::new(config.api_key.clone()),
    )
    .with_db_limit(config.db_limit);
    transcriber.set(transcription);

    let (stop_tx, stop_rx) = watch::channel(false);
    let (release_tx, release_rx) = watch::channel(false);
    // While the maintenance switch is on, filing, transcription and jobs
    // pause; the lease is kept.
    // Until the first read of maintenance.enabled succeeds, nothing is filed,
    // transcribed or stepped.
    let paused = Arc::new(AtomicBool::new(true));
    let (rescan_tx, rescans) = mpsc::unbounded_channel::<()>();
    // Health runs until the lease is released, so that the lease never renews
    // on a stale "healthy".
    let tasks = [
        (
            "health",
            tokio::spawn(health_loop(
                health,
                healthy.clone(),
                folders_rx,
                release_rx.clone(),
            )),
        ),
        (
            "maintenance",
            tokio::spawn(maintenance_loop(
                pool.clone(),
                config.db_limit,
                paused.clone(),
                stop_rx.clone(),
            )),
        ),
        (
            "rescans",
            tokio::spawn(rescan_loop(pool.clone(), rescan_tx, stop_rx.clone())),
        ),
        (
            "watcher",
            tokio::spawn(watch_loop(
                pool.clone(),
                config.db_limit,
                config.host.clone(),
                watcher,
                Rescans {
                    every: settings.rescan,
                    asked: rescans,
                },
                config.stable_for,
                folders_tx,
                stop_rx.clone(),
            )),
        ),
        (
            "summary",
            tokio::spawn(summary_loop(
                pool.clone(),
                config.db_limit,
                counts.clone(),
                heard.clone(),
                lease.clone(),
                settings.done_rows_kept_days,
                stop_rx.clone(),
            )),
        ),
        (
            "jobs",
            tokio::spawn(job_loop(
                pool.clone(),
                config.db_limit,
                config.host.clone(),
                crate::jobs::Context::new(config.api_key.clone()),
                lease.clone(),
                healthy.clone(),
                paused.clone(),
                stop_rx.clone(),
            )),
        ),
        (
            "transcriber",
            tokio::spawn(transcribe_loop(
                transcriber,
                pool.clone(),
                config.db_limit,
                rule,
                lease.clone(),
                healthy.clone(),
                paused.clone(),
                heard,
                stop_rx.clone(),
            )),
        ),
    ];
    let lease_task = tokio::spawn(lease_loop(
        pool.clone(),
        lease.clone(),
        healthy.clone(),
        busy.clone(),
        release_rx,
    ));
    let filer_task = tokio::spawn(file_loop(
        filer, lease, healthy, paused, busy, counts, stop_rx,
    ));

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
    // Its own connection, made on the node the pool works on: the primary.
    pool: PgPool,
    lease: Arc<Lease>,
    healthy: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    mut release: watch::Receiver<bool>,
) {
    let mut conn: Option<PgConnection> = None;
    let mut was_held = false;
    loop {
        if conn.is_none() {
            match connect_lease(&pool.connect_options()).await {
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
        conn = connect_lease(&pool.connect_options()).await.ok();
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
    paused: Arc<AtomicBool>,
    busy: Arc<AtomicBool>,
    counts: Arc<Counts>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        if lease.should_work() && healthy.load(Ordering::SeqCst) && !paused.load(Ordering::SeqCst) {
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

/// The job runner: while this host holds the lease, steps the job at the head
/// of the line, one step after another; a stop asked for takes effect
/// between steps, and a job left running by another host resumes here.
#[allow(clippy::too_many_arguments)]
async fn job_loop(
    pool: PgPool,
    db_limit: Duration,
    host: String,
    context: crate::jobs::Context,
    lease: Arc<Lease>,
    healthy: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            break;
        }
        if lease.should_work() && healthy.load(Ordering::SeqCst) && !paused.load(Ordering::SeqCst) {
            // The step's database work plus one candidate request, which the
            // request limit caps: a slow server ends its own step, and the
            // job line goes on.
            match limited(
                db_limit + crate::jobs::REQUEST_LIMIT,
                crate::jobs::step(&pool, &host, &context),
            )
            .await
            {
                Ok(Some((id, state))) => {
                    if state != "running" {
                        log::info(format!("job {id} is {state}"));
                    }
                    continue;
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn(format!("jobs wait: {error}; trying again in 5 s"));
                    // Say why on the job at the head of the line, without
                    // touching updated_at, which tells a stuck job apart.
                    let why: String = format!("waiting: {error}").chars().take(300).collect();
                    let _ = limited(
                        db_limit,
                        sqlx::query(
                            "UPDATE jobs SET message = $1 \
                             WHERE id = (SELECT id FROM jobs \
                                         WHERE id = (SELECT min(id) FROM jobs \
                                                     WHERE state IN ('queued', 'running')) \
                                         FOR UPDATE SKIP LOCKED)",
                        )
                        .bind(why)
                        .execute(&pool),
                    )
                    .await;
                    if wait(&mut stop, RETRY).await {
                        break;
                    }
                    continue;
                }
            }
        }
        if wait(&mut stop, Duration::from_secs(2)).await {
            break;
        }
    }
}

fn report(row: &Waiting, outcome: &Outcome, counts: &Counts) {
    lock(counts).add(outcome);
    match outcome {
        Outcome::Filed(id, _) => log::debug(format!("filed {} as call {id}", row.rel_path)),
        Outcome::AlreadyFiled(id) => {
            log::debug(format!("{} had been filed as call {id}", row.rel_path))
        }
        Outcome::Rejected(why) => log::info(format!("rejected {}: {why}", row.rel_path)),
        Outcome::Later { why, .. } => log::warn(format!("{}: {why}", row.rel_path)),
        Outcome::LeaseLost => log::info(format!(
            "the lease was lost while filing; {} stays waiting",
            row.rel_path
        )),
    }
}

/// When the watcher lists every folder: every `every`, and at once when an
/// admin asks.
struct Rescans {
    every: Duration,
    asked: mpsc::UnboundedReceiver<()>,
}

#[allow(clippy::too_many_arguments)]
async fn watch_loop(
    pool: PgPool,
    db_limit: Duration,
    host: String,
    mut watcher: Watcher,
    mut rescans: Rescans,
    stable_for: Duration,
    folders: watch::Sender<Vec<PathBuf>>,
    mut stop: watch::Receiver<bool>,
) {
    let (events_tx, mut events) = mpsc::unbounded_channel::<PathBuf>();
    // Kept alive until the watched folders change.
    let mut _events = start_events(&watcher, events_tx.clone());
    let tick = (stable_for / 2).clamp(Duration::from_millis(100), Duration::from_secs(1));
    let mut next_rescan = Instant::now();
    loop {
        let now = Instant::now();
        while let Ok(path) = events.try_recv() {
            watcher.touch(&path, now);
        }
        // An admin asked for a rescan, or changed a recorder.
        if rescans.asked.try_recv().is_ok() {
            while rescans.asked.try_recv().is_ok() {}
            next_rescan = now;
        }
        // The requests to bring files in whose files are in `batches`.
        let mut brought = Vec::new();
        let batches = if now >= next_rescan {
            next_rescan = now + rescans.every;
            // The recorders may have changed since the last rescan.
            let asked = match limited(db_limit, store::listed_recorders(&pool)).await {
                Ok(listed) => {
                    let (asked, watched): (Vec<Listed>, Vec<Listed>) =
                        listed.into_iter().partition(|l| l.asked);
                    if watcher.set_sources(watched.into_iter().map(|l| l.source).collect()) {
                        _events = start_events(&watcher, events_tx.clone());
                        let watched: Vec<PathBuf> =
                            watcher.paths().map(Path::to_path_buf).collect();
                        log::info(format!("{} watched recorder(s) now", watched.len()));
                        let _ = folders.send(watched);
                    }
                    asked
                }
                Err(error) => {
                    log::warn(format!(
                        "the recorders cannot be read ({error}); the watcher keeps the folders it watches"
                    ));
                    Vec::new()
                }
            };
            let mut batches = watcher.rescan(now);
            let mut found = watcher.listings().to_vec();
            for listed in &asked {
                list_asked(
                    &mut watcher,
                    listed,
                    stable_for,
                    &mut found,
                    &mut batches,
                    &mut brought,
                );
            }
            if let Err(error) = limited(db_limit, store::folder_health(&pool, &host, &found)).await
            {
                log::warn(format!("what the folders hold cannot be written: {error}"));
            }
            batches
        } else {
            watcher.settle(now)
        };
        if !batches.is_empty() || !brought.is_empty() {
            match limited(db_limit, record(&pool, &batches)).await {
                Ok(()) => {
                    watcher.recorded(&batches);
                    for (source_id, at) in brought {
                        if let Err(error) =
                            limited(db_limit, store::brought_in(&pool, source_id, at)).await
                        {
                            log::warn(format!(
                                "a recorder's files were brought in, but that could not be noted ({error}); \
                                 they are listed again"
                            ));
                        }
                    }
                }
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

/// Lists a recorder taken in when asked: counts its files and, once a request
/// to bring them in is `stable_for` old, adds the files it held at that
/// moment to `batches` and the request to `brought`. A file written in the
/// moment of the request may still have been growing: the wait lets it settle.
fn list_asked(
    watcher: &mut Watcher,
    listed: &Listed,
    stable_for: Duration,
    found: &mut Vec<(i16, Result<usize, String>)>,
    batches: &mut Vec<Batch>,
    brought: &mut Vec<(i16, DateTime<Utc>)>,
) {
    let id = listed.source.id;
    let files = match watcher.list_folder(&listed.source.path) {
        Ok(files) => files,
        Err(error) => {
            found.push((id, Err(format!("cannot be listed: {error}"))));
            return;
        }
    };
    found.push((id, Ok(files.len())));
    let Some(at) = listed.bring_in else {
        return;
    };
    if Utc::now() < at + stable_for {
        return;
    }
    let seen = watcher::to_bring_in(&files, at.into());
    if !seen.is_empty() {
        batches.push(Batch {
            source_id: id,
            files: seen,
        });
    }
    brought.push((id, at));
}

/// Reads the maintenance switch every `MAINTENANCE_EVERY`; a value that
/// cannot be read leaves the last one.
async fn maintenance_loop(
    pool: PgPool,
    db_limit: Duration,
    paused: Arc<AtomicBool>,
    mut stop: watch::Receiver<bool>,
) {
    let mut known: Option<bool> = None;
    let mut failing = false;
    loop {
        let read = limited(db_limit, async {
            sqlx::query_scalar::<_, serde_json::Value>(
                "SELECT value FROM settings WHERE key = 'maintenance.enabled'",
            )
            .fetch_optional(&pool)
            .await
        })
        .await;
        match read {
            Ok(value) => {
                let on = value.and_then(|v| v.as_bool()).unwrap_or(false);
                paused.store(on, Ordering::SeqCst);
                match known {
                    Some(before) if before != on => log::info(if on {
                        "maintenance is on: filing, transcription and jobs pause; the lease is kept"
                    } else {
                        "maintenance is off: filing, transcription and jobs go on"
                    }),
                    None if on => log::info(
                        "maintenance is on: filing, transcription and jobs pause; the lease is kept",
                    ),
                    _ => {}
                }
                if failing {
                    log::info("maintenance setting read again");
                    failing = false;
                }
                known = Some(on);
            }
            Err(error) => {
                if !failing {
                    log::warn(format!(
                        "cannot read maintenance.enabled ({error}); filing, transcription and jobs stay {}",
                        if paused.load(Ordering::SeqCst) {
                            "paused"
                        } else {
                            "running"
                        }
                    ));
                    failing = true;
                }
            }
        }
        if wait(&mut stop, MAINTENANCE_EVERY).await {
            break;
        }
    }
}

/// Hears the requests for a rescan and passes them to the watcher; a lost
/// connection is opened again after a pause.
async fn rescan_loop(
    pool: PgPool,
    rescans: mpsc::UnboundedSender<()>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let heard = async {
            let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
            listener.listen(RESCAN_CHANNEL).await?;
            loop {
                // A connection lost is opened again (`None`); one gone quiet
                // with its machine is an error, and the loop starts over.
                if crate::primary::listened(&mut listener, crate::primary::QUIET)
                    .await?
                    .is_some()
                {
                    let _ = rescans.send(());
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), sqlx::Error>(())
        };
        tokio::select! {
            result = heard => {
                if let Err(error) = result {
                    log::warn(format!("rescan requests cannot be heard ({error}); trying again in 5 s"));
                }
            }
            _ = stop.changed() => break,
        }
        if wait(&mut stop, RETRY).await {
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
    mut folders: watch::Receiver<Vec<PathBuf>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut was_healthy = true;
    loop {
        if folders.has_changed().unwrap_or(false) {
            let watched = folders.borrow_and_update().clone();
            health.set_sources(&watched.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        }
        health.check().await;
        let now_healthy = health.healthy();
        healthy.store(now_healthy, Ordering::SeqCst);
        if now_healthy != was_healthy {
            if now_healthy {
                log::info("the archive and the sources answer again");
            } else {
                log::error(
                    "the archive, or every watched recorder's folder, has failed for too long: \
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
    heard: Arc<Mutex<Heard>>,
    lease: Arc<Lease>,
    kept_days: u32,
    mut stop: watch::Receiver<bool>,
) {
    let mut pruned: Option<Instant> = None;
    loop {
        if wait(&mut stop, SUMMARY_EVERY).await {
            break;
        }
        let minute = std::mem::take(&mut *lock(&counts));
        let files = match limited(db_limit, store::counts(&pool)).await {
            Ok((waiting, rejected_all)) => {
                format!("{waiting} waiting, {rejected_all} rejected in all")
            }
            Err(error) => format!("the counts cannot be read: {error}"),
        };
        let role = if lease.held() { "holder" } else { "standby" };
        log::info(minute.line(role, &files));
        let heard = std::mem::take(&mut *lock(&heard));
        let queue = match limited(db_limit, queue::counts(&pool)).await {
            Ok(q) => format!(
                "{} queued in the last minute; the queue holds {} ({} due), {} failed",
                q.last_minute, q.queued, q.due, q.failed
            ),
            Err(error) => format!("the queue cannot be read: {error}"),
        };
        log::info(heard.line(&queue));
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

/// The transcriber's loop: reads the settings again every minute, sends due
/// calls while this host holds the lease and is healthy, and writes each
/// result. At a stop the requests in flight end; their calls stay queued.
#[allow(clippy::too_many_arguments)]
async fn transcribe_loop(
    mut transcriber: Transcriber,
    pool: PgPool,
    db_limit: Duration,
    rule: Arc<AtomicI32>,
    lease: Arc<Lease>,
    healthy: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    heard: Arc<Mutex<Heard>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut read_at = Instant::now();
    let mut unsaved: Option<Vec<crate::servers::ServerState>> = None;
    let mut was_held = false;
    let mut save_failed_at: Option<Instant> = None;
    loop {
        if *stop.borrow() {
            break;
        }
        if read_at.elapsed() >= SETTINGS_EVERY {
            read_at = Instant::now();
            match tokio::time::timeout(db_limit, settings::load_transcription(&pool)).await {
                Ok(Ok(new)) if new.as_ref() != transcriber.settings() => {
                    log::info(if new.is_some() {
                        "the transcription settings changed"
                    } else {
                        "transcription is off"
                    });
                    rule.store(
                        new.as_ref().map_or(-1, |t| t.min_duration_ms),
                        Ordering::SeqCst,
                    );
                    transcriber.set(new);
                }
                Ok(Ok(_)) => {}
                Ok(Err(error)) => log::warn(format!("{error}; the last settings stay")),
                Err(_) => {
                    log::warn("the transcription settings cannot be read; the last ones stay")
                }
            }
        }
        if let Some(states) = transcriber.take_server_change() {
            unsaved = Some(states);
        }
        let held = lease.held();
        if held && !was_held {
            unsaved = Some(transcriber.server_states());
        }
        was_held = held;
        if held
            && save_failed_at.is_none_or(|at| at.elapsed() >= RETRY)
            && let Some(states) = &unsaved
        {
            for state in states {
                if let Some(cooling) = state.cooling_for {
                    log::warn(format!(
                        "transcription server {} cools down for {} s: {}",
                        state.url,
                        cooling.as_secs(),
                        state.last_error.as_deref().unwrap_or("")
                    ));
                }
            }
            match limited(db_limit, queue::save_servers(&pool, states)).await {
                Ok(()) => {
                    unsaved = None;
                    save_failed_at = None;
                }
                Err(error) => {
                    save_failed_at = Some(Instant::now());
                    log::warn(format!("the servers' state cannot be saved: {error}"));
                }
            }
        }
        // Requests under way finish; no new one starts during maintenance.
        if lease.should_work()
            && healthy.load(Ordering::SeqCst)
            && !paused.load(Ordering::SeqCst)
            && let Err(error) = transcriber.fill().await
        {
            log::warn(format!(
                "transcription paused: the database failed ({error}); trying again in 5 s"
            ));
            if wait(&mut stop, RETRY).await {
                break;
            }
            continue;
        }
        if transcriber.busy() == 0 {
            if wait(&mut stop, Duration::from_secs(1)).await {
                break;
            }
            continue;
        }
        let done = tokio::select! {
            done = transcriber.next_done() => done,
            _ = stop.changed() => None,
            _ = sleep(Duration::from_secs(1)) => None,
        };
        if let Some(done) = done {
            match transcriber.settle(done).await {
                Ok(outcome) => {
                    lock(&heard).add(&outcome);
                    describe(&outcome);
                }
                Err(error) => log::warn(format!(
                    "a transcript could not be written ({error}); the call stays queued"
                )),
            }
        }
    }
    transcriber.abort();
}

fn describe(outcome: &transcriber::Outcome) {
    use transcriber::Outcome as T;
    match outcome {
        T::Written(call, written) => log::debug(format!("call {call}: {written:?}")),
        T::Retry { call, error } => log::info(format!("call {call} is tried again later: {error}")),
        T::Failed { call, error } => log::warn(format!("call {call} failed for good: {error}")),
        T::ServerDown {
            call,
            server,
            error,
            ..
        } => log::debug(format!(
            "call {call} stays queued: {server} failed: {error}"
        )),
        T::LeaseLost(call) => log::info(format!(
            "the lease was lost while transcribing; call {call} stays queued"
        )),
        T::Gone(call) => log::debug(format!("call {call} left the queue while it was sent")),
    }
}
