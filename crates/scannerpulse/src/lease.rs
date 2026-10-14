//! The processing lease (today's HA Method B): one host at a time files and
//! transcribes. The row `processing` names the holder until it expires, and
//! every running host keeps a heartbeat row `ingest:<host>`. All times are the
//! database's clock, so the hosts' clocks never matter.
//!
//! - The holder renews every `renew` with a lifetime of `ttl`.
//! - The preferred host takes an expired lease at once; the other waits
//!   `grace` more, so that a short database failover does not move the work.
//! - A holder that is not preferred hands the lease back once the other
//!   host's heartbeat has run for `return_after` without a gap and from the
//!   same process (a new process restarts the count).
//!
//! Fencing: `held` compares the monotonic clock with a deadline taken just
//! before the last successful renewal was sent, minus a margin. It needs no
//! database call, and a process that was frozen and resumes finds its deadline
//! passed. `store::record` checks the lease row again in its transaction.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sqlx::PgExecutor;
use sqlx::postgres::PgConnection;

use crate::filer::Fence;
use crate::store::PROCESSING_LEASE;

/// The heartbeat row of a host is `ingest:<host>`.
pub const HEARTBEAT_PREFIX: &str = "ingest:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub renew: Duration,
    pub ttl: Duration,
    pub grace: Duration,
    pub return_after: Duration,
    /// Taken off the local deadline, for the time a renewal takes.
    pub margin: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            renew: Duration::from_secs(5),
            ttl: Duration::from_secs(15),
            grace: Duration::from_secs(10),
            return_after: Duration::from_secs(120),
            margin: Duration::from_secs(2),
        }
    }
}

#[derive(Default)]
struct State {
    deadline: Option<Instant>,
    yielding: bool,
    /// The other host's heartbeat, as its row holds it, and since when it has
    /// been seen without a gap.
    peer: Option<(String, Instant)>,
}

pub struct Lease {
    host: String,
    me: String,
    preferred: bool,
    timings: Timings,
    state: Mutex<State>,
}

impl Lease {
    /// The lease of this process on `host`.
    pub fn new(host: &str, preferred: bool, timings: Timings) -> Lease {
        let start = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let token = format!("{}-{start}", std::process::id());
        Lease::with_token(host, &token, preferred, timings)
    }

    /// `token` tells this process from any other on the same host.
    pub fn with_token(host: &str, token: &str, preferred: bool, timings: Timings) -> Lease {
        Lease {
            host: host.to_string(),
            me: format!("{host} {token}"),
            preferred,
            timings,
            state: Mutex::new(State::default()),
        }
    }

    pub fn timings(&self) -> Timings {
        self.timings
    }

    /// `<host> <token>`, as the lease rows hold it.
    pub fn holder(&self) -> &str {
        &self.me
    }

    pub fn held(&self) -> bool {
        self.state()
            .deadline
            .is_some_and(|deadline| Instant::now() < deadline)
    }

    /// Held, and not handing the lease back.
    pub fn should_work(&self) -> bool {
        let state = self.state();
        state
            .deadline
            .is_some_and(|deadline| Instant::now() < deadline)
            && !state.yielding
    }

    /// Whether the lease is to go back to the preferred host: call
    /// `hand_over` between files.
    pub fn yielding(&self) -> bool {
        self.state().yielding
    }

    /// One round: the heartbeat, the lease, and the watch on the other host.
    /// A host whose paths fail (`healthy` false) gives the lease up and stops
    /// its heartbeat, so the other host neither waits for it nor hands the
    /// lease back to it. After a database error the deadline of the last good
    /// renewal runs out by itself.
    pub async fn tick(&self, conn: &mut PgConnection, healthy: bool) -> sqlx::Result<()> {
        let result = self.round(conn, healthy).await;
        if result.is_err() {
            let mut state = self.state();
            state.yielding = false;
            state.peer = None;
        }
        result
    }

    async fn round(&self, conn: &mut PgConnection, healthy: bool) -> sqlx::Result<()> {
        if !healthy {
            return self.release(conn).await;
        }
        sqlx::query(
            "INSERT INTO leases (name, holder, expires_at, renewed_at) \
             VALUES ($1, $2, now() + make_interval(secs => $3), now()) \
             ON CONFLICT (name) DO UPDATE SET holder = EXCLUDED.holder, \
                 expires_at = EXCLUDED.expires_at, renewed_at = EXCLUDED.renewed_at",
        )
        .bind(self.heartbeat_name())
        .bind(&self.me)
        .bind(self.timings.ttl.as_secs_f64())
        .execute(&mut *conn)
        .await?;
        let sent = Instant::now();
        let grace = if self.preferred {
            Duration::ZERO
        } else {
            self.timings.grace
        };
        let taken: Option<String> = sqlx::query_scalar(
            "INSERT INTO leases (name, holder, expires_at, renewed_at) \
             VALUES ($1, $2, now() + make_interval(secs => $3), now()) \
             ON CONFLICT (name) DO UPDATE SET holder = EXCLUDED.holder, \
                 expires_at = EXCLUDED.expires_at, renewed_at = EXCLUDED.renewed_at \
             WHERE leases.holder = EXCLUDED.holder \
                OR leases.expires_at < now() - make_interval(secs => $4) \
             RETURNING holder",
        )
        .bind(PROCESSING_LEASE)
        .bind(&self.me)
        .bind(self.timings.ttl.as_secs_f64())
        .bind(grace.as_secs_f64())
        .fetch_optional(&mut *conn)
        .await?;
        self.state().deadline = taken.map(|_| sent + self.timings.ttl - self.timings.margin);
        self.watch_peer(conn).await
    }

    async fn watch_peer(&self, conn: &mut PgConnection) -> sqlx::Result<()> {
        let peer = if self.preferred || !self.held() {
            None
        } else {
            self.live_peer(conn).await?
        };
        let mut state = self.state();
        let Some(peer) = peer else {
            state.peer = None;
            state.yielding = false;
            return Ok(());
        };
        let now = Instant::now();
        // A new process on the other host starts the count again: a crash loop
        // must not get the lease back.
        let since = match state.peer.take() {
            Some((seen, since)) if seen == peer => since,
            _ => {
                state.yielding = false;
                now
            }
        };
        if now.duration_since(since) >= self.timings.return_after {
            state.yielding = true;
        }
        state.peer = Some((peer, since));
        Ok(())
    }

    /// The heartbeat of the one other running host, or `None` when there is
    /// none, or more than one.
    async fn live_peer(&self, conn: &mut PgConnection) -> sqlx::Result<Option<String>> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT holder FROM leases \
             WHERE starts_with(name, $1) AND name <> $2 AND expires_at > now()",
        )
        .bind(HEARTBEAT_PREFIX)
        .bind(self.heartbeat_name())
        .fetch_all(conn)
        .await?;
        Ok(match <[String; 1]>::try_from(rows) {
            Ok([peer]) => Some(peer),
            Err(_) => None,
        })
    }

    /// Gives the lease to the preferred host. False, and nothing changes,
    /// unless this lease is handing back and knows the other host: the other
    /// host may have restarted since.
    pub async fn hand_over(&self, conn: &mut PgConnection) -> sqlx::Result<bool> {
        let peer = {
            let mut state = self.state();
            if !state.yielding {
                return Ok(false);
            }
            let peer = state.peer.take().map(|(peer, _)| peer);
            state.deadline = None;
            state.yielding = false;
            peer
        };
        let Some(peer) = peer else {
            return Ok(false);
        };
        let done = sqlx::query(
            "UPDATE leases SET holder = $1, expires_at = now() + make_interval(secs => $2), \
                 renewed_at = now() \
             WHERE name = $3 AND holder = $4",
        )
        .bind(peer)
        .bind(self.timings.ttl.as_secs_f64())
        .bind(PROCESSING_LEASE)
        .bind(&self.me)
        .execute(conn)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// A clean stop: the lease is given up, so the other host takes it at its
    /// next round (a day in the past is beyond any grace), and this host's
    /// heartbeat is removed.
    pub async fn release(&self, conn: &mut PgConnection) -> sqlx::Result<()> {
        {
            let mut state = self.state();
            state.deadline = None;
            state.yielding = false;
            state.peer = None;
        }
        sqlx::query(
            "UPDATE leases SET holder = '', expires_at = now() - interval '1 day', renewed_at = now() \
             WHERE name = $1 AND holder = $2",
        )
        .bind(PROCESSING_LEASE)
        .bind(&self.me)
        .execute(&mut *conn)
        .await?;
        sqlx::query("DELETE FROM leases WHERE name = $1 AND holder = $2")
            .bind(self.heartbeat_name())
            .bind(&self.me)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }

    fn heartbeat_name(&self) -> String {
        format!("{HEARTBEAT_PREFIX}{}", self.host)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // The state is a few plain values: a panic elsewhere cannot leave it half written.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl Fence for Lease {
    fn held(&self) -> bool {
        Lease::held(self)
    }

    fn holder(&self) -> &str {
        Lease::holder(self)
    }
}

/// A heartbeat, for `scannerpulse lease` and the admin page.
#[derive(Debug, Clone, PartialEq)]
pub struct Heartbeat {
    pub host: String,
    /// Seconds since it was last renewed.
    pub age: f64,
    /// Seconds until it expires; negative when it has.
    pub expires_in: f64,
}

/// The lease as the database holds it.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// The holder, empty after a clean release; `None` when ingest never ran.
    pub holder: Option<String>,
    /// Seconds until the lease expires; negative when it has.
    pub expires_in: f64,
    pub heartbeats: Vec<Heartbeat>,
}

impl Status {
    /// The lease has a holder that has not expired.
    pub fn valid(&self) -> bool {
        self.holder.as_deref().is_some_and(|h| !h.is_empty()) && self.expires_in > 0.0
    }

    /// The lines `scannerpulse lease` prints.
    pub fn lines(&self) -> Vec<String> {
        let holder = match self.holder.as_deref() {
            None => "holder: none (ingest has never run)".to_string(),
            Some("") => "holder: none (released)".to_string(),
            Some(holder) if self.valid() => {
                format!("holder: {holder} (expires in {:.1} s)", self.expires_in)
            }
            Some(holder) => format!(
                "holder: none (the lease of {holder} expired {:.1} s ago)",
                -self.expires_in
            ),
        };
        let mut lines = vec![holder];
        for beat in &self.heartbeats {
            let state = if beat.expires_in > 0.0 {
                "running"
            } else {
                "stopped"
            };
            lines.push(format!(
                "heartbeat {}: renewed {:.1} s ago ({state})",
                beat.host, beat.age
            ));
        }
        lines
    }
}

pub async fn status(db: impl PgExecutor<'_>) -> sqlx::Result<Status> {
    let rows: Vec<(String, String, f64, f64)> = sqlx::query_as(
        "SELECT name, holder, \
                EXTRACT(EPOCH FROM expires_at - now())::float8, \
                EXTRACT(EPOCH FROM now() - renewed_at)::float8 \
         FROM leases ORDER BY name",
    )
    .fetch_all(db)
    .await?;
    let mut status = Status {
        holder: None,
        expires_in: 0.0,
        heartbeats: Vec::new(),
    };
    for (name, holder, expires_in, age) in rows {
        if name == PROCESSING_LEASE {
            status.holder = Some(holder);
            status.expires_in = expires_in;
        } else if let Some(host) = name.strip_prefix(HEARTBEAT_PREFIX) {
            status.heartbeats.push(Heartbeat {
                host: host.to_string(),
                age,
                expires_in,
            });
        }
    }
    Ok(status)
}
