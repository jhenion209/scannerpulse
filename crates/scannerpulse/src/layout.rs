//! The install's layout, for the admin page: each ScannerPulse machine,
//! which holds the shared address and the processing lease, each database
//! machine's role and how far the standby is behind, whether the referee
//! answers, and the failover mode.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sqlx::{Connection, PgConnection, PgPool};

/// Where setup keeps the layout, beside its plan; readable by the services.
pub const PATH: &str = "/etc/scannerpulse/layout.json";

/// How often each ScannerPulse machine says it is up.
const BEAT_EVERY: Duration = Duration::from_secs(15);

/// A machine not heard from for this long is shown as not answering.
const UP_WITHIN_SECONDS: f64 = 45.0;

/// How long a database machine or the referee may take to answer.
const ASK_LIMIT: Duration = Duration::from_secs(3);

/// The plan's public part: no password. Setup writes it beside the plan on
/// the first machine; the second's join writes it there too.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    pub hub: String,
    pub nodes: Vec<String>,
    #[serde(default)]
    pub referee: Option<String>,
    #[serde(default)]
    pub second: Option<String>,
    #[serde(default)]
    pub shared: Option<String>,
    #[serde(default)]
    pub install_id: String,
}

impl Layout {
    pub fn read(path: &Path) -> Option<Layout> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    /// Written whole, then put in place, readable by all.
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let new = path.with_extension("json.new");
        let _ = std::fs::remove_file(&new);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&new)?;
        file.write_all(serde_json::to_string_pretty(self)?.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&new, path)
    }

    /// How the standby becomes the primary: "referee" (by itself) or
    /// "hand".
    pub fn mode(&self) -> &'static str {
        if self.referee.is_some() {
            "referee"
        } else {
            "hand"
        }
    }
}

/// `shared` (address/prefix) is among this machine's addresses, as
/// `ip -o -4 addr show` lists them (`listing`).
pub fn holds(listing: &str, shared: &str) -> bool {
    let address = shared.split('/').next().unwrap_or(shared);
    listing.lines().any(|line| {
        line.split_whitespace()
            .any(|word| word.split_once('/').is_some_and(|(a, _)| a == address))
    })
}

/// A WAL position ("16/B374D848") as a number of bytes.
pub fn lsn(text: &str) -> Option<u64> {
    let (high, low) = text.trim().split_once('/')?;
    Some((u64::from_str_radix(high, 16).ok()? << 32) | u64::from_str_radix(low, 16).ok()?)
}

/// How many bytes the standby at `standby` is behind the primary at
/// `primary`; none when either is not a position.
pub fn behind(primary: &str, standby: &str) -> Option<u64> {
    Some(lsn(primary)?.saturating_sub(lsn(standby)?))
}

/// This machine holds the shared address `layout` names.
fn holds_here(layout: Option<&Layout>) -> bool {
    let Some(shared) = layout.and_then(|l| l.shared.as_deref()) else {
        return false;
    };
    std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| holds(&String::from_utf8_lossy(&output.stdout), shared))
}

/// Says every `BEAT_EVERY` that this machine (`host`) is up: its version,
/// and whether it holds the shared address.
pub fn spawn_beat(pool: PgPool, host: String, layout: Option<PathBuf>) {
    tokio::spawn(async move {
        loop {
            let found = layout.as_deref().and_then(Layout::read);
            let holding = holds_here(found.as_ref());
            let beat = sqlx::query(
                "INSERT INTO machines (host, version, holds_shared, seen_at) \
                 VALUES ($1, $2, $3, now()) \
                 ON CONFLICT (host) DO UPDATE SET version = EXCLUDED.version, \
                 holds_shared = EXCLUDED.holds_shared, seen_at = EXCLUDED.seen_at",
            )
            .bind(&host)
            .bind(env!("CARGO_PKG_VERSION"))
            .bind(holding)
            .execute(&pool)
            .await;
            if let Err(error) = beat {
                crate::log::warn(format!("cannot say this machine is up: {error}"));
            }
            if let Err(error) = forget_stale(&pool).await {
                crate::log::warn(format!("cannot forget machines gone for good: {error}"));
            }
            tokio::time::sleep(BEAT_EVERY).await;
        }
    });
}

/// Forgets a machine not heard from for a week, such as a replaced second,
/// so it is not listed for ever.
pub async fn forget_stale(pool: &PgPool) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM machines WHERE seen_at < now() - interval '7 days'")
        .execute(pool)
        .await?;
    Ok(())
}

/// One database machine as it answers: whether it is a standby, its WAL
/// position, for a standby the time since its last replayed write, and for
/// a primary its timeline.
type Answer = Result<(bool, String, Option<f64>, Option<u32>), String>;

async fn ask_node(
    options: &sqlx::postgres::PgConnectOptions,
    node: &crate::primary::Node,
) -> Answer {
    let options = options.clone().host(&node.host).port(node.port);
    let ask = async {
        let mut conn = PgConnection::connect_with(&options).await?;
        let answer: (bool, String, Option<f64>) = sqlx::query_as(
            "SELECT pg_is_in_recovery(), \
                    (CASE WHEN pg_is_in_recovery() THEN pg_last_wal_replay_lsn() \
                     ELSE pg_current_wal_lsn() END)::text, \
                    EXTRACT(EPOCH FROM now() - pg_last_xact_replay_timestamp())::float8",
        )
        .fetch_one(&mut conn)
        .await?;
        let wal: Option<String> = sqlx::query_scalar(crate::primary::CURRENT_WAL)
            .fetch_one(&mut conn)
            .await?;
        let _ = conn.close().await;
        let timeline = wal.as_deref().and_then(crate::primary::timeline_of);
        Ok::<_, sqlx::Error>((answer.0, answer.1, answer.2, timeline))
    };
    match tokio::time::timeout(ASK_LIMIT, ask).await {
        Err(_) => Err(format!("no answer within {} s", ASK_LIMIT.as_secs())),
        Ok(Err(error)) => Err(error.to_string()),
        Ok(Ok(answer)) => Ok(answer),
    }
}

/// Each machine's role (none when it did not answer) and the position of
/// the primary: the one on the newest timeline. Any other machine that
/// answers as a primary is a former one, back after a hand promotion and
/// waiting for `db rejoin`.
fn roles(answers: &[Answer]) -> (Vec<Option<&'static str>>, Option<String>) {
    let timelines: Vec<Option<u32>> = answers
        .iter()
        .map(|answer| match answer {
            Ok((false, _, _, timeline)) => *timeline,
            _ => None,
        })
        .collect();
    let primary = crate::primary::newest(&timelines)
        .or_else(|| answers.iter().position(|a| matches!(a, Ok((false, ..)))));
    let roles = answers
        .iter()
        .enumerate()
        .map(|(i, answer)| match answer {
            Ok((false, ..)) if Some(i) == primary => Some("primary"),
            Ok((false, ..)) => Some("former_primary"),
            Ok((true, ..)) => Some("standby"),
            Err(_) => None,
        })
        .collect();
    let at = primary.and_then(|i| answers[i].as_ref().ok().map(|a| a.1.clone()));
    (roles, at)
}

/// The referee answers on its port.
async fn ask_referee(host: &str) -> Result<(), String> {
    match tokio::time::timeout(ASK_LIMIT, tokio::net::TcpStream::connect((host, 5432))).await {
        Err(_) => Err(format!("no answer within {} s", ASK_LIMIT.as_secs())),
        Ok(Err(error)) => Err(error.to_string()),
        Ok(Ok(_)) => Ok(()),
    }
}

/// The layout as the admin page shows it, from `layout` (the file setup
/// writes), `nodes` (the database machines the settings name) and the
/// database's word on each ScannerPulse machine and the lease.
pub async fn report(
    pool: &PgPool,
    layout: Option<&Layout>,
    nodes: &[crate::primary::Node],
) -> sqlx::Result<Value> {
    let mut conn = pool.acquire().await?;
    let lease = crate::lease::status(&mut *conn).await?;
    let lease_host = lease
        .valid()
        .then_some(lease.holder.as_deref())
        .flatten()
        .and_then(|holder| holder.split_whitespace().next())
        .map(String::from);
    let rows: Vec<(String, String, bool, f64)> = sqlx::query_as(
        "SELECT host, version, holds_shared, EXTRACT(EPOCH FROM now() - seen_at)::float8 \
         FROM machines ORDER BY host",
    )
    .fetch_all(&mut *conn)
    .await?;
    drop(conn);
    let mut machines: Vec<Value> = rows
        .into_iter()
        .map(|(host, version, holds_shared, age)| {
            let up = age < UP_WITHIN_SECONDS;
            json!({
                "host": host,
                "version": version,
                "seen_seconds_ago": age.round(),
                "up": up,
                "holds_shared": up && holds_shared,
                "holds_lease": lease_host.as_deref() == Some(host.as_str()),
            })
        })
        .collect();
    // A second that has never said it is up (a version before 0.7.0, or its
    // web has not started) is listed as down, not left out.
    if let Some(second) = layout.and_then(|l| l.second.as_deref())
        && !machines.iter().any(|m| m["host"] == second)
    {
        machines.push(json!({
            "host": second,
            "version": null,
            "seen_seconds_ago": null,
            "up": false,
            "holds_shared": false,
            "holds_lease": false,
        }));
    }
    let options = pool.connect_options();
    let mut answers = Vec::with_capacity(nodes.len());
    for node in nodes {
        answers.push(ask_node(&options, node).await);
    }
    let (roles, primary_at) = roles(&answers);
    let database: Vec<Value> = nodes
        .iter()
        .zip(answers)
        .zip(roles)
        .map(|((node, answer), role)| match (answer, role) {
            (Ok((false, at, ..)), Some(role)) => json!({"host": node.host, "role": role, "at": at}),
            (Ok((_, at, replayed, _)), _) => json!({
                "host": node.host,
                "role": "standby",
                "at": at,
                "behind_bytes": primary_at.as_deref().and_then(|p| behind(p, &at)),
                "replayed_seconds_ago": replayed.map(f64::round),
            }),
            (Err(why), _) => json!({"host": node.host, "role": null, "error": why}),
        })
        .collect();
    let referee = match layout.and_then(|l| l.referee.as_deref()) {
        None => Value::Null,
        Some(host) => match ask_referee(host).await {
            Ok(()) => json!({"host": host, "answers": true}),
            Err(why) => json!({"host": host, "answers": false, "error": why}),
        },
    };
    let mode = match layout {
        Some(layout) => layout.mode(),
        None if nodes.is_empty() => "one",
        None => "unknown",
    };
    Ok(json!({
        "mode": mode,
        "first": layout.map(|l| l.hub.clone()),
        "second": layout.and_then(|l| l.second.clone()),
        "shared": layout.and_then(|l| l.shared.clone()),
        "machines": machines,
        "lease": lease_host,
        "database": database,
        "referee": referee,
        "switchover": (mode == "hand" || mode == "referee")
            .then_some("sudo /usr/local/bin/scannerpulse db switchover"),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_is_the_plans_public_part() {
        let layout = Layout {
            hub: "192.0.2.10".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            referee: Some("ref.example.org".into()),
            second: Some("web2.example.org".into()),
            shared: Some("192.0.2.50/24".into()),
            install_id: "0123456789abcdef".into(),
        };
        assert_eq!(layout.mode(), "referee");
        assert_eq!(
            Layout {
                referee: None,
                ..layout.clone()
            }
            .mode(),
            "hand"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layout.json");
        layout.write(&path).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert_eq!(Layout::read(&path), Some(layout));
        assert_eq!(Layout::read(&dir.path().join("none.json")), None);
    }

    #[test]
    fn this_machine_holds_the_shared_address_when_its_interface_has_it() {
        let listing = "1: lo    inet 127.0.0.1/8 scope host lo\n2: host0    inet 192.0.2.12/24 brd 192.0.2.255 scope global host0\n2: host0    inet 192.0.2.50/24 scope global secondary host0\n";
        assert!(holds(listing, "192.0.2.50/24"));
        assert!(!holds(listing, "192.0.2.5/24"));
        assert!(!holds(listing, "192.0.2.60/24"));
    }

    #[test]
    fn a_standby_is_behind_by_the_distance_between_the_lsns() {
        assert_eq!(lsn("16/B374D848"), Some(0x16_B374_D848));
        assert_eq!(lsn("0/0"), Some(0));
        assert_eq!(lsn("nonsense"), None);
        assert_eq!(behind("16/B374D848", "16/B374D000"), Some(0x848));
        // A standby ahead of a primary read a moment earlier is not behind.
        assert_eq!(behind("16/B374D000", "16/B374D848"), Some(0));
    }

    #[test]
    fn a_former_primary_is_told_from_the_primary_by_its_timeline() {
        let primary = |at: &str, timeline| Ok((false, at.to_string(), None, Some(timeline)));
        let standby = |at: &str| Ok((true, at.to_string(), Some(2.0), None));
        // The old primary is back on timeline 1; the promoted one is on 2.
        let (got, at) = roles(&[primary("0/5000000", 1), primary("0/9000000", 2)]);
        assert_eq!(got, [Some("former_primary"), Some("primary")]);
        assert_eq!(at.as_deref(), Some("0/9000000"));
        // One primary and one standby, either way round, and a silent one.
        let (got, at) = roles(&[standby("0/8FFF000"), primary("0/9000000", 2)]);
        assert_eq!(got, [Some("standby"), Some("primary")]);
        assert_eq!(at.as_deref(), Some("0/9000000"));
        let (got, at) = roles(&[Err("down".into()), standby("0/1")]);
        assert_eq!(got, [None, Some("standby")]);
        assert_eq!(at, None);
    }
}
