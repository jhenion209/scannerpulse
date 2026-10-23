//! `scannerpulse agent`: the root helper on the first ScannerPulse machine (or
//! one machine alone). The web service runs without root, so an admin's
//! "Update now", switchover or approved step of a conversion is a row in
//! `agent_requests` that this takes, runs, and writes what it says into,
//! line by line: the admin page reads it through the restarts an update or a
//! move makes. It does nothing else an admin did not ask for, except to look
//! for a newer release once a day, unless `updates.check` is false.

use serde::Serialize;
use sqlx::PgPool;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The helper's unit, on the first ScannerPulse machine only.
pub const UNIT: &str = "/etc/systemd/system/scannerpulse-agent.service";

/// How often it says it is up and looks for a request.
const EVERY: Duration = Duration::from_secs(5);
/// How often it looks for a newer release.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
/// A helper not heard from for this long is shown as not running.
pub const SILENT_AFTER: i64 = 30;

/// Set in the helper's own process: what it runs leaves the helper's
/// restart to it, once the request is closed.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// This process is the root helper.
pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// What an admin may ask for: the conversion's steps take their machines
/// and folders from its plan (`convert::PATH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// `scannerpulse update`, every machine in turn.
    Update,
    /// `scannerpulse db switchover`: the standby becomes the primary.
    Switchover,
    /// `scannerpulse archive move FOLDER`: the copy compared.
    ArchiveCheck,
    /// `scannerpulse archive move FOLDER --switch`.
    ArchiveSwitch,
    /// `scannerpulse db move DB1 DB2 --hub ADDRESS [--referee REF]`.
    DbMove,
    /// `scannerpulse db move --switch`.
    DbSwitch,
    /// `scannerpulse db move --retire`, the approval its confirmation.
    DbRetire,
    /// `scannerpulse second NAME [ADDRESS/PREFIX]`, without its wait.
    Second,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Update,
        Kind::Switchover,
        Kind::ArchiveCheck,
        Kind::ArchiveSwitch,
        Kind::DbMove,
        Kind::DbSwitch,
        Kind::DbRetire,
        Kind::Second,
    ];

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.name() == text)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Update => "update",
            Kind::Switchover => "switchover",
            Kind::ArchiveCheck => "archive_check",
            Kind::ArchiveSwitch => "archive_switch",
            Kind::DbMove => "db_move",
            Kind::DbSwitch => "db_switch",
            Kind::DbRetire => "db_retire",
            Kind::Second => "second",
        }
    }

    /// A step of the conversion, which reads its plan.
    pub fn converts(self) -> bool {
        !matches!(self, Kind::Update | Kind::Switchover)
    }
}

/// Why a request was not taken.
#[derive(Debug)]
pub enum Refused {
    /// Another request waits or runs.
    Busy,
    /// No helper has said it is up lately.
    NoHelper,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Refused {
    fn from(error: sqlx::Error) -> Refused {
        Refused::Database(error)
    }
}

/// A request, as the admin page shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Request {
    pub id: i64,
    pub kind: String,
    pub requested_by: String,
    pub requested_at: chrono::DateTime<chrono::Utc>,
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub host: Option<String>,
    pub ok: Option<bool>,
    pub said: String,
}

/// Records an admin's request, once a helper is up and none other is open.
pub async fn ask(pool: &PgPool, kind: Kind, by: &str) -> Result<i64, Refused> {
    // One that waits or runs comes first: its helper may be busy with it.
    if open(pool).await? {
        return Err(Refused::Busy);
    }
    let up: Option<bool> = sqlx::query_scalar(
        "SELECT bool_or(seen_at > now() - make_interval(secs => $1)) FROM agents",
    )
    .bind(SILENT_AFTER as f64)
    .fetch_one(pool)
    .await?;
    if up != Some(true) {
        return Err(Refused::NoHelper);
    }
    sqlx::query_scalar(
        "INSERT INTO agent_requests (kind, requested_by) VALUES ($1, $2) RETURNING id",
    )
    .bind(kind.name())
    .bind(by)
    .fetch_one(pool)
    .await
    .map_err(|error| match error.as_database_error() {
        Some(database) if database.is_unique_violation() => Refused::Busy,
        _ => Refused::Database(error),
    })
}

/// The newest request of `kind`, if any.
pub async fn last(pool: &PgPool, kind: Kind) -> sqlx::Result<Option<Request>> {
    sqlx::query_as("SELECT * FROM agent_requests WHERE kind = $1 ORDER BY id DESC LIMIT 1")
        .bind(kind.name())
        .fetch_optional(pool)
        .await
}

/// A request of either kind waits or runs.
pub async fn open(pool: &PgPool) -> sqlx::Result<bool> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM agent_requests WHERE finished_at IS NULL)")
        .fetch_one(pool)
        .await
}

/// Takes the oldest request no helper has started, for `host`, of a kind
/// this helper knows: one a newer web recorded waits for a newer helper.
pub async fn claim(pool: &PgPool, host: &str) -> sqlx::Result<Option<(i64, Kind)>> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "UPDATE agent_requests SET started_at = now(), host = $1 \
         WHERE id = (SELECT id FROM agent_requests WHERE started_at IS NULL AND kind = ANY($2) \
                     ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, kind",
    )
    .bind(host)
    .bind(Kind::ALL.map(Kind::name).to_vec())
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|(id, kind)| Kind::parse(&kind).map(|kind| (id, kind))))
}

/// Adds a line to what request `id` said.
pub async fn say(pool: &PgPool, id: i64, line: &str) -> sqlx::Result<()> {
    sqlx::query("UPDATE agent_requests SET said = said || $2 || E'\\n' WHERE id = $1")
        .bind(id)
        .bind(line)
        .execute(pool)
        .await
        .map(drop)
}

/// Closes request `id`, with its last line.
pub async fn finish(pool: &PgPool, id: i64, ok: bool, line: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE agent_requests SET finished_at = now(), ok = $2, said = said || $3 || E'\\n' \
         WHERE id = $1",
    )
    .bind(id)
    .bind(ok)
    .bind(line)
    .execute(pool)
    .await
    .map(drop)
}

/// Closes the requests `host`'s helper started and did not finish: it
/// stopped (or the machine did) while it ran them.
pub async fn abandoned(pool: &PgPool, host: &str) -> sqlx::Result<u64> {
    sqlx::query(
        "UPDATE agent_requests SET finished_at = now(), ok = false, \
         said = said || 'The helper stopped before this finished: see `journalctl -u scannerpulse-agent` on ' || host || ', then ask again.' || E'\\n' \
         WHERE host = $1 AND started_at IS NOT NULL AND finished_at IS NULL",
    )
    .bind(host)
    .execute(pool)
    .await
    .map(|done| done.rows_affected())
}

/// Says that `host`'s helper is up, and which version it runs.
pub async fn beat(pool: &PgPool, host: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO agents (host, version, seen_at) VALUES ($1, $2, now()) \
         ON CONFLICT (host) DO UPDATE SET version = EXCLUDED.version, seen_at = now()",
    )
    .bind(host)
    .bind(crate::update::VERSION)
    .execute(pool)
    .await
    .map(drop)
}

/// Keeps the newest release the daily check found.
pub async fn remember(pool: &PgPool, release: &crate::update::Release) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO newest_release (one, version, notes, checked_at) VALUES (true, $1, $2, now()) \
         ON CONFLICT (one) DO UPDATE SET version = EXCLUDED.version, notes = EXCLUDED.notes, \
         checked_at = now()",
    )
    .bind(&release.version)
    .bind(&release.notes)
    .execute(pool)
    .await
    .map(drop)
}

/// Whether the daily check is on (`updates.check`, true unless set false).
pub async fn checks(pool: &PgPool) -> sqlx::Result<bool> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'updates.check'")
            .fetch_optional(pool)
            .await?;
    Ok(value.and_then(|value| value.as_bool()).unwrap_or(true))
}

/// What request `id` runs, each line it says written to the request as it
/// comes. An update or a move restarts this machine's web and ingest, never
/// this helper, which restarts itself afterwards. Whether it ended well: a
/// check of the archive's copy ends well only when the copy is complete.
async fn carry_out(pool: &PgPool, id: i64, kind: Kind) -> (bool, String) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = {
        let pool = pool.clone();
        tokio::spawn(async move {
            // A line the database did not take (it may be moving) goes with
            // the next: late rather than lost.
            let mut held = String::new();
            while let Some(line) = rx.recv().await {
                crate::log::info(format!("agent: {line}"));
                let lines = if held.is_empty() {
                    line
                } else {
                    format!("{held}\n{line}")
                };
                held = match say(&pool, id, &lines).await {
                    Ok(()) => String::new(),
                    Err(_) => lines,
                };
            }
            while !held.is_empty() {
                match say(&pool, id, &held).await {
                    Ok(()) => held.clear(),
                    Err(_) => tokio::time::sleep(EVERY).await,
                }
            }
        })
    };
    let said = move |line: &str| {
        let _ = tx.send(line.to_string());
    };
    let well = |done: String| (true, done);
    let result = match kind {
        Kind::Update => crate::update::run(false, true, said, |_| true)
            .await
            .map(well),
        Kind::Switchover => {
            tokio::task::spawn_blocking(move || crate::setup::node::db_switchover(said))
                .await
                .unwrap_or_else(|error| Err(format!("the switchover stopped: {error}")))
                .map(well)
        }
        _ => {
            let by: String =
                sqlx::query_scalar("SELECT requested_by FROM agent_requests WHERE id = $1")
                    .bind(id)
                    .fetch_one(pool)
                    .await
                    .unwrap_or_else(|_| "the root helper".into());
            let options = pool.connect_options();
            crate::steps::carry_out(
                Some(&options),
                crate::steps::words(kind),
                &by,
                |line| crate::log::warn(format!("agent: {line}")),
                convert_step(pool, kind, said),
            )
            .await
        }
    };
    let _ = writer.await;
    match result {
        Ok(done) => done,
        Err(error) => (false, format!("{}: {error}", kind.name())),
    }
}

/// A step of the conversion, on the machines and folders its plan names.
async fn convert_step(
    pool: &PgPool,
    kind: Kind,
    said: impl Fn(&str),
) -> Result<(bool, String), String> {
    use crate::setup::{archive_move, change, db_move};
    let plan = crate::convert::Convert::read(Path::new(crate::convert::PATH))?;
    let well = |done: String| (true, done);
    match kind {
        Kind::ArchiveCheck | Kind::ArchiveSwitch => {
            let new = plan
                .archive
                .ok_or("the conversion's plan keeps the archive where it is")?;
            let mut conn = pool.acquire().await.map_err(|error| error.to_string())?;
            archive_move::run(
                &mut conn,
                Path::new(&new),
                kind == Kind::ArchiveSwitch,
                said,
            )
            .await
        }
        Kind::DbMove => {
            // Run again, the move reads its own plan: the database may be
            // restarting under pg_autoctl.
            let mut conn = pool.acquire().await.ok();
            db_move::begin(conn.as_deref_mut(), plan.nodes, plan.hub, plan.referee)
                .await
                .map(well)
        }
        Kind::DbSwitch => db_move::switch(said).await.map(well),
        Kind::DbRetire => db_move::retire(|_| true).await.map(well),
        Kind::Second => {
            let name = plan
                .second
                .ok_or("the conversion's plan adds no second ScannerPulse machine")?;
            change::add_second(&name, plan.shared.as_deref())
                .await
                .map(well)
        }
        Kind::Update | Kind::Switchover => Err("not a step of the conversion".into()),
    }
}

fn host() -> String {
    std::env::var("SCANNERPULSE_HOST")
        .or_else(|_| {
            std::fs::read_to_string("/proc/sys/kernel/hostname").map(|n| n.trim().to_string())
        })
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "agent".to_string())
}

/// Says the helper is up every `EVERY`, whatever else it is doing: an
/// update can take minutes.
fn spawn_beat(pool: PgPool, host: String) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = beat(&pool, &host).await {
                crate::log::warn(format!("agent: cannot say the helper is up: {error}"));
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// Looks for a newer release once a day while `updates.check` is on, each
/// look given a minute; a failed one is tried again within the hour.
fn spawn_check(pool: PgPool) {
    tokio::spawn(async move {
        loop {
            let wait = if checks(&pool).await.unwrap_or(false) {
                let source = crate::update::Source::from_env();
                match tokio::time::timeout(Duration::from_secs(60), crate::update::latest(&source))
                    .await
                {
                    Ok(Ok(release)) => {
                        let _ = remember(&pool, &release).await;
                        CHECK_EVERY
                    }
                    Ok(Err(error)) => {
                        crate::log::warn(format!("agent: {error}"));
                        Duration::from_secs(3600)
                    }
                    Err(_) => {
                        crate::log::warn(
                            "agent: the newest release did not answer within a minute",
                        );
                        Duration::from_secs(3600)
                    }
                }
            } else {
                Duration::from_secs(3600)
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// `scannerpulse agent`: runs until `shutdown`.
pub async fn run(
    pool: PgPool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let host = host();
    RUNNING.store(true, Ordering::Relaxed);
    // A request that changes the settings (a move) leaves this helper on
    // the old ones until it restarts.
    let settings = || std::fs::read(crate::setup::steps::env_path()).ok();
    let started_with = settings();
    spawn_beat(pool.clone(), host.clone());
    spawn_check(pool.clone());
    crate::clocks::spawn_nodes(pool.clone());
    // Until the requests it left unfinished are closed, it takes none: the
    // database may not answer at once (the pool finds the primary first).
    let mut cleaned = false;
    tokio::pin!(shutdown);
    loop {
        if !cleaned {
            match abandoned(&pool, &host).await {
                Ok(closed) => {
                    cleaned = true;
                    if closed > 0 {
                        crate::log::warn(format!(
                            "agent: closed {closed} request(s) left unfinished when the helper last stopped"
                        ));
                    }
                }
                Err(error) => {
                    crate::log::warn(format!("agent: cannot read the requests yet: {error}"))
                }
            }
        }
        let claimed = if cleaned {
            claim(&pool, &host).await
        } else {
            Ok(None)
        };
        match claimed {
            Ok(Some((id, kind))) => {
                crate::log::info(format!("agent: request {id}, {}", kind.name()));
                let (ok, line) = carry_out(&pool, id, kind).await;
                // Closed before anything else is taken, however long the
                // database takes to answer (a switchover moves it).
                while let Err(error) = finish(&pool, id, ok, &line).await {
                    crate::log::warn(format!("agent: cannot close request {id} yet: {error}"));
                    tokio::time::sleep(EVERY).await;
                }
                // A new program in place, or new settings: this helper runs
                // the old ones still.
                let new_program = kind == Kind::Update
                    && crate::update::version_of(Path::new(crate::setup::BINARY))
                        .is_some_and(|version| version != crate::update::VERSION);
                if new_program || settings() != started_with {
                    crate::log::info("agent: restarting into the new program or settings");
                    let _ = std::process::Command::new("systemctl")
                        .args(["--no-block", "restart", "scannerpulse-agent.service"])
                        .status();
                }
                continue;
            }
            Ok(None) => {}
            Err(error) => crate::log::warn(format!("agent: cannot read the requests: {error}")),
        }
        tokio::select! {
            () = &mut shutdown => return Ok(()),
            () = tokio::time::sleep(EVERY) => {}
        }
    }
}

/// The helper's unit: root, as it changes the machine; restarted if it stops.
pub fn unit(binary: &std::path::Path) -> String {
    format!(
        "[Unit]\n\
         Description=ScannerPulse's root helper: updates and switchovers an admin asks for\n\
         Wants=network-online.target\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         EnvironmentFile={}\n\
         ExecStart={} agent\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        crate::setup::steps::env_path().display(),
        binary.display()
    )
}

/// `scannerpulse agent`, as root from its unit: the database from the
/// settings (following the primary, as the web service does), until
/// `shutdown`.
pub async fn serve(shutdown: impl std::future::Future<Output = ()>) -> Result<(), String> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let database = url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|error| format!("DATABASE_URL is not a database URL: {error}"))?;
    let pool = crate::primary::pool(
        &database,
        &crate::primary::from_env()?,
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(5)),
    );
    crate::settings::follow_syslog(pool.clone(), "scannerpulse-agent");
    run(pool, shutdown).await
}

/// `scannerpulse agent install`, as root: the helper's unit written and
/// started on the first ScannerPulse machine or one machine alone. Setup
/// makes it; an install from before 0.9.0 gets it so, once. The second
/// machine has none: the helper runs where the plan is.
pub fn install() -> Result<String, String> {
    let settings = std::fs::read_to_string(crate::setup::steps::env_path())
        .map_err(|_| "this machine runs no ScannerPulse site: nothing to do".to_string())?;
    let plan_here = std::path::Path::new(crate::setup::plan::PLAN).exists();
    if crate::setup::second::is_second(&settings, plan_here) {
        return Ok(
            "This is the second ScannerPulse machine: the root helper runs on the first.".into(),
        );
    }
    std::fs::write(UNIT, unit(std::path::Path::new(crate::setup::BINARY)))
        .map_err(|error| format!("{UNIT}: {error}"))?;
    for args in [
        &["daemon-reload"][..],
        &["enable", "--now", "scannerpulse-agent.service"][..],
    ] {
        let output = std::process::Command::new("systemctl")
            .args(args)
            .output()
            .map_err(|error| format!("cannot run systemctl: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "systemctl {}: {}",
                args.join(" "),
                crate::setup::apply::said(&output)
            ));
        }
    }
    Ok("The root helper runs: the admin page's Update now and switchover reach it.".into())
}
