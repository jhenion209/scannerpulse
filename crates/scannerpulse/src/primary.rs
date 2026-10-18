//! A database on two nodes, a primary and its standby: every process is
//! given each node (SCANNERPULSE_DATABASE_HOSTS), works on the one that is
//! the primary, and moves to the new primary after a failover or a
//! switchover. With one node nothing of this runs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{Connection, PgConnection, PgPool};

pub const HOSTS: &str = "SCANNERPULSE_DATABASE_HOSTS";

/// How long a node gets to answer whether it is the primary.
const ASK_LIMIT: Duration = Duration::from_secs(2);

/// How often a pool looks for the primary.
const LOOK_EVERY: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub host: String,
    pub port: u16,
}

/// The nodes in `list`: `host`, `host:port` or `[address]:port`, separated
/// by spaces; the port is 5432 unless given.
pub fn nodes(list: &str) -> Result<Vec<Node>, String> {
    let bad = |item: &str| format!("{HOSTS}: {item:?} is not host:port");
    list.split_whitespace()
        .map(|item| {
            let (host, port) = if let Some(rest) = item.strip_prefix('[') {
                let (host, after) = rest.split_once(']').ok_or_else(|| bad(item))?;
                (host, after.strip_prefix(':'))
            } else {
                match item.rsplit_once(':') {
                    Some((host, port)) => (host, Some(port)),
                    None => (item, None),
                }
            };
            let port = match port {
                None => 5432,
                Some(port) => port
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p > 0)
                    .ok_or_else(|| bad(item))?,
            };
            if host.is_empty() {
                return Err(bad(item));
            }
            Ok(Node {
                host: host.to_string(),
                port,
            })
        })
        .collect()
}

/// The nodes the settings name, if more than the database address's own.
pub fn from_env() -> Result<Vec<Node>, String> {
    nodes(&std::env::var(HOSTS).unwrap_or_default())
}

fn on(options: &PgConnectOptions, node: &Node) -> PgConnectOptions {
    options.clone().host(&node.host).port(node.port)
}

/// A primary's current WAL file; nothing (NULL) on a standby.
pub const CURRENT_WAL: &str = "SELECT CASE WHEN pg_is_in_recovery() THEN NULL \
     ELSE pg_walfile_name(pg_current_wal_lsn()) END";

/// The timeline in a WAL file's name: its first 8 hex digits.
pub fn timeline_of(wal: &str) -> Option<u32> {
    u32::from_str_radix(wal.get(..8)?, 16).ok()
}

/// The timeline of the node `options` reaches if it is a primary: it
/// answers within `ASK_LIMIT` and is not a standby. Each promotion starts a
/// newer timeline.
async fn timeline(options: &PgConnectOptions) -> Option<u32> {
    let ask = async {
        let mut conn = PgConnection::connect_with(options).await?;
        let wal: Option<String> = sqlx::query_scalar(CURRENT_WAL).fetch_one(&mut conn).await?;
        let _ = conn.close().await;
        Ok::<_, sqlx::Error>(wal)
    };
    let wal = tokio::time::timeout(ASK_LIMIT, ask).await.ok()?.ok()??;
    timeline_of(&wal)
}

/// Which of the nodes' timelines (`None` for a standby or no answer) is
/// the primary's: the newest, so a former primary back after a failover
/// is never taken for it.
fn newest(timelines: &[Option<u32>]) -> Option<usize> {
    newest_from(timelines, 0)
}

/// The newest of the nodes' timelines that is at least `floor`, the newest
/// a pool has worked on: a pool never moves back to an older timeline, even
/// while the newest machine misses a look.
fn newest_from(timelines: &[Option<u32>], floor: u32) -> Option<usize> {
    timelines
        .iter()
        .enumerate()
        .filter_map(|(i, t)| Some((i, (*t).filter(|t| *t >= floor)?)))
        .max_by_key(|&(i, t)| (t, std::cmp::Reverse(i)))
        .map(|(i, _)| i)
}

async fn timelines(options: &PgConnectOptions, nodes: &[Node]) -> Vec<Option<u32>> {
    let mut timelines = Vec::with_capacity(nodes.len());
    for node in nodes {
        timelines.push(timeline(&on(options, node)).await);
    }
    timelines
}

/// Which of `nodes` is the primary now, asking each with `options` (the
/// name, the role, the TLS mode).
pub async fn find(options: &PgConnectOptions, nodes: &[Node]) -> Option<usize> {
    newest(&timelines(options, nodes).await)
}

/// A pool on the primary. With more than one node it starts on `options`,
/// looks for the primary at once and every `LOOK_EVERY` after, and moves
/// when it changes: a connection made before the move is dropped when it is
/// next taken, so nothing writes to a node that has become a standby.
pub fn pool(options: &PgConnectOptions, nodes: &[Node], settings: PgPoolOptions) -> PgPool {
    if nodes.len() < 2 {
        return settings.connect_lazy_with(options.clone());
    }
    let moved: Arc<Mutex<Option<Instant>>> = Arc::default();
    let since = moved.clone();
    let pool = settings
        .before_acquire(move |_, meta| {
            let since = *since.lock().unwrap_or_else(|e| e.into_inner());
            Box::pin(async move { Ok(since.is_none_or(|at| meta.age < at.elapsed())) })
        })
        .connect_lazy_with(options.clone());
    let (watched, nodes, base) = (pool.clone(), nodes.to_vec(), options.clone());
    tokio::spawn(async move {
        let mut current = nodes
            .iter()
            .position(|n| n.host == base.get_host() && n.port == base.get_port());
        let (mut floor, mut warned) = (0, None);
        loop {
            if watched.is_closed() {
                return;
            }
            let timelines = timelines(&base, &nodes).await;
            let stale = timelines.iter().position(|t| t.is_some_and(|t| t < floor));
            if stale != warned
                && let Some(i) = stale
            {
                crate::log::warn(format!(
                    "database: {} answers as a primary on an older timeline: run `sudo scannerpulse db rejoin` there",
                    nodes[i].host
                ));
            }
            warned = stale;
            let found = newest_from(&timelines, floor);
            if let Some(t) = found.and_then(|i| timelines[i]) {
                floor = t;
            }
            if let Some(found) = found
                && Some(found) != current
            {
                watched.set_connect_options(on(&base, &nodes[found]));
                *moved.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
                if current.is_some() {
                    crate::log::warn(format!(
                        "database: the primary is now {}:{}",
                        nodes[found].host, nodes[found].port
                    ));
                }
                current = Some(found);
            }
            tokio::time::sleep(LOOK_EVERY).await;
        }
    });
    pool
}

/// DATABASE_URL's options, on the node that is the primary when the
/// settings name more than one.
pub async fn options(url: &str) -> Result<PgConnectOptions, String> {
    let options: PgConnectOptions = url
        .parse()
        .map_err(|error| format!("DATABASE_URL is not a database URL: {error}"))?;
    let nodes = from_env()?;
    if nodes.len() < 2 {
        return Ok(options);
    }
    let found = find(&options, &nodes).await.ok_or_else(|| {
        let tried: Vec<String> = nodes
            .iter()
            .map(|n| format!("{}:{}", n.host, n.port))
            .collect();
        format!(
            "no database node answers as the primary ({})",
            tried.join(", ")
        )
    })?;
    Ok(on(&options, &nodes[found]))
}

/// One connection to the primary, for a command, within `limit`.
pub async fn connect(url: &str, limit: Duration) -> Result<PgConnection, String> {
    let options = options(url).await?;
    tokio::time::timeout(limit, PgConnection::connect_with(&options))
        .await
        .map_err(|_| {
            format!(
                "the database server did not answer within {} seconds",
                limit.as_secs()
            )
        })?
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{newest, newest_from};

    #[test]
    fn a_pool_never_moves_back_to_an_older_timeline() {
        // The promoted machine (timeline 4) missed a look while the former
        // primary (timeline 3) answers alone: the pool stays where it is.
        assert_eq!(newest_from(&[Some(3), None], 4), None);
        assert_eq!(newest_from(&[Some(3), Some(4)], 4), Some(1));
        // A newer promotion is followed.
        assert_eq!(newest_from(&[Some(5), None], 4), Some(0));
    }

    #[test]
    fn of_two_primaries_the_newest_timeline_is_the_primary() {
        // A former primary back after a failover answers as a primary on
        // the timeline it had; the machine promoted since is on a newer one.
        assert_eq!(newest(&[Some(3), Some(4)]), Some(1));
        assert_eq!(newest(&[Some(5), Some(4)]), Some(0));
        assert_eq!(newest(&[None, Some(2)]), Some(1));
        assert_eq!(newest(&[Some(2), None]), Some(0));
        assert_eq!(newest(&[Some(4), Some(4)]), Some(0));
        assert_eq!(newest(&[None, None]), None);
    }
}
