//! The admin page's Status tab, and `scannerpulse status`: the processing
//! lease, ingest's files, the transcription queue, the servers, the database
//! and the checks to make after a failover. Read only.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::PgPool;

/// The rows each list shows.
pub const LISTED: i64 = 20;
/// A running job its host has not stepped for this long is stuck.
pub const STUCK_MINUTES: i32 = 10;
/// A file still waiting this long after it was first seen is left in its
/// source.
pub const LEFT_MINUTES: i32 = 60;
/// The days the check for calls filed twice looks back.
pub const TWICE_DAYS: i32 = 7;
/// The longest each of the Status tab's queries may take.
pub const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct Rejected {
    source: String,
    rel_path: String,
    reason: Option<String>,
    seen_at: DateTime<Utc>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct Recent {
    id: i64,
    started_at: DateTime<Utc>,
    system: Option<String>,
    talkgroup: Option<i64>,
    talkgroup_name: Option<String>,
    transcript_state: String,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct Standby {
    name: Option<String>,
    address: Option<String>,
    state: Option<String>,
    sync_state: Option<String>,
    replay_lag_seconds: Option<f64>,
}

/// Everything the Status tab shows, within `TIME_LIMIT`.
pub async fn read(pool: &PgPool) -> sqlx::Result<Value> {
    read_within(pool, TIME_LIMIT).await
}

/// Everything the Status tab shows, read in one snapshot in which each query
/// may take at most `limit`: a slow or locked table then fails the reading
/// rather than hang it.
pub async fn read_within(pool: &PgPool, limit: std::time::Duration) -> sqlx::Result<Value> {
    use sqlx::Connection;
    let mut conn = pool.acquire().await?;
    let mut tx = conn.begin().await?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{}ms", limit.as_millis()))
        .execute(&mut *tx)
        .await?;
    let lease = crate::lease::status(&mut *tx).await?;
    let (waiting, rejected, left): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'waiting'), \
                count(*) FILTER (WHERE state = 'rejected'), \
                count(*) FILTER (WHERE state = 'waiting' \
                                 AND first_seen < now() - make_interval(mins => $1)) \
         FROM ingest_files f \
         WHERE NOT EXISTS (SELECT 1 FROM imports i WHERE i.source_id = f.source_id)",
    )
    .bind(LEFT_MINUTES)
    .fetch_one(&mut *tx)
    .await?;
    let recent_rejected: Vec<Rejected> = sqlx::query_as(
        "SELECT s.name AS source, f.rel_path, f.reason, f.seen_at \
         FROM ingest_files f JOIN sources s ON s.id = f.source_id \
         WHERE f.state = 'rejected' \
           AND NOT EXISTS (SELECT 1 FROM imports i WHERE i.source_id = f.source_id) \
         ORDER BY f.seen_at DESC LIMIT $1",
    )
    .bind(LISTED)
    .fetch_all(&mut *tx)
    .await?;
    let queue = crate::queue::counts(&mut *tx).await?;
    let failed = crate::queue::failed(&mut *tx, LISTED).await?;
    let servers = crate::queue::servers(&mut *tx).await?;
    // Counting every call takes seconds on a large archive: the planner's
    // estimate is enough here.
    let (calls_about, transcripts_about): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT (SELECT sum(c.reltuples)::bigint FROM pg_inherits i \
                 JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'calls'::regclass), \
                (SELECT sum(c.reltuples)::bigint FROM pg_inherits i \
                 JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'transcripts'::regclass)",
    )
    .fetch_one(&mut *tx)
    .await?;
    let recent: Vec<Recent> = sqlx::query_as(
        "SELECT c.id, c.started_at, s.name AS system, t.number AS talkgroup, \
                t.name AS talkgroup_name, c.transcript_state \
         FROM calls c \
         LEFT JOIN talkgroups t ON t.id = c.talkgroup_id \
         LEFT JOIN systems s ON s.id = t.system_id \
         ORDER BY c.started_at DESC, c.id DESC LIMIT $1",
    )
    .bind(LISTED)
    .fetch_all(&mut *tx)
    .await?;
    let (primary, server_version, address, port): (bool, String, Option<String>, Option<i32>) =
        sqlx::query_as(
            "SELECT NOT pg_is_in_recovery(), current_setting('server_version'), \
                    inet_server_addr()::text, inet_server_port()",
        )
        .fetch_one(&mut *tx)
        .await?;
    // The same reading as ingest's maintenance loop: missing or not a bool is off.
    let maintenance = sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM settings WHERE key = 'maintenance.enabled'",
    )
    .fetch_optional(&mut *tx)
    .await?
    .and_then(|v| v.as_bool())
    .unwrap_or(false);
    let standbys: Vec<Standby> = sqlx::query_as(
        "SELECT application_name AS name, client_addr::text AS address, state, sync_state, \
                extract(epoch FROM replay_lag)::float8 AS replay_lag_seconds \
         FROM pg_stat_replication ORDER BY application_name",
    )
    .fetch_all(&mut *tx)
    .await?;
    let (stuck, running): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE updated_at < now() - make_interval(mins => $1) \
                                 OR (updated_at IS NULL AND started_at IS NOT NULL)), \
                count(*) \
         FROM jobs WHERE state = 'running'",
    )
    .bind(STUCK_MINUTES)
    .fetch_one(&mut *tx)
    .await?;
    // A running host that cannot use a storage location: its plays of those
    // calls fail, and nothing is moved into it.
    let locations: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT l.name, h.host, h.problem FROM location_health h \
         JOIN locations l ON l.id = h.location_id \
         WHERE NOT h.ok AND EXISTS (SELECT 1 FROM leases s \
                                    WHERE s.name = $1 || h.host AND s.expires_at > now()) \
         ORDER BY l.name, h.host",
    )
    .bind(crate::lease::HEARTBEAT_PREFIX)
    .fetch_all(&mut *tx)
    .await?;
    let twice: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (SELECT 1 FROM calls \
           WHERE started_at > now() - make_interval(days => $1) \
           GROUP BY location_id, rel_path HAVING count(*) > 1) d",
    )
    .bind(TWICE_DAYS)
    .fetch_one(&mut *tx)
    .await?;
    Ok(json!({
        "maintenance": maintenance,
        "lease": {
            "valid": lease.valid(),
            "holder": lease.holder,
            "expires_in": lease.expires_in,
            "heartbeats": lease.heartbeats,
        },
        "ingest": {
            "waiting": waiting,
            "rejected": rejected,
            "recent_rejected": recent_rejected,
        },
        "queue": queue,
        "locations": locations.iter().map(|(location, host, problem)| json!({
            "location": location,
            "host": host,
            "problem": problem,
        })).collect::<Vec<_>>(),
        "failed": failed,
        "servers": servers.iter().map(|s| json!({
            "url": s.url,
            "cooling_until": s.cooling_until,
            "failed_rounds": s.failed_rounds,
            "last_error": s.last_error,
            "updated_at": s.updated_at,
        })).collect::<Vec<_>>(),
        "totals": {"calls_about": calls_about, "transcripts_about": transcripts_about},
        "recent_calls": recent,
        "database": {
            "primary": primary,
            "version": server_version,
            "address": address,
            "port": port,
            "standbys": standbys,
        },
        "failover": {
            "lease_held": lease.valid(),
            "jobs_running": running,
            "jobs_stuck": stuck,
            "calls_failed": queue.failed,
            "calls_filed_twice": twice,
            "files_left_in_sources": left,
        },
    }))
}
