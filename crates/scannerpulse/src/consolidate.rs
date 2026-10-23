//! Consolidating an import, only when the administrator asks: each of its
//! calls is copied into the main archive, in the archive's layout (the day it
//! started, in the site's zone; the folder of the talkgroup its file names;
//! its file's name), read back, and pointed at the copy, as a move does. The
//! import's files stay as they are: nothing is written in its folder, and
//! none is listed for the sweep. Once it holds no call, the import can be
//! removed; its register stays, so that an import of the folder again takes
//! none of the files it took.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::archive;
use crate::data::{bounded, free_rel, identities, io_error, unchanged, writes};
use crate::jobs::{self, Job, JobError, Kind, Outcome};
use crate::moves::{margin, room_problem};
use crate::reader;
use crate::setup::folders::size;

/// The word typed to start a consolidation: a move's.
pub const CONFIRM: &str = crate::moves::CONFIRM;

/// The calls one step copies.
pub const FILES_PER_STEP: i64 = 200;

/// How long measuring the calls' files and the archive's disk may take.
const MEASURE_LIMIT: Duration = Duration::from_secs(10);

/// What consolidating an import would do: its calls, the space their files
/// use, about, and the main archive's free space; or why it cannot start.
#[derive(Debug, Clone, Serialize)]
pub struct ConsolidatePreview {
    pub name: String,
    pub calls: i64,
    pub bytes_about: Option<i64>,
    pub free_bytes: Option<i64>,
    pub refused: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Consolidate {
    name: String,
    location: i16,
}

/// The import named `name`: its location and folder.
async fn import_of(conn: &mut PgConnection, name: &str) -> Result<(i16, String), JobError> {
    sqlx::query_as(
        "SELECT l.id, l.root FROM imports i JOIN sources s ON s.id = i.source_id \
         JOIN locations l ON l.id = i.location_id WHERE s.name = $1",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| JobError::Refused(format!("there is no import named {name}")))
}

/// What consolidating the import `name` would do. It changes nothing.
pub async fn preview(conn: &mut PgConnection, name: &str) -> Result<ConsolidatePreview, JobError> {
    let (location, root) = import_of(conn, name).await?;
    // No call gives a sample of two nulls.
    let (calls, sample): (i64, Vec<Option<String>>) = sqlx::query_as(&format!(
        "SELECT count(*), coalesce((array_agg(rel_path) FILTER (WHERE id % 97 = 0))[1:{}], \
                                   ARRAY[min(rel_path), max(rel_path)]) \
         FROM calls WHERE location_id = $1",
        crate::storage::SAMPLED
    ))
    .bind(location)
    .fetch_one(&mut *conn)
    .await?;
    let sample: Vec<String> = sample.into_iter().flatten().collect();
    let mut preview = ConsolidatePreview {
        name: name.to_string(),
        calls,
        bytes_about: None,
        free_bytes: None,
        refused: None,
    };
    if calls == 0 {
        preview.refused = Some(format!("{name} holds no call"));
        return Ok(preview);
    }
    if let Some(job) = running(conn, name).await? {
        preview.refused = Some(format!("job {job} works on {name} already"));
        return Ok(preview);
    }
    let archive: String = sqlx::query_scalar("SELECT root FROM locations WHERE is_primary")
        .fetch_one(&mut *conn)
        .await?;
    let measured = tokio::time::timeout(
        MEASURE_LIMIT,
        tokio::task::spawn_blocking(move || {
            let bytes = crate::storage::bytes_about(Path::new(&root), &sample, calls);
            let disk = rustix::fs::statvfs(Path::new(&archive)).map(|s| {
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
                "{name}, or the main archive, did not answer within {} s",
                MEASURE_LIMIT.as_secs()
            ));
            return Ok(preview);
        }
    };
    preview.bytes_about = bytes;
    let Some(bytes) = bytes else {
        preview.refused = Some(format!(
            "{name}'s files cannot be read here to measure them"
        ));
        return Ok(preview);
    };
    match disk {
        Ok((free, total)) => {
            preview.free_bytes = Some(i64::try_from(free).unwrap_or(i64::MAX));
            preview.refused = room_problem(
                "the main archive",
                u64::try_from(bytes).unwrap_or(0),
                free,
                total,
            );
        }
        Err(error) => preview.refused = Some(format!("the main archive: {error}")),
    }
    Ok(preview)
}

/// A job that imports into, or consolidates, the import `name`, queued or
/// running.
async fn running(conn: &mut PgConnection, name: &str) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind IN ('import', 'consolidate') \
           AND state IN ('queued', 'running') AND params ->> 'name' = $1 ORDER BY id LIMIT 1",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
}

/// Adds a job that consolidates the import `name`, once [`preview`] finds
/// nothing against it and `move` is typed. Returns the job's id and what was
/// counted.
pub async fn start(
    conn: &mut PgConnection,
    name: &str,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, ConsolidatePreview), JobError> {
    let preview = preview(conn, name).await?;
    if let Some(why) = &preview.refused {
        return Err(JobError::Refused(why.clone()));
    }
    if confirm != CONFIRM {
        return Err(JobError::Refused(format!("type {CONFIRM} to confirm")));
    }
    let (location, _) = import_of(conn, name).await?;
    let params = serde_json::to_value(Consolidate {
        name: name.to_string(),
        location,
    })
    .expect("plain data");
    let job = jobs::create(
        &mut *conn,
        Kind::Consolidate,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    Ok((job, preview))
}

/// A consolidation's state between steps.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Work {
    /// The last call examined.
    after: Option<(DateTime<Utc>, i64)>,
    copied: i64,
    missing: i64,
    changed: i64,
    refused: i64,
    last_refusal: Option<String>,
}

impl Work {
    fn message(&self, name: &str) -> String {
        let mut message = format!(
            "{} calls of {name} copied into the main archive; {} missing, {} changed while they were copied, {} refused",
            self.copied, self.missing, self.changed, self.refused
        );
        if let Some(why) = &self.last_refusal {
            message.push_str(&format!("; the last refusal: {why}"));
        }
        message
    }
}

/// A call one step copies.
#[derive(Debug, sqlx::FromRow)]
struct Copying {
    id: i64,
    started_at: DateTime<Utc>,
    rel_path: String,
}

/// What copying one call did.
enum Copied {
    Done,
    Missing,
    /// It is not where the step's batch read it: a purge or a correction
    /// came first.
    Changed,
    Refused(String),
    /// Its file, or the archive, did not answer within the limit.
    Hung(String),
}

/// The consolidation ends with its counts when a folder stops answering:
/// started again once it answers, it takes the calls left.
fn hung(what: &str, work: &Work, name: &str) -> Outcome {
    Outcome::Failed(format!(
        "{what}: {}; start it again once it answers",
        work.message(name)
    ))
}

pub(crate) async fn step(
    conn: &mut PgConnection,
    job: &Job,
    limit: Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Consolidate>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a consolidation's".into(),
        ));
    };
    let mut work: Work = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let zone = match crate::settings::load(&mut *conn).await {
        Ok(settings) => settings.time_zone,
        Err(crate::settings::SettingsError::Database(error)) => return Err(error),
        Err(other) => return Ok(Outcome::Failed(other.to_string())),
    };
    // Shared, so that neither location is removed while the step runs.
    let places: Vec<(i16, String, bool)> = sqlx::query_as(
        "SELECT id, root, is_primary FROM locations WHERE id = $1 OR is_primary FOR SHARE",
    )
    .bind(params.location)
    .fetch_all(&mut *conn)
    .await?;
    let (Some(source), Some(archive)) = (
        places.iter().find(|(id, ..)| *id == params.location),
        places.iter().find(|(.., primary)| *primary),
    ) else {
        return Ok(Outcome::Failed(format!(
            "{} is gone: {}",
            params.name,
            work.message(&params.name)
        )));
    };
    let (source_root, archive_id, archive_root) = (
        PathBuf::from(&source.1),
        archive.0,
        PathBuf::from(&archive.1),
    );
    let checked = (
        source_root.clone(),
        archive_root.clone(),
        params.name.clone(),
    );
    let probed = bounded(limit, move || {
        let (source, archive, name) = checked;
        if std::fs::read_dir(&source)?.next().is_none() {
            return Err(io::Error::other(format!(
                "{name} is not mounted at {} on this machine",
                source.display()
            )));
        }
        let stats = rustix::fs::statvfs(&archive)?;
        Ok((
            stats.f_bavail.saturating_mul(stats.f_frsize),
            stats.f_blocks.saturating_mul(stats.f_frsize),
            identities(&[source.as_path(), archive.as_path()]),
        ))
    })
    .await;
    let (free, total, before) = match probed {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(hung(
                &format!("{}, or the main archive, {error}", params.name),
                &work,
                &params.name,
            ));
        }
        Err(error) => return Err(io_error(error)),
    };
    if free < margin(total) {
        return Ok(Outcome::Failed(format!(
            "the main archive has {} free, less than the {} it keeps free: {}; make room and start it again",
            size(free),
            size(margin(total)),
            work.message(&params.name)
        )));
    }
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<Copying> = sqlx::query_as(
        "SELECT id, started_at, rel_path FROM calls WHERE location_id = $1 \
           AND ($2::timestamptz IS NULL OR (started_at, id) > ($2, $3)) \
         ORDER BY started_at, id LIMIT $4",
    )
    .bind(params.location)
    .bind(after_at)
    .bind(after_id)
    .bind(FILES_PER_STEP)
    .fetch_all(&mut *conn)
    .await?;
    if batch.is_empty() {
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE location_id = $1")
            .bind(params.location)
            .fetch_one(&mut *conn)
            .await?;
        let mut message = work.message(&params.name);
        message.push_str(&if left == 0 {
            format!("; {} holds no call now: it can be removed", params.name)
        } else {
            format!("; {} still holds {left} calls", params.name)
        });
        return Ok(Outcome::Done(job.done, message));
    }
    let folders = vec![source_root.clone(), archive_root.clone()];
    for call in &batch {
        let place = (
            params.location,
            source_root.as_path(),
            archive_id,
            archive_root.as_path(),
        );
        match copy_one(conn, place, call, zone, limit).await? {
            Copied::Done => work.copied += 1,
            Copied::Missing => work.missing += 1,
            Copied::Changed => work.changed += 1,
            Copied::Refused(why) => {
                work.refused += 1;
                work.last_refusal = Some(why);
            }
            Copied::Hung(why) => {
                unchanged(&folders, &before, limit).await?;
                return Ok(hung(&why, &work, &params.name));
            }
        }
        work.after = Some((call.started_at, call.id));
    }
    unchanged(&folders, &before, limit).await?;
    let done = job.done + batch.len() as i64;
    let total = job.total.map(|total| total.max(done));
    Ok(Outcome::More {
        done,
        total,
        message: format!(
            "{} calls: {} copied into the main archive, {} missing, {} changed, {} refused",
            jobs::progress(done, total),
            work.copied,
            work.missing,
            work.changed,
            work.refused
        ),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}

/// Copies the call's file into the main archive's layout (the first free
/// name there), checks it, gives it the file's modification time and points
/// the call at it. The import's file stays, and is listed for nothing. The
/// call is locked until the step's end.
async fn copy_one(
    conn: &mut PgConnection,
    (location, source_root, archive_id, archive_root): (i16, &Path, i16, &Path),
    call: &Copying,
    zone: Tz,
    limit: Duration,
) -> sqlx::Result<Copied> {
    let now: Option<(i16, String)> = sqlx::query_as(
        "SELECT location_id, rel_path FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE",
    )
    .bind(call.id)
    .bind(call.started_at)
    .fetch_optional(&mut *conn)
    .await?;
    if now != Some((location, call.rel_path.clone())) {
        return Ok(Copied::Changed);
    }
    let refused = |why: String| Copied::Refused(format!("{}: {why}", call.rel_path));
    let timed_out = |error: &io::Error| error.kind() == io::ErrorKind::TimedOut;
    let old = source_root.join(&call.rel_path);
    let read = bounded(limit, move || {
        let data = std::fs::read(&old)?;
        let mtime = std::fs::metadata(&old)?.modified()?;
        Ok((data, mtime))
    })
    .await;
    let (data, mtime) = match read {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Copied::Missing),
        Err(error) if timed_out(&error) => {
            return Ok(Copied::Hung(format!("{} {error}", call.rel_path)));
        }
        Err(error) => return Ok(refused(error.to_string())),
    };
    // Filed as the filer files a recorder's file: the day it started and the
    // talkgroup its file names.
    let name = call.rel_path.rsplit('/').next().unwrap_or(&call.rel_path);
    let described = reader::describe(name, &reader::tags(&data));
    let folder = archive::folder(
        described
            .talkgroup
            .as_ref()
            .map(|tg| (tg.number, tg.name.as_str())),
    );
    let day = call.started_at.with_timezone(&zone).date_naive();
    let rel = match archive::rel_path(day, &folder, name) {
        Ok(rel) => rel,
        Err(why) => return Ok(refused(why)),
    };
    let dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
    let writing = writes().await?;
    let (rel, copy) =
        match free_rel(conn, archive_root, archive_id, dir, name, &data, limit).await? {
            Ok(found) => found,
            Err(error) if timed_out(&error) => {
                return Ok(Copied::Hung(format!("{} {error}", archive_root.display())));
            }
            Err(error) => return Ok(refused(error.to_string())),
        };
    let path = archive_root.join(&rel);
    let written = bounded(limit, move || {
        if !copy {
            archive::write_part(&path, &data, &archive::sha256(&data))?;
            archive::finish_part(&path)?;
        }
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(mtime)
    })
    .await;
    match written {
        Ok(()) => {}
        Err(error) if timed_out(&error) => {
            return Ok(Copied::Hung(format!("{} {error}", archive_root.display())));
        }
        Err(error) => return Ok(refused(error.to_string())),
    }
    drop(writing);
    sqlx::query(
        "UPDATE calls SET location_id = $3, rel_path = $4 WHERE id = $1 AND started_at = $2",
    )
    .bind(call.id)
    .bind(call.started_at)
    .bind(archive_id)
    .bind(&rel)
    .execute(&mut *conn)
    .await?;
    Ok(Copied::Done)
}
