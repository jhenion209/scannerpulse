//! `scannerpulse agent`: the root helper on the first ScannerPulse machine (or
//! one machine alone). The web service runs without root, so an admin's
//! "Update now" or switchover is a row in `agent_requests` that this takes,
//! runs, and writes what it says into, line by line: the admin page reads it
//! through the restarts an update makes. It does nothing else an admin did
//! not ask for, except to look for a newer release once a day, unless
//! `updates.check` is false.

use serde::Serialize;
use sqlx::PgPool;
use std::time::Duration;

/// The helper's unit, on the first ScannerPulse machine only.
pub const UNIT: &str = "/etc/systemd/system/scannerpulse-agent.service";

/// How often it says it is up and looks for a request.
const EVERY: Duration = Duration::from_secs(5);
/// How often it looks for a newer release.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 3600);
/// A helper not heard from for this long is shown as not running.
pub const SILENT_AFTER: i64 = 30;

/// What an admin may ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// `scannerpulse update`, every machine in turn.
    Update,
    /// `scannerpulse db switchover`: the standby becomes the primary.
    Switchover,
}

impl Kind {
    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "update" => Some(Kind::Update),
            "switchover" => Some(Kind::Switchover),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Update => "update",
            Kind::Switchover => "switchover",
        }
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

/// Takes the oldest request no helper has started, for `host`.
pub async fn claim(pool: &PgPool, host: &str) -> sqlx::Result<Option<(i64, Kind)>> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "UPDATE agent_requests SET started_at = now(), host = $1 \
         WHERE id = (SELECT id FROM agent_requests WHERE started_at IS NULL \
                     ORDER BY id LIMIT 1 FOR UPDATE SKIP LOCKED) \
         RETURNING id, kind",
    )
    .bind(host)
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

/// What request `id` runs: the update or the switchover, each line it says
/// written to the request as it comes. The update restarts this machine's
/// web and ingest, never this helper, which restarts itself afterwards.
async fn carry_out(pool: &PgPool, id: i64, kind: Kind) -> (bool, String) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = {
        let pool = pool.clone();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                crate::log::info(format!("agent: {line}"));
                let _ = say(&pool, id, &line).await;
            }
        })
    };
    let said = move |line: &str| {
        let _ = tx.send(line.to_string());
    };
    let result = match kind {
        Kind::Update => crate::update::run(false, true, said, |_| true).await,
        Kind::Switchover => {
            tokio::task::spawn_blocking(move || crate::setup::node::db_switchover(said))
                .await
                .unwrap_or_else(|error| Err(format!("the switchover stopped: {error}")))
        }
    };
    let _ = writer.await;
    match result {
        Ok(done) => (true, done),
        Err(error) => (false, format!("{}: {error}", kind.name())),
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

/// `scannerpulse agent`: runs until `shutdown`.
pub async fn run(
    pool: PgPool,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), String> {
    let host = host();
    // Until the requests it left unfinished are closed, it takes none: the
    // database may not answer at once (the pool finds the primary first).
    let mut cleaned = false;
    let mut checked: Option<std::time::Instant> = None;
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
        if let Err(error) = beat(&pool, &host).await {
            crate::log::warn(format!("agent: cannot say the helper is up: {error}"));
        }
        if checked.is_none_or(|at| at.elapsed() > CHECK_EVERY)
            && checks(&pool).await.unwrap_or(false)
        {
            match crate::update::latest(&crate::update::Source::from_env()).await {
                Ok(release) => {
                    let _ = remember(&pool, &release).await;
                    checked = Some(std::time::Instant::now());
                }
                // Tried again within the hour.
                Err(error) => {
                    crate::log::warn(format!("agent: {error}"));
                    checked =
                        Some(std::time::Instant::now() - CHECK_EVERY + Duration::from_secs(3600));
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
                let _ = finish(&pool, id, ok, &line).await;
                // A new program in place: this helper runs the old one still.
                if kind == Kind::Update
                    && crate::update::version_of(std::path::Path::new(crate::setup::BINARY))
                        .is_some_and(|version| version != crate::update::VERSION)
                {
                    crate::log::info("agent: restarting into the new program");
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
