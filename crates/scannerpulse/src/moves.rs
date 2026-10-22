//! Moving a date range of calls into another storage location. Each call's
//! file is copied into the location, in the archive's layout, and read back;
//! one transaction then locks the call, finds it where it was read, points
//! it at the copy and lists the old file as a leftover, which the filer
//! deletes. A stop, a crash or a failover never leaves a call without a
//! checked file, and a later start of the same move takes the calls left.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::archive;
use crate::data::{free_rel, identities, io_error, same_identities, writes};
use crate::jobs::{self, Job, JobError, Kind, Outcome};
use crate::setup::folders::size;
use crate::storage;

/// The calls one step of a move takes.
pub const FILES_PER_STEP: i64 = 200;

/// The word typed to start a move.
pub const CONFIRM: &str = "move";

/// How long measuring the calls' files and the destination's disk may take:
/// a hung share must not hold the request.
const MEASURE_LIMIT: Duration = Duration::from_secs(10);

/// The space a location keeps free: 1 GB, or 2 % of its disk when that is
/// more.
pub fn margin(total: u64) -> u64 {
    (total / 50).max(1_000_000_000)
}

/// Why `needed` bytes do not fit in the `free` bytes of `name`'s disk of
/// `total` bytes, its margin kept free, or None.
pub fn room_problem(name: &str, needed: u64, free: u64, total: u64) -> Option<String> {
    let keep = margin(total);
    (needed.saturating_add(keep) > free).then(|| {
        format!(
            "{name} has {} free: these calls need about {}, and {} stays free",
            size(free),
            size(needed),
            size(keep)
        )
    })
}

/// Where the calls of a move are now.
#[derive(Debug, Clone, Serialize)]
pub struct Held {
    pub location: String,
    pub calls: i64,
}

/// What a move would do: the calls of the days `from_day` to `to_day` in
/// every location calls are moved out of, the space they use, about, and the
/// destination's free space; or why it cannot start.
#[derive(Debug, Clone, Serialize)]
pub struct MovePreview {
    pub to: String,
    pub to_id: i16,
    pub from_day: NaiveDate,
    pub to_day: NaiveDate,
    pub calls: i64,
    pub held: Vec<Held>,
    pub bytes_about: Option<i64>,
    pub free_bytes: Option<i64>,
    pub refused: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Move {
    to: i16,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
    from_day: NaiveDate,
    to_day: NaiveDate,
}

/// A move's state between steps.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MoveWork {
    /// The last call examined.
    after: Option<(DateTime<Utc>, i64)>,
    moved: i64,
    missing: i64,
    changed: i64,
    refused: i64,
    last_refusal: Option<String>,
}

/// The instants the days `from_day` to `to_day` of `zone` begin and end.
fn bounds(
    from_day: NaiveDate,
    to_day: NaiveDate,
    zone: Tz,
) -> Result<(DateTime<Utc>, DateTime<Utc>), JobError> {
    use chrono::Datelike;
    if [from_day, to_day]
        .iter()
        .any(|day| !(1970..=9999).contains(&day.year()))
    {
        return Err(JobError::Refused(
            "days must be between 1970 and 9999".into(),
        ));
    }
    if to_day < from_day {
        return Err(JobError::Refused(format!("{to_day} is before {from_day}")));
    }
    let start = |day| crate::backfill::start_of(day, zone).map_err(JobError::Refused);
    let after = to_day
        .succ_opt()
        .ok_or_else(|| JobError::Refused("the last day is too far".into()))?;
    Ok((start(from_day)?, start(after)?))
}

/// What moving the calls of the days `from_day` to `to_day` (in `zone`) into
/// the location `to` would do. It changes nothing.
pub async fn preview(
    conn: &mut PgConnection,
    to: &str,
    from_day: NaiveDate,
    to_day: NaiveDate,
    zone: Tz,
) -> Result<MovePreview, JobError> {
    let dest: Option<(i16, String, String)> =
        sqlx::query_as("SELECT id, root, mode FROM locations WHERE name = $1")
            .bind(to)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((to_id, root, mode)) = dest else {
        return Err(JobError::Refused(format!(
            "there is no location named {to}"
        )));
    };
    if mode != "managed" {
        return Err(JobError::Refused(format!(
            "{to} is read-only: nothing is moved into it"
        )));
    }
    let (from, until) = bounds(from_day, to_day, zone)?;
    let rows: Vec<(String, String, i64, Vec<String>)> = sqlx::query_as(&format!(
        "SELECT l.name, l.root, count(*), \
                coalesce((array_agg(c.rel_path) FILTER (WHERE c.id % 97 = 0))[1:{}], \
                         ARRAY[min(c.rel_path), max(c.rel_path)]) \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.started_at >= $1 AND c.started_at < $2 AND l.mode = 'managed' \
           AND c.location_id <> $3 \
         GROUP BY l.id, l.name, l.root ORDER BY l.id",
        storage::SAMPLED
    ))
    .bind(from)
    .bind(until)
    .bind(to_id)
    .fetch_all(&mut *conn)
    .await?;
    let mut preview = MovePreview {
        to: to.to_string(),
        to_id,
        from_day,
        to_day,
        calls: rows.iter().map(|row| row.2).sum(),
        held: rows
            .iter()
            .map(|(location, _, calls, _)| Held {
                location: location.clone(),
                calls: *calls,
            })
            .collect(),
        bytes_about: None,
        free_bytes: None,
        refused: None,
    };
    if preview.calls == 0 {
        preview.refused = Some(format!("no call of {from_day} to {to_day} is outside {to}"));
        return Ok(preview);
    }
    if let Some(why) = storage::unusable(conn, to_id, to).await? {
        preview.refused = Some(why);
        return Ok(preview);
    }
    let measured = tokio::time::timeout(
        MEASURE_LIMIT,
        tokio::task::spawn_blocking(move || {
            let bytes = rows
                .iter()
                .map(|(_, root, calls, sample)| {
                    storage::bytes_about(Path::new(root), sample, *calls)
                })
                .sum::<Option<i64>>();
            let disk = rustix::fs::statvfs(Path::new(&root)).map(|s| {
                (
                    s.f_bavail.saturating_mul(s.f_frsize),
                    s.f_blocks.saturating_mul(s.f_frsize),
                )
            });
            (bytes, disk)
        }),
    )
    .await;
    let (bytes, disk) = match measured {
        Ok(Ok(found)) => found,
        Ok(Err(error)) => return Err(JobError::Refused(format!("the measure failed: {error}"))),
        Err(_) => {
            preview.refused = Some(format!(
                "{to}, or the calls' files, did not answer within {} s",
                MEASURE_LIMIT.as_secs()
            ));
            return Ok(preview);
        }
    };
    preview.bytes_about = bytes;
    let Some(bytes) = bytes else {
        preview.refused = Some("the calls' files cannot be read here to measure them".into());
        return Ok(preview);
    };
    match disk {
        Ok((free, total)) => {
            preview.free_bytes = Some(i64::try_from(free).unwrap_or(i64::MAX));
            preview.refused = room_problem(to, u64::try_from(bytes).unwrap_or(0), free, total);
        }
        Err(error) => preview.refused = Some(format!("{to}: {error}")),
    }
    Ok(preview)
}

/// Adds a job that moves the calls of the days `from_day` to `to_day` into
/// the location `to`, once [`preview`] finds nothing against it and `move`
/// is typed. Returns the job's id and what was counted.
pub async fn start(
    conn: &mut PgConnection,
    to: &str,
    from_day: NaiveDate,
    to_day: NaiveDate,
    zone: Tz,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, MovePreview), JobError> {
    let preview = preview(conn, to, from_day, to_day, zone).await?;
    if let Some(why) = &preview.refused {
        return Err(JobError::Refused(why.clone()));
    }
    if confirm != CONFIRM {
        return Err(JobError::Refused(format!("type {CONFIRM} to confirm")));
    }
    let (from, until) = bounds(from_day, to_day, zone)?;
    let params = serde_json::to_value(Move {
        to: preview.to_id,
        from,
        until,
        from_day,
        to_day,
    })
    .expect("plain data");
    let job = jobs::create(
        &mut *conn,
        Kind::Move,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    Ok((job, preview))
}

/// A call one step moves.
#[derive(Debug, sqlx::FromRow)]
struct Moving {
    id: i64,
    started_at: DateTime<Utc>,
    location_id: i16,
    rel_path: String,
    root: String,
    /// Its location holds a marker: every location but the main archive.
    marked: bool,
}

/// What moving one call did.
enum Moved {
    Done,
    Missing,
    /// It is not where its step's batch read it: a correction, a purge or a
    /// longer copy came first. A later move takes it.
    Changed,
    Refused(String),
}

/// A folder is the location it was added as, mounted: Err otherwise, and the
/// step is tried again.
fn mounted(root: &Path, marked: bool, name: &str) -> sqlx::Result<()> {
    if root.is_dir() && (!marked || root.join(storage::MARKER).is_file()) {
        Ok(())
    } else {
        Err(io_error(io::Error::other(format!(
            "{name} is not mounted at {} on this machine",
            root.display()
        ))))
    }
}

pub(crate) async fn step(conn: &mut PgConnection, job: &Job) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Move>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a move's".into(),
        ));
    };
    let mut work: MoveWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    // Shared, so that the location is not removed while the step runs.
    let dest: Option<(String, String, String, bool)> = sqlx::query_as(
        "SELECT name, root, mode, is_primary FROM locations WHERE id = $1 FOR SHARE",
    )
    .bind(params.to)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((name, root, mode, primary)) = dest else {
        return Ok(Outcome::Failed(format!(
            "the location {} is gone: {} calls were moved",
            params.to, work.moved
        )));
    };
    if mode != "managed" {
        return Ok(Outcome::Failed(format!("{name} is read-only")));
    }
    let dest_root = PathBuf::from(&root);
    mounted(&dest_root, !primary, &name)?;
    let stats = rustix::fs::statvfs(&dest_root).map_err(|error| io_error(error.into()))?;
    let (free, total) = (
        stats.f_bavail.saturating_mul(stats.f_frsize),
        stats.f_blocks.saturating_mul(stats.f_frsize),
    );
    if free < margin(total) {
        return Ok(Outcome::Failed(format!(
            "{name} has {} free, less than the {} it keeps free: {} calls were moved; make room and start the move again",
            size(free),
            size(margin(total)),
            work.moved
        )));
    }
    // A host that lost the location would fail to play every call moved now.
    if let Some(why) = storage::unusable(conn, params.to, &name).await? {
        return Ok(Outcome::Failed(format!(
            "{why}: {} calls were moved; start the move again once every host can use {name}",
            work.moved
        )));
    }
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<Moving> = sqlx::query_as(
        "SELECT c.id, c.started_at, c.location_id, c.rel_path, l.root, NOT l.is_primary AS marked \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.started_at >= $1 AND c.started_at < $2 AND l.mode = 'managed' \
           AND c.location_id <> $3 \
           AND ($4::timestamptz IS NULL OR (c.started_at, c.id) > ($4, $5)) \
         ORDER BY c.started_at, c.id LIMIT $6",
    )
    .bind(params.from)
    .bind(params.until)
    .bind(params.to)
    .bind(after_at)
    .bind(after_id)
    .bind(FILES_PER_STEP)
    .fetch_all(&mut *conn)
    .await?;
    if batch.is_empty() {
        let mut message = format!(
            "{} calls of {} to {} moved to {name}; {} missing, {} changed while they were moved, {} refused",
            work.moved, params.from_day, params.to_day, work.missing, work.changed, work.refused
        );
        if let Some(why) = &work.last_refusal {
            message.push_str(&format!("; the last refusal: {why}"));
        }
        return Ok(Outcome::Done(job.done, message));
    }
    let mut roots: Vec<(PathBuf, bool)> = batch
        .iter()
        .map(|call| (PathBuf::from(&call.root), call.marked))
        .collect();
    roots.sort();
    roots.dedup();
    for (root, marked) in &roots {
        mounted(root, *marked, &root.display().to_string())?;
    }
    // A source reached by another path is the destination itself: its
    // "copy" would be the call's file, and the sweep would delete it.
    let sources: Vec<&Path> = roots.iter().map(|(root, _)| root.as_path()).collect();
    if let Some(i) = storage::same_as(&dest_root, &sources).map_err(io_error)? {
        return Ok(Outcome::Failed(format!(
            "{name} and {} are one folder reached by two paths: nothing is moved between them; {} calls were moved",
            sources[i].display(),
            work.moved
        )));
    }
    let mut folders: Vec<&Path> = roots.iter().map(|(root, _)| root.as_path()).collect();
    folders.push(&dest_root);
    let before = identities(&folders);
    for call in &batch {
        match move_one(conn, params.to, &dest_root, call).await? {
            Moved::Done => work.moved += 1,
            Moved::Missing => work.missing += 1,
            Moved::Changed => work.changed += 1,
            Moved::Refused(why) => {
                work.refused += 1;
                work.last_refusal = Some(why);
            }
        }
        work.after = Some((call.started_at, call.id));
    }
    same_identities(&folders, &before)?;
    let done = job.done + batch.len() as i64;
    let total = job.total.map(|total| total.max(done));
    Ok(Outcome::More {
        done,
        total,
        message: format!(
            "{} calls: {} moved to {name}, {} missing, {} changed, {} refused",
            jobs::progress(done, total),
            work.moved,
            work.missing,
            work.changed,
            work.refused
        ),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}

/// Copies the call's file into the destination at the same path (or the
/// first free name beside it), checks it, gives it the old file's
/// modification time, points the call at it and lists the old file as a
/// leftover. The call is locked until the step's end.
async fn move_one(
    conn: &mut PgConnection,
    dest_id: i16,
    dest_root: &Path,
    call: &Moving,
) -> sqlx::Result<Moved> {
    let now: Option<(i16, String)> = sqlx::query_as(
        "SELECT location_id, rel_path FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE",
    )
    .bind(call.id)
    .bind(call.started_at)
    .fetch_optional(&mut *conn)
    .await?;
    if now != Some((call.location_id, call.rel_path.clone())) {
        return Ok(Moved::Changed);
    }
    let refused = |error: io::Error| Moved::Refused(format!("{}: {error}", call.rel_path));
    let old = Path::new(&call.root).join(&call.rel_path);
    let data = match std::fs::read(&old) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Moved::Missing),
        Err(error) => return Ok(refused(error)),
    };
    let mtime = match std::fs::metadata(&old).and_then(|meta| meta.modified()) {
        Ok(mtime) => mtime,
        Err(error) => return Ok(refused(error)),
    };
    let Some((dir, name)) = call.rel_path.rsplit_once('/') else {
        return Ok(Moved::Refused(format!(
            "{} is not in a folder",
            call.rel_path
        )));
    };
    // The name is chosen and given under the lock: the filer takes names in
    // the main archive, which may be the destination.
    let writing = writes().await?;
    let (rel, copy) = match free_rel(conn, dest_root, dest_id, dir, name, &data).await? {
        Ok(found) => found,
        Err(error) => return Ok(refused(error)),
    };
    let path = dest_root.join(&rel);
    if !copy {
        // After this the copy is an orphan if the step fails: the next
        // attempt finds it as an exact copy and takes it.
        if let Err(error) = archive::write_part(&path, &data, &archive::sha256(&data))
            .and_then(|()| archive::finish_part(&path))
        {
            return Ok(refused(error));
        }
    }
    if let Err(error) = std::fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(mtime))
    {
        return Ok(refused(error));
    }
    drop(writing);
    sqlx::query(
        "UPDATE calls SET location_id = $3, rel_path = $4 WHERE id = $1 AND started_at = $2",
    )
    .bind(call.id)
    .bind(call.started_at)
    .bind(dest_id)
    .bind(&rel)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO archive_leftovers (location_id, rel_path) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(call.location_id)
    .bind(&call.rel_path)
    .execute(&mut *conn)
    .await?;
    Ok(Moved::Done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_keeps_a_gigabyte_or_two_percent_free() {
        assert_eq!(margin(10_000_000_000), 1_000_000_000);
        assert_eq!(margin(100_000_000_000), 2_000_000_000);
        let tb = 1_000_000_000_000;
        assert_eq!(room_problem("Cold", 10_000_000_000, tb / 2, tb), None);
        assert_eq!(
            room_problem("Cold", 499_000_000_000, tb / 2, tb).as_deref(),
            Some("Cold has 500.0 GB free: these calls need about 499.0 GB, and 20.0 GB stays free")
        );
        // The margin counts: 1 GB of calls does not fit in 2 GB free of a
        // 100 GB disk.
        assert!(room_problem("Cold", 1_000_000_000, 2_000_000_000, 100_000_000_000).is_some());
    }
}
