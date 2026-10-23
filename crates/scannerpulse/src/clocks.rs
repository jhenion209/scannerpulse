//! Clock checks. A call's time comes from its recorder's clock, written in
//! its file's name to the second, and two recorders' copies of one call are
//! matched by their times: a clock a second or two off can keep one call as
//! two. Every machine of the install is checked: whether its time service
//! runs and is synchronized, and how far its clock is from the primary
//! database's. A recorder is measured from its files. The checks only warn:
//! they never set a clock.

use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

/// How often each machine's clock is checked.
pub const CHECK_EVERY: Duration = Duration::from_secs(600);

/// How far a machine's clock may be from the primary database's before it
/// is named: its time service should hold it within milliseconds.
pub const LIMIT_MS: i64 = 1_000;

/// A machine whose clock was last checked longer ago than this is named.
pub const STALE_MINUTES: i64 = 30;

/// How long a database machine or the referee may take to answer.
const ASK_LIMIT: Duration = Duration::from_secs(20);

/// One machine's clock as checked: its role (`scannerpulse`, `database` or
/// `referee`), its time service, its offset from the primary database's
/// clock in milliseconds, or why it could not be checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub host: String,
    pub role: String,
    pub ntp: Option<bool>,
    pub synchronized: Option<bool>,
    pub offset_ms: Option<i64>,
    pub problem: Option<String>,
}

/// A clock to look at: the machine or recorder, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub subject: String,
    pub text: String,
}

/// What a machine says of its clock: whether its time service runs and has
/// synchronized with NTP (none when `timedatectl` cannot tell), and its time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clock {
    pub ntp: Option<bool>,
    pub synchronized: Option<bool>,
    /// Milliseconds since the epoch.
    pub now_ms: i64,
}

/// The time service's state in `timedatectl show -p NTP -p NTPSynchronized`'s
/// answer: whether it is on, and whether it has synchronized.
pub fn time_service(report: &str) -> (Option<bool>, Option<bool>) {
    let field = |name: &str| {
        report
            .lines()
            .find_map(|line| line.trim().strip_prefix(name)?.strip_prefix('='))
            .map(|value| value.trim() == "yes")
    };
    (field("NTP"), field("NTPSynchronized"))
}

/// This machine's clock now.
pub fn clock() -> Clock {
    let report = Command::new("timedatectl")
        .args(["show", "-p", "NTP", "-p", "NTPSynchronized"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let (ntp, synchronized) = time_service(&report);
    Clock {
        ntp,
        synchronized,
        now_ms: chrono::Utc::now().timestamp_millis(),
    }
}

/// How far another clock, read as `theirs` between `sent` and `answered` on
/// this one, is ahead of this one: theirs less the midpoint of the round
/// trip, in milliseconds.
pub fn offset(sent: i64, answered: i64, theirs: i64) -> i64 {
    theirs - (sent + answered) / 2
}

/// Keeps a machine's check, in place of its last.
pub async fn record(db: impl sqlx::PgExecutor<'_>, checked: &Checked) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO clock_checks (host, role, ntp, synchronized, offset_ms, problem, checked_at) \
         VALUES ($1, $2, $3, $4, $5, $6, now()) \
         ON CONFLICT (host) DO UPDATE SET role = EXCLUDED.role, ntp = EXCLUDED.ntp, \
             synchronized = EXCLUDED.synchronized, offset_ms = EXCLUDED.offset_ms, \
             problem = EXCLUDED.problem, checked_at = EXCLUDED.checked_at",
    )
    .bind(&checked.host)
    .bind(&checked.role)
    .bind(checked.ntp)
    .bind(checked.synchronized)
    .bind(checked.offset_ms)
    .bind(&checked.problem)
    .execute(db)
    .await?;
    Ok(())
}

/// This machine's clock against the primary database's: the database's time
/// read between two readings of this one.
async fn from_database(pool: &PgPool) -> sqlx::Result<i64> {
    let sent = chrono::Utc::now().timestamp_millis();
    let theirs: i64 =
        sqlx::query_scalar("SELECT (extract(epoch FROM clock_timestamp()) * 1000)::bigint")
            .fetch_one(pool)
            .await?;
    let answered = chrono::Utc::now().timestamp_millis();
    // The database's clock less this one's; this one's less the database's
    // is its opposite.
    Ok(-offset(sent, answered, theirs))
}

/// A ScannerPulse machine's own check: its time service, and its clock
/// against the primary database's.
pub async fn own(pool: &PgPool, host: &str) -> sqlx::Result<Checked> {
    let here = tokio::task::spawn_blocking(clock)
        .await
        .map_err(|error| sqlx::Error::Protocol(error.to_string()))?;
    Ok(Checked {
        host: host.to_string(),
        role: "scannerpulse".into(),
        ntp: here.ntp,
        synchronized: here.synchronized,
        offset_ms: Some(from_database(pool).await?),
        problem: None,
    })
}

/// A database machine's or the referee's check, asked through the hub's
/// restricted key (`node serve`'s `clock`), for [`ASK_LIMIT`] at most; `mine`
/// is this machine's clock less the database's.
pub async fn asked(host: &str, role: &str, mine: i64) -> Checked {
    let (node, sent) = (host.to_string(), chrono::Utc::now().timestamp_millis());
    let answer = tokio::time::timeout(
        ASK_LIMIT,
        tokio::task::spawn_blocking(move || crate::setup::node::ask(&node, "clock", &[])),
    )
    .await;
    let answered = chrono::Utc::now().timestamp_millis();
    let unchecked = |problem: String| Checked {
        host: host.to_string(),
        role: role.to_string(),
        ntp: None,
        synchronized: None,
        offset_ms: None,
        problem: Some(problem),
    };
    let text = match answer {
        Err(_) => return unchecked(format!("did not answer within {} s", ASK_LIMIT.as_secs())),
        Ok(Err(error)) => return unchecked(error.to_string()),
        Ok(Ok(Err(why))) => return unchecked(why),
        Ok(Ok(Ok(text))) => text,
    };
    match serde_json::from_str::<Clock>(text.trim()) {
        Ok(theirs) => Checked {
            host: host.to_string(),
            role: role.to_string(),
            ntp: theirs.ntp,
            synchronized: theirs.synchronized,
            // Theirs less mine, plus mine less the database's.
            offset_ms: Some(offset(sent, answered, theirs.now_ms) + mine),
            problem: None,
        },
        Err(_) => unchecked(format!("answered {:?}, not a clock", text.trim())),
    }
}

/// Checks the database machines' and the referee's clocks every
/// [`CHECK_EVERY`]: the first machine's agent, which holds the hub's key.
pub fn spawn_nodes(pool: PgPool) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = check_nodes(&pool).await {
                crate::log::warn(format!(
                    "cannot check the database machines' clocks: {error}"
                ));
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

async fn check_nodes(pool: &PgPool) -> sqlx::Result<()> {
    let nodes = crate::primary::from_env().unwrap_or_default();
    let referee = crate::layout::Layout::read(std::path::Path::new(crate::layout::PATH))
        .and_then(|layout| layout.referee);
    if nodes.is_empty() && referee.is_none() {
        return Ok(());
    }
    let mine = from_database(pool).await?;
    let asked_of = nodes
        .iter()
        .map(|node| (node.host.clone(), "database"))
        .chain(referee.map(|host| (host, "referee")));
    for (host, role) in asked_of {
        record(pool, &asked(&host, role, mine).await).await?;
    }
    Ok(())
}

/// A length in seconds, to the tenth: `2.4 s`.
fn seconds(ms: i64) -> String {
    format!("{:.1} s", ms.abs() as f64 / 1000.0)
}

/// The machines whose clocks need a look, by name: a clock not synchronized
/// (its time service off or not), an offset from the database's of
/// [`LIMIT_MS`] or more, a check that failed, or none for [`STALE_MINUTES`].
pub async fn machine_warnings(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<Warning>> {
    let rows: Vec<(Checked, f64)> = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<bool>,
            Option<bool>,
            Option<i64>,
            Option<String>,
            f64,
        ),
    >(
        "SELECT host, role, ntp, synchronized, offset_ms, problem, \
                EXTRACT(EPOCH FROM now() - checked_at)::float8 FROM clock_checks ORDER BY host",
    )
    .fetch_all(db)
    .await?
    .into_iter()
    .map(|(host, role, ntp, synchronized, offset_ms, problem, age)| {
        (
            Checked {
                host,
                role,
                ntp,
                synchronized,
                offset_ms,
                problem,
            },
            age,
        )
    })
    .collect();
    let mut warnings = Vec::new();
    for (checked, age) in rows {
        let minutes = (age / 60.0).round() as i64;
        let text = if minutes >= STALE_MINUTES {
            Some(format!("its clock was last checked {minutes} minutes ago"))
        } else if let Some(problem) = &checked.problem {
            Some(format!("its clock could not be checked: {problem}"))
        } else if checked.synchronized == Some(false) {
            // A container has no time service of its own (its host's keeps
            // its clock), nor has a machine whose clock chrony keeps: off
            // alone is not named.
            Some(if checked.ntp == Some(false) {
                "its time service is off".into()
            } else {
                "its clock is not synchronized by NTP".into()
            })
        } else {
            checked
                .offset_ms
                .filter(|ms| ms.abs() >= LIMIT_MS)
                .map(|ms| {
                    format!(
                        "its clock is {} {} the database's",
                        seconds(ms),
                        if ms < 0 { "behind" } else { "ahead of" }
                    )
                })
        };
        if let Some(text) = text {
            warnings.push(Warning {
                subject: checked.host,
                text,
            });
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_service_is_read_from_timedatectl() {
        assert_eq!(
            time_service("NTP=yes\nNTPSynchronized=no\n"),
            (Some(true), Some(false))
        );
        assert_eq!(
            time_service("NTPSynchronized=yes\nNTP=no\n"),
            (Some(false), Some(true))
        );
        // A machine without systemd's time service says nothing.
        assert_eq!(time_service(""), (None, None));
    }

    #[test]
    fn a_clock_is_measured_against_the_midpoint_of_the_round_trip() {
        // Asked at 1000, answered at 1200: their 1600 is 500 ahead.
        assert_eq!(offset(1000, 1200, 1600), 500);
        assert_eq!(offset(1000, 1200, 900), -200);
    }
}
