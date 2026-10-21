//! The database side of ingest: the sources and the primary location, the
//! rows of `ingest_files`, the lookups of systems, sites and talkgroups, and
//! the recording of a call.

use std::fmt;
use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::postgres::PgConnection;
use sqlx::{Connection, PgExecutor, Postgres, Transaction};

use crate::reader::{Described, FileCall, Talkgroup};

/// The lease row that names the host that files.
pub const PROCESSING_LEASE: &str = "processing";

/// How long the transaction that records a call may sit idle. A holder frozen
/// inside it would otherwise keep its lock on the lease row and block every
/// takeover.
pub const FENCE_IDLE_TIMEOUT: &str = "5s";

/// How the reason of a file rejected as gone from its source begins. Such a
/// file waits again when it comes back.
pub const MISSING: &str = "missing";

/// Runs one database call under `limit`. A call that takes longer is a
/// database error, so the caller's usual pause and retry apply.
pub async fn limited<T>(
    limit: Duration,
    call: impl Future<Output = sqlx::Result<T>>,
) -> sqlx::Result<T> {
    tokio::time::timeout(limit, call)
        .await
        .unwrap_or_else(|_| Err(timed_out(limit)))
}

/// The error of a database call that did not end within `limit`.
pub fn timed_out(limit: Duration) -> sqlx::Error {
    sqlx::Error::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("the database did not answer within {limit:?}"),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: i16,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub id: i16,
    pub root: PathBuf,
}

pub async fn primary_location(db: impl PgExecutor<'_>) -> sqlx::Result<Option<Location>> {
    let row: Option<(i16, String)> =
        sqlx::query_as("SELECT id, root FROM locations WHERE is_primary")
            .fetch_optional(db)
            .await?;
    Ok(row.map(|(id, root)| Location {
        id,
        root: root.into(),
    }))
}

/// A recorder as the watcher lists it: watched, or taken in when asked, with
/// the time of a request to bring its files in that no watcher has listed it
/// for yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub source: Source,
    pub asked: bool,
    pub bring_in: Option<DateTime<Utc>>,
}

/// The recorders the watcher lists: the watched ones and those taken in when
/// asked, not the paused or removed ones.
pub async fn listed_recorders(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Listed>> {
    type Row = (i16, String, String, bool, Option<DateTime<Utc>>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, name, path, intake = 'asked', \
                CASE WHEN brought_in_at IS DISTINCT FROM bring_in_at THEN bring_in_at END \
         FROM sources WHERE removed_at IS NULL AND (enabled OR intake = 'asked') ORDER BY id",
    )
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, path, asked, bring_in)| Listed {
            source: Source {
                id,
                name,
                path: path.into(),
            },
            asked,
            bring_in,
        })
        .collect())
}

/// Notes that a watcher listed a recorder for the request of `at`; a newer
/// request stays to be listed.
pub async fn brought_in(
    db: impl PgExecutor<'_>,
    source_id: i16,
    at: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query("UPDATE sources SET brought_in_at = $2 WHERE id = $1 AND bring_in_at = $2")
        .bind(source_id)
        .bind(at)
        .execute(db)
        .await?;
    Ok(())
}

/// What `host`'s watcher found in each recorder's folder: the number of audio
/// files, or why it could not list it.
pub async fn folder_health(
    db: impl PgExecutor<'_>,
    host: &str,
    found: &[(i16, Result<usize, String>)],
) -> sqlx::Result<()> {
    let ids: Vec<i16> = found.iter().map(|(id, _)| *id).collect();
    let files: Vec<Option<i32>> = found
        .iter()
        .map(|(_, f)| {
            f.as_ref()
                .ok()
                .map(|n| i32::try_from(*n).unwrap_or(i32::MAX))
        })
        .collect();
    let problems: Vec<Option<String>> = found
        .iter()
        .map(|(_, f)| f.as_ref().err().cloned())
        .collect();
    sqlx::query(
        "INSERT INTO source_health (host, source_id, ok, problem, files, checked_at) \
         SELECT $1, u.id, u.problem IS NULL, u.problem, u.files, now() \
         FROM unnest($2::smallint[], $3::text[], $4::int[]) AS u (id, problem, files) \
         WHERE EXISTS (SELECT 1 FROM sources s WHERE s.id = u.id AND s.removed_at IS NULL) \
         ON CONFLICT (host, source_id) DO UPDATE SET ok = EXCLUDED.ok, problem = EXCLUDED.problem, \
             files = EXCLUDED.files, checked_at = EXCLUDED.checked_at",
    )
    .bind(host)
    .bind(ids)
    .bind(problems)
    .bind(files)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn enabled_sources(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Source>> {
    let rows: Vec<(i16, String, String)> =
        sqlx::query_as("SELECT id, name, path FROM sources WHERE enabled ORDER BY id")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, path)| Source {
            id,
            name,
            path: path.into(),
        })
        .collect())
}

/// A file as the watcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// The path inside the source.
    pub rel_path: String,
    pub size: i64,
    pub file_mtime: DateTime<Utc>,
}

/// Records what the watcher sees in a source; each path at most once, and
/// nothing for a removed recorder (another host may not know yet). A new
/// file becomes a waiting row. A known file refreshes `seen_at`. A rejected
/// file stays rejected unless its size or modification time changed, or it was
/// rejected as missing: a missing file that comes back waits again. A filed
/// file that is still in the source waits again, so that the filer finds its
/// archive copy and deletes it.
pub async fn upsert_seen(
    db: impl PgExecutor<'_>,
    source_id: i16,
    files: &[Seen],
) -> sqlx::Result<u64> {
    let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
    let sizes: Vec<i64> = files.iter().map(|f| f.size).collect();
    let mtimes: Vec<DateTime<Utc>> = files.iter().map(|f| f.file_mtime).collect();
    let done = sqlx::query(
        "INSERT INTO ingest_files AS f (source_id, rel_path, size, file_mtime, first_seen, seen_at, state) \
         SELECT $1, u.rel_path, u.size, u.file_mtime, now(), now(), 'waiting' \
         FROM unnest($2::text[], $3::bigint[], $4::timestamptz[]) AS u (rel_path, size, file_mtime) \
         WHERE EXISTS (SELECT 1 FROM sources s WHERE s.id = $1 AND s.removed_at IS NULL) \
         ON CONFLICT (source_id, rel_path) DO UPDATE SET \
             seen_at = now(), size = EXCLUDED.size, file_mtime = EXCLUDED.file_mtime, \
             state = CASE WHEN f.state = 'staged' THEN 'staged' \
                          WHEN f.state = 'rejected' AND coalesce(f.reason, '') NOT LIKE $5 || '%' \
                           AND f.size IS NOT DISTINCT FROM EXCLUDED.size \
                           AND f.file_mtime IS NOT DISTINCT FROM EXCLUDED.file_mtime \
                          THEN 'rejected' ELSE 'waiting' END, \
             reason = CASE WHEN f.state = 'staged' THEN f.reason \
                           WHEN f.state = 'rejected' AND coalesce(f.reason, '') NOT LIKE $5 || '%' \
                            AND f.size IS NOT DISTINCT FROM EXCLUDED.size \
                            AND f.file_mtime IS NOT DISTINCT FROM EXCLUDED.file_mtime \
                           THEN f.reason END",
    )
    .bind(source_id)
    .bind(paths)
    .bind(sizes)
    .bind(mtimes)
    .bind(MISSING)
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// A file waiting to be filed.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Waiting {
    pub id: i64,
    pub source_id: i16,
    /// Its recorder's folder.
    pub source_path: String,
    pub rel_path: String,
    pub size: Option<i64>,
    /// Its modification time when the watcher last saw it.
    pub file_mtime: Option<DateTime<Utc>>,
    pub seen_at: DateTime<Utc>,
    /// The call it was filed as, when it was filed before and seen again.
    pub call_id: Option<i64>,
    pub call_started_at: Option<DateTime<Utc>>,
    /// True when it is the very file filed as that call: the modification
    /// time stored on the row equals the call's.
    pub same_file: bool,
    /// Its file waits in the archive's staging folder, its system now known.
    pub staged: bool,
}

/// Waiting files, oldest first, leaving out the ids in `skip` and the files
/// of removed recorders; with them the staged files whose system name the
/// administrator has since given (a system's or a mapped spelling).
pub async fn waiting(
    db: impl PgExecutor<'_>,
    limit: i64,
    skip: &[i64],
) -> sqlx::Result<Vec<Waiting>> {
    sqlx::query_as(
        "SELECT f.id, f.source_id, s.path AS source_path, f.rel_path, f.size, f.file_mtime, f.seen_at, \
                f.call_id, f.call_started_at, c.id IS NOT NULL AS same_file, \
                f.state = 'staged' AS staged \
         FROM ingest_files f \
         JOIN sources s ON s.id = f.source_id \
         LEFT JOIN calls c ON c.id = f.call_id AND c.started_at = f.call_started_at \
             AND c.file_mtime IS NOT DISTINCT FROM f.file_mtime \
         WHERE (f.state = 'waiting' OR (f.state = 'staged' \
                AND (EXISTS (SELECT 1 FROM systems y WHERE y.name = f.staged_system) \
                     OR EXISTS (SELECT 1 FROM system_spellings y WHERE y.name = f.staged_system)))) \
           AND s.removed_at IS NULL AND f.id <> ALL($2) \
         ORDER BY f.first_seen, f.id LIMIT $1",
    )
    .bind(limit)
    .bind(skip)
    .fetch_all(db)
    .await
}

/// Marks a waiting file rejected, with the reason.
pub async fn reject(db: impl PgExecutor<'_>, id: i64, reason: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE ingest_files SET state = 'rejected', reason = $2 \
         WHERE id = $1 AND state IN ('waiting', 'staged')",
    )
    .bind(id)
    .bind(reason)
    .execute(db)
    .await?;
    Ok(())
}

/// Stages a waiting file of an unknown system: its copy waits in the
/// archive's staging folder, named by its system and talkgroup for the list.
pub async fn stage(
    db: impl PgExecutor<'_>,
    id: i64,
    system: &str,
    talkgroup: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE ingest_files SET state = 'staged', staged_system = $2, staged_talkgroup = $3, \
         reason = NULL WHERE id = $1 AND state = 'waiting'",
    )
    .bind(id)
    .bind(system)
    .bind(talkgroup)
    .execute(db)
    .await?;
    Ok(())
}

/// Marks a file done as a call recorded earlier.
pub async fn mark_done(
    db: impl PgExecutor<'_>,
    id: i64,
    call: (i64, DateTime<Utc>),
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE ingest_files SET state = 'done', reason = NULL, call_id = $2, call_started_at = $3 \
         WHERE id = $1",
    )
    .bind(id)
    .bind(call.0)
    .bind(call.1)
    .execute(db)
    .await?;
    Ok(())
}

/// Deletes the rows of files filed more than `days` days ago.
pub async fn prune_done(db: impl PgExecutor<'_>, days: u32) -> sqlx::Result<u64> {
    let done = sqlx::query(
        "DELETE FROM ingest_files WHERE state = 'done' AND seen_at < now() - make_interval(days => $1)",
    )
    .bind(i32::try_from(days).unwrap_or(i32::MAX))
    .execute(db)
    .await?;
    Ok(done.rows_affected())
}

/// The numbers of waiting and of rejected files.
pub async fn counts(db: impl PgExecutor<'_>) -> sqlx::Result<(i64, i64)> {
    sqlx::query_as(
        "SELECT count(*) FILTER (WHERE state = 'waiting'), count(*) FILTER (WHERE state = 'rejected') \
         FROM ingest_files",
    )
    .fetch_one(db)
    .await
}

/// The call recorded for an archive file, if any. `near` is the call's start
/// as its file gives it; the search stays within a day of it, so that it
/// reads one or two month partitions.
pub async fn call_at(
    db: impl PgExecutor<'_>,
    location_id: i16,
    rel_path: &str,
    near: DateTime<Utc>,
) -> sqlx::Result<Option<(i64, DateTime<Utc>)>> {
    sqlx::query_as(
        "SELECT id, started_at FROM calls WHERE location_id = $1 AND rel_path = $2 \
         AND started_at > $3 - interval '1 day' AND started_at < $3 + interval '1 day'",
    )
    .bind(location_id)
    .bind(rel_path)
    .bind(near)
    .fetch_optional(db)
    .await
}

/// Creates the month partitions for `at` when they are missing. Call it
/// outside any transaction: it locks the partitioned tables until its own
/// transaction ends, and gives up after 10 s of waiting for those locks.
pub async fn ensure_partition(db: impl PgExecutor<'_>, at: DateTime<Utc>) -> sqlx::Result<()> {
    sqlx::query("SELECT ensure_month_partitions($1)")
        .bind(at.date_naive())
        .execute(db)
        .await?;
    Ok(())
}

/// What a talkgroup name a recorder sent does to the stored name. Names
/// change only with the administrator's approval: a different name is a
/// proposal. SDRTrunk's placeholder is no name: it never replaces one and is
/// never proposed, and a real name replaces it at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSeen {
    Same,
    Rename,
    Propose,
}

pub fn name_seen(stored: &str, seen: &str) -> NameSeen {
    if stored == seen || is_placeholder(seen) {
        NameSeen::Same
    } else if is_placeholder(stored) {
        NameSeen::Rename
    } else {
        NameSeen::Propose
    }
}

/// The recorder's name for a talkgroup that is missing from its alias list.
pub fn is_placeholder(name: &str) -> bool {
    name.starts_with("Unidentified")
}

/// The id of a system, added when it is new. The SELECT comes first because an
/// INSERT ... ON CONFLICT uses up an identity value on every call.
pub async fn system_id(conn: &mut PgConnection, name: &str) -> sqlx::Result<i32> {
    Ok(resolve_system(conn, name).await?.0)
}

/// The id of the system a file names, and whether the name is a spelling
/// mapped to it rather than its own name; a system is added when the name is
/// new. Filing stages a file of an unknown system first ([`system_known`]).
pub async fn resolve_system(conn: &mut PgConnection, name: &str) -> sqlx::Result<(i32, bool)> {
    match find_system(&mut *conn, name).await? {
        Some(found) => Ok(found),
        None => sqlx::query_scalar("INSERT INTO systems (name) VALUES ($1) RETURNING id")
            .bind(name)
            .fetch_one(&mut *conn)
            .await
            .map(|id| (id, false)),
    }
}

/// The id of the system named `name`, and whether the name is a spelling
/// mapped to it; `None` when no system has it.
pub async fn find_system(db: impl PgExecutor<'_>, name: &str) -> sqlx::Result<Option<(i32, bool)>> {
    sqlx::query_as(
        "SELECT id, false FROM systems WHERE name = $1 \
         UNION ALL SELECT system_id, true FROM system_spellings WHERE name = $1 LIMIT 1",
    )
    .bind(name)
    .fetch_optional(db)
    .await
}

/// Whether a file naming the system `name` may be filed: the name is a
/// system's or a spelling mapped to one, or no system exists yet (the first
/// system of an install is added at once).
pub async fn system_known(db: impl PgExecutor<'_>, name: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM systems WHERE name = $1) \
             OR EXISTS (SELECT 1 FROM system_spellings WHERE name = $1) \
             OR NOT EXISTS (SELECT 1 FROM systems)",
    )
    .bind(name)
    .fetch_one(db)
    .await
}

/// The id of a site of a system, added when it is new.
pub async fn site_id(conn: &mut PgConnection, system_id: i32, name: &str) -> sqlx::Result<i32> {
    let found: Option<i32> =
        sqlx::query_scalar("SELECT id FROM sites WHERE system_id = $1 AND name = $2")
            .bind(system_id)
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?;
    match found {
        Some(id) => Ok(id),
        None => {
            sqlx::query_scalar("INSERT INTO sites (system_id, name) VALUES ($1, $2) RETURNING id")
                .bind(system_id)
                .bind(name)
                .fetch_one(&mut *conn)
                .await
        }
    }
}

/// The id of a talkgroup of a system. A new talkgroup is added with
/// transcription off. A real name replaces a placeholder at once, and the old
/// name goes to `files_name` when that is empty. Any other different name is
/// a proposal ([`name_seen`]), counted for the recorder `source_id`: the
/// talkgroup keeps its name, and since the file is filed under the name it
/// carries, its files are marked as not corrected.
pub async fn talkgroup_id(
    conn: &mut PgConnection,
    system_id: i32,
    talkgroup: &Talkgroup,
    source_id: i16,
) -> sqlx::Result<i32> {
    let found: Option<(i32, String)> =
        sqlx::query_as("SELECT id, name FROM talkgroups WHERE system_id = $1 AND number = $2")
            .bind(system_id)
            .bind(talkgroup.number)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((id, stored)) = found else {
        return sqlx::query_scalar(
            "INSERT INTO talkgroups (system_id, number, name) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(system_id)
        .bind(talkgroup.number)
        .bind(&talkgroup.name)
        .fetch_one(&mut *conn)
        .await;
    };
    match name_seen(&stored, &talkgroup.name) {
        NameSeen::Same => {}
        NameSeen::Propose => {
            sqlx::query(
                "INSERT INTO name_proposals (talkgroup_id, name, source_ids, calls) \
                 VALUES ($1, $2, ARRAY[$3::smallint], 1) \
                 ON CONFLICT (talkgroup_id, name) DO UPDATE SET \
                     calls = name_proposals.calls + 1, last_seen = now(), \
                     source_ids = CASE WHEN $3 = ANY (name_proposals.source_ids) \
                                       THEN name_proposals.source_ids \
                                       ELSE name_proposals.source_ids || $3::smallint END",
            )
            .bind(id)
            .bind(&talkgroup.name)
            .bind(source_id)
            .execute(&mut *conn)
            .await?;
            sqlx::query(
                "UPDATE talkgroups SET files_correct = false, \
                 files_name = CASE WHEN coalesce(files_name, '') = '' THEN $2 ELSE files_name END \
                 WHERE id = $1 AND files_correct",
            )
            .bind(id)
            .bind(&talkgroup.name)
            .execute(&mut *conn)
            .await?;
        }
        NameSeen::Rename => {
            sqlx::query(
                "UPDATE talkgroups SET name = $2, \
                 files_name = CASE WHEN files_correct AND EXISTS (SELECT 1 FROM calls WHERE talkgroup_id = $1) THEN $3 \
                                   WHEN coalesce(files_name, '') = '' THEN $3 ELSE files_name END, \
                 files_correct = files_correct AND NOT EXISTS (SELECT 1 FROM calls WHERE talkgroup_id = $1) \
                 WHERE id = $1",
            )
            .bind(id)
            .bind(&talkgroup.name)
            .bind(stored)
            .execute(&mut *conn)
            .await?;
        }
    }
    Ok(id)
}

/// A call to record: the file's call, where it came from and where it is now.
#[derive(Debug, Clone, Copy)]
pub struct NewCall<'a> {
    pub call: &'a FileCall,
    pub source_id: i16,
    pub location_id: i16,
    /// The path in the archive, relative to the location's root.
    pub rel_path: &'a str,
    /// Queue the call for transcription when it is at least this long and its
    /// talkgroup has transcription on; `None` while transcription is off.
    pub transcribe_from_ms: Option<i32>,
}

#[derive(Debug)]
pub enum RecordError {
    /// `holder` does not hold the processing lease: nothing was written.
    LeaseLost,
    Database(sqlx::Error),
}

impl From<sqlx::Error> for RecordError {
    fn from(error: sqlx::Error) -> Self {
        RecordError::Database(error)
    }
}

impl fmt::Display for RecordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecordError::LeaseLost => write!(f, "this host does not hold the processing lease"),
            RecordError::Database(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RecordError {}

/// A transaction in which `holder` holds the processing lease. The lease row
/// is read FOR SHARE, so a takeover waits for the transaction: the check and
/// the writes cannot be split. The transaction may sit idle 5 s at most.
pub async fn fenced<'c>(
    conn: &'c mut PgConnection,
    holder: &str,
) -> Result<Transaction<'c, Postgres>, RecordError> {
    let mut tx = conn.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL idle_in_transaction_session_timeout = '{FENCE_IDLE_TIMEOUT}'"
    ))
    .execute(&mut *tx)
    .await?;
    let held: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM leases WHERE name = $1 AND holder = $2 AND expires_at > now() FOR SHARE",
    )
    .bind(PROCESSING_LEASE)
    .bind(holder)
    .fetch_optional(&mut *tx)
    .await?;
    if held.is_none() {
        return Err(RecordError::LeaseLost);
    }
    Ok(tx)
}

/// Step 3 of filing, in one fenced transaction: looks up the system, site and
/// talkgroup, records the call, marks the ingest row `row_id` done, queues the
/// call for transcription when `new` says so, and announces the call on
/// `scannerpulse_calls`. Returns the call's id and start.
pub async fn record(
    conn: &mut PgConnection,
    holder: &str,
    row_id: i64,
    new: &NewCall<'_>,
) -> Result<(i64, DateTime<Utc>), RecordError> {
    let mut tx = fenced(conn, holder).await?;
    let (talkgroup_id, site_id, mapped) =
        lookups(&mut tx, &new.call.described, new.source_id).await?;
    let call = new.call;
    let d = &call.described;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, site_id, radio_id, radio_alias, \
                            title, source_id, location_id, rel_path, file_mtime) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) RETURNING id",
    )
    .bind(call.started_at)
    .bind(call.duration_ms)
    .bind(talkgroup_id)
    .bind(site_id)
    .bind(d.radio_id)
    .bind(&d.radio_alias)
    .bind(&d.title)
    .bind(new.source_id)
    .bind(new.location_id)
    .bind(new.rel_path)
    .bind(call.file_mtime)
    .fetch_one(&mut *tx)
    .await?;
    mark_done(&mut *tx, row_id, (id, call.started_at)).await?;
    if mapped {
        // The file names a spelling of its system: the correction rewrites it.
        sqlx::query("INSERT INTO mapped_calls (call_id, started_at) VALUES ($1, $2)")
            .bind(id)
            .bind(call.started_at)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE talkgroups SET files_correct = false WHERE id = $1 AND files_correct")
            .bind(talkgroup_id)
            .execute(&mut *tx)
            .await?;
    }
    if let (Some(from_ms), Some(talkgroup)) = (new.transcribe_from_ms, talkgroup_id)
        && call.duration_ms >= from_ms
    {
        sqlx::query(
            "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
             SELECT $1, $2, now(), now(), 'queued' FROM talkgroups WHERE id = $3 AND transcribe",
        )
        .bind(id)
        .bind(call.started_at)
        .bind(talkgroup)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("SELECT pg_notify('scannerpulse_calls', $1)")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((id, call.started_at))
}

/// The talkgroup and site of a call, and whether its system name is a mapped
/// spelling. Both belong to a system, so a call without one has neither.
async fn lookups(
    conn: &mut PgConnection,
    d: &Described,
    source_id: i16,
) -> sqlx::Result<(Option<i32>, Option<i32>, bool)> {
    let Some(system) = d.system.as_deref() else {
        return Ok((None, None, false));
    };
    let (system, mapped) = resolve_system(conn, system).await?;
    let site = match d.site.as_deref() {
        Some(name) => Some(site_id(conn, system, name).await?),
        None => None,
    };
    let talkgroup = match &d.talkgroup {
        Some(talkgroup) => Some(talkgroup_id(conn, system, talkgroup, source_id).await?),
        None => None,
    };
    Ok((talkgroup, site, mapped))
}
