//! The Data tab's jobs, which move archived files: the correction of a
//! talkgroup's files after a rename, the merge of a talkgroup into another
//! of its system, and the purge of a talkgroup, whose files go to a holding
//! folder. A file is written under its new name,
//! read back and compared, and its call's row moved to it, in the step's
//! transaction; the old file is removed only by the next step, once that
//! record is committed, so a stop, a crash or a failover never leaves a row
//! without its file.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::PgConnection;

use crate::archive;
use crate::jobs::{self, Job, JobError, Kind, Outcome};
use crate::reader::Talkgroup;

/// The calls one step of a correction takes.
pub const FILES_PER_STEP: i64 = 50;

/// A talkgroup as the Data tab's previews and jobs name it.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Named {
    pub id: i32,
    pub system: String,
    pub number: i64,
    pub name: String,
}

impl Named {
    /// SYSTEM/NUMBER.
    pub fn written(&self) -> String {
        format!("{}/{}", self.system, self.number)
    }

    fn talkgroup(&self) -> Talkgroup {
        Talkgroup {
            number: self.number,
            name: self.name.clone(),
        }
    }

    /// The archive folder its calls belong in.
    fn folder(&self) -> String {
        archive::folder(Some((self.number, &self.name)))
    }
}

pub async fn named(db: impl sqlx::PgExecutor<'_>, id: i32) -> sqlx::Result<Option<Named>> {
    sqlx::query_as(
        "SELECT t.id, s.name AS system, t.number, t.name \
         FROM talkgroups t JOIN systems s ON s.id = t.system_id WHERE t.id = $1",
    )
    .bind(id)
    .fetch_optional(db)
    .await
}

/// The talkgroup, locked for the step's transaction, so that a rename waits
/// for the step.
async fn named_locked(conn: &mut PgConnection, id: i32) -> sqlx::Result<Option<Named>> {
    sqlx::query_as(
        "SELECT t.id, s.name AS system, t.number, t.name \
         FROM talkgroups t JOIN systems s ON s.id = t.system_id WHERE t.id = $1 FOR SHARE OF t",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
}

async fn existing(conn: &mut PgConnection, id: i32) -> Result<Named, JobError> {
    named(&mut *conn, id)
        .await?
        .ok_or_else(|| JobError::Refused(format!("there is no talkgroup {id}")))
}

/// A job that moves or deletes files starts only when the talkgroup's name
/// is typed exactly.
fn confirmed(talkgroup: &Named, confirm: &str) -> Result<(), JobError> {
    if confirm == talkgroup.name {
        Ok(())
    } else {
        Err(JobError::Refused(format!(
            "type the talkgroup's name exactly to confirm: {}",
            talkgroup.name
        )))
    }
}

/// What a correction would do: the calls whose files are in another folder
/// than the name's, in locations it may write, and those in read-only
/// locations, which it leaves.
#[derive(Debug, Clone, Serialize)]
pub struct CorrectPreview {
    pub talkgroup: Named,
    pub calls: i64,
    pub read_only: i64,
}

pub async fn correct_preview(conn: &mut PgConnection, id: i32) -> Result<CorrectPreview, JobError> {
    let talkgroup = existing(conn, id).await?;
    let (calls, read_only): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE l.mode = 'managed'), count(*) FILTER (WHERE l.mode <> 'managed') \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.talkgroup_id = $1 AND split_part(c.rel_path, '/', 2) <> $2",
    )
    .bind(id)
    .bind(talkgroup.folder())
    .fetch_one(&mut *conn)
    .await?;
    Ok(CorrectPreview {
        talkgroup,
        calls,
        read_only,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Correct {
    talkgroup_id: i32,
}

/// Adds a job that corrects the talkgroup's files: folder, file name and
/// ID3 title. Returns its id and the number of calls.
pub async fn start_correct(
    conn: &mut PgConnection,
    id: i32,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, i64), JobError> {
    let preview = correct_preview(conn, id).await?;
    confirmed(&preview.talkgroup, confirm)?;
    let params = serde_json::to_value(Correct { talkgroup_id: id }).expect("plain data");
    let job = jobs::create(
        &mut *conn,
        Kind::Correct,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    Ok((job, preview.calls))
}

/// A file that a committed step replaced, to be removed by the next step.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Gone {
    location_id: i16,
    rel_path: String,
}

/// A correction's state between steps.
#[derive(Debug, Default, Serialize, Deserialize)]
struct FileWork {
    remove: Vec<Gone>,
    /// The last call examined.
    after: Option<(DateTime<Utc>, i64)>,
    moved: i64,
    refused: i64,
    missing: i64,
    last_refusal: Option<String>,
}

/// A call whose file a step handles.
#[derive(Debug, sqlx::FromRow)]
struct CallFile {
    id: i64,
    started_at: DateTime<Utc>,
    location_id: i16,
    rel_path: String,
}

async fn roots(conn: &mut PgConnection) -> sqlx::Result<HashMap<i16, PathBuf>> {
    let rows: Vec<(i16, String)> = sqlx::query_as("SELECT id, root FROM locations")
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, root)| (id, root.into()))
        .collect())
}

fn io_error(error: io::Error) -> sqlx::Error {
    sqlx::Error::Io(error)
}

/// Removes the files a committed step replaced, unless a call names one
/// again, and then their folder when it is empty.
async fn remove_gone(
    conn: &mut PgConnection,
    roots: &HashMap<i16, PathBuf>,
    gone: &[Gone],
) -> sqlx::Result<()> {
    for file in gone {
        let named: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM calls WHERE location_id = $1 AND rel_path = $2)",
        )
        .bind(file.location_id)
        .bind(&file.rel_path)
        .fetch_one(&mut *conn)
        .await?;
        let Some(root) = roots.get(&file.location_id) else {
            continue;
        };
        if named {
            continue;
        }
        let path = root.join(&file.rel_path);
        if let Err(error) = std::fs::remove_file(&path) {
            if error.kind() != io::ErrorKind::NotFound {
                crate::log::warn(format!("cannot remove {}: {error}", path.display()));
            }
        }
        if let Some(dir) = path.parent() {
            // Only an empty folder goes.
            let _ = std::fs::remove_dir(dir);
        }
    }
    Ok(())
}

/// The first name in `dir` for a file of `data`: one no call has and no other
/// file holds, or an exact copy left by a step that did not commit.
async fn free_rel(
    conn: &mut PgConnection,
    root: &Path,
    location_id: i16,
    dir: &str,
    name: &str,
    data: &[u8],
) -> sqlx::Result<io::Result<(String, bool)>> {
    let digest = archive::sha256(data);
    let mut n = 0;
    loop {
        let rel = format!("{dir}/{}", archive::candidate(name, n));
        n += 1;
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM calls WHERE location_id = $1 AND rel_path = $2)",
        )
        .bind(location_id)
        .bind(&rel)
        .fetch_one(&mut *conn)
        .await?;
        if taken {
            continue;
        }
        let path = root.join(&rel);
        let _ = std::fs::remove_file(archive::part_path(&path));
        match std::fs::metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Ok((rel, false))),
            Err(error) => return Ok(Err(error)),
            Ok(meta) if meta.len() == data.len() as u64 => match archive::file_digest(&path) {
                Ok((found, _)) if found == digest => return Ok(Ok((rel, true))),
                Ok(_) => {}
                Err(error) => return Ok(Err(error)),
            },
            Ok(_) => {}
        }
    }
}

/// What correcting one call's file did.
enum Handled {
    /// The row names the new file; the old one goes in the next step.
    Moved(Gone),
    Refused(String),
    Missing,
}

/// Writes the call's file under `to`'s folder and name, with `to` in its
/// title, and moves the row to it.
async fn correct_one(
    conn: &mut PgConnection,
    root: &Path,
    call: &CallFile,
    to: &Talkgroup,
    folder: &str,
) -> sqlx::Result<Handled> {
    let parts: Vec<&str> = call.rel_path.split('/').collect();
    let [day, _, name] = parts[..] else {
        return Ok(Handled::Refused(format!(
            "{} is not <day>/<folder>/<file>",
            call.rel_path
        )));
    };
    let old = root.join(&call.rel_path);
    let data = match std::fs::read(&old) {
        Ok(data) => data,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Handled::Missing),
        Err(error) => {
            return Ok(Handled::Refused(format!("{}: {error}", call.rel_path)));
        }
    };
    let mtime = match std::fs::metadata(&old).and_then(|meta| meta.modified()) {
        Ok(mtime) => mtime,
        Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
    };
    let retagged = match crate::retag::retag(name, &data, to) {
        Ok(retagged) => retagged,
        Err(why) => return Ok(Handled::Refused(format!("{}: {why}", call.rel_path))),
    };
    let dir = format!("{day}/{folder}");
    // The name is chosen and given under the lock, so that the filer, which
    // takes the same one, never takes it too.
    let writing = archive::WRITES.lock().await;
    let (rel, copy) = match free_rel(
        conn,
        root,
        call.location_id,
        &dir,
        &retagged.name,
        &retagged.data,
    )
    .await?
    {
        Ok(found) => found,
        // A name the file system refuses (too long, a file where a folder
        // should be) refuses the call; the job goes on.
        Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
    };
    let path = root.join(&rel);
    let refused = |error: io::Error| Handled::Refused(format!("{}: {error}", call.rel_path));
    if !copy {
        let digest = archive::sha256(&retagged.data);
        if let Err(error) = archive::write_part(&path, &retagged.data, &digest) {
            return Ok(refused(error));
        }
        // After this the new file is an orphan if the step fails: the next
        // attempt finds it as an exact copy and reuses it.
        if let Err(error) = archive::finish_part(&path) {
            return Ok(refused(error));
        }
    }
    // A later import reads a time in the repeated hour with the file's
    // modification time, so the new file keeps the old one's.
    if let Err(error) = std::fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(mtime))
    {
        return Ok(refused(error));
    }
    drop(writing);
    let moved = sqlx::query(
        "UPDATE calls SET rel_path = $4 WHERE id = $1 AND started_at = $2 AND rel_path = $3",
    )
    .bind(call.id)
    .bind(call.started_at)
    .bind(&call.rel_path)
    .bind(&rel)
    .execute(&mut *conn)
    .await?;
    if moved.rows_affected() != 1 {
        return Ok(Handled::Refused(format!(
            "call {} changed while its file was corrected",
            call.id
        )));
    }
    Ok(Handled::Moved(Gone {
        location_id: call.location_id,
        rel_path: call.rel_path.clone(),
    }))
}

/// Removes the files that the job's last committed step replaced: a stopped
/// job does it before it ends.
pub(crate) async fn release(conn: &mut PgConnection, job: &Job) -> sqlx::Result<()> {
    let work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove).await
}

pub(crate) async fn correct_step(conn: &mut PgConnection, job: &Job) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Correct>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a correction's".into(),
        ));
    };
    let mut work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let step = correct_files(conn, params.talkgroup_id, &mut work, job.done, job.total).await?;
    Ok(step.outcome(serde_json::to_value(&work).expect("plain data")))
}

/// What a step of correcting files did.
enum FileStep {
    More {
        done: i64,
        total: Option<i64>,
        message: String,
    },
    Done(i64, String),
    Failed(String),
}

impl FileStep {
    fn outcome(self, work: serde_json::Value) -> Outcome {
        match self {
            FileStep::More {
                done,
                total,
                message,
            } => Outcome::More {
                done,
                total,
                work,
                message,
            },
            FileStep::Done(done, message) => Outcome::Done(done, message),
            FileStep::Failed(why) => Outcome::Failed(why),
        }
    }
}

/// One step of correcting the files of `talkgroup_id`, with `done` of
/// `total` calls examined before it.
async fn correct_files(
    conn: &mut PgConnection,
    talkgroup_id: i32,
    work: &mut FileWork,
    done: i64,
    total: Option<i64>,
) -> sqlx::Result<FileStep> {
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove).await?;
    work.remove.clear();
    let Some(talkgroup) = named_locked(&mut *conn, talkgroup_id).await? else {
        return Ok(FileStep::Failed(format!(
            "talkgroup {talkgroup_id} is gone"
        )));
    };
    let folder = talkgroup.folder();
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<CallFile> = sqlx::query_as(
        "SELECT c.id, c.started_at, c.location_id, c.rel_path \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.talkgroup_id = $1 AND l.mode = 'managed' \
           AND split_part(c.rel_path, '/', 2) <> $2 \
           AND ($3::timestamptz IS NULL OR (c.started_at, c.id) > ($3, $4)) \
         ORDER BY c.started_at, c.id LIMIT $5",
    )
    .bind(talkgroup_id)
    .bind(&folder)
    .bind(after_at)
    .bind(after_id)
    .bind(FILES_PER_STEP)
    .fetch_all(&mut *conn)
    .await?;
    if batch.is_empty() {
        let message = finish_correction(conn, &talkgroup, &folder, work).await?;
        return Ok(FileStep::Done(done, message));
    }
    let to = talkgroup.talkgroup();
    for call in &batch {
        if let Some(root) = roots.get(&call.location_id) {
            if !root.is_dir() {
                return Err(io_error(io::Error::other(format!(
                    "the location {} is not there",
                    root.display()
                ))));
            }
            match correct_one(conn, root, call, &to, &folder).await? {
                Handled::Moved(gone) => {
                    work.moved += 1;
                    work.remove.push(gone);
                }
                Handled::Refused(why) => {
                    work.refused += 1;
                    work.last_refusal = Some(why);
                }
                Handled::Missing => work.missing += 1,
            }
        }
        work.after = Some((call.started_at, call.id));
    }
    let done = done + batch.len() as i64;
    let total = total.map(|total| total.max(done));
    Ok(FileStep::More {
        done,
        total,
        message: format!(
            "{} files: {} corrected, {} refused, {} missing",
            jobs::progress(done, total),
            work.moved,
            work.refused,
            work.missing
        ),
    })
}

/// The last step: the talkgroup's files are marked correct when no call of
/// it is left in another folder. Returns the job's last message.
async fn finish_correction(
    conn: &mut PgConnection,
    talkgroup: &Named,
    folder: &str,
    work: &FileWork,
) -> sqlx::Result<String> {
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM calls WHERE talkgroup_id = $1 AND split_part(rel_path, '/', 2) <> $2",
    )
    .bind(talkgroup.id)
    .bind(folder)
    .fetch_one(&mut *conn)
    .await?;
    if left == 0 {
        sqlx::query(
            "UPDATE talkgroups SET files_correct = true, files_name = NULL WHERE id = $1 AND name = $2",
        )
        .bind(talkgroup.id)
        .bind(&talkgroup.name)
        .execute(&mut *conn)
            .await?;
    }
    let mut message = format!(
        "{} files of {} corrected; {} refused, {} missing",
        work.moved,
        talkgroup.written(),
        work.refused,
        work.missing
    );
    if left > 0 {
        message.push_str(&format!(
            "; {left} still carry an old name (read-only locations, or refused)"
        ));
    }
    if let Some(why) = &work.last_refusal {
        message.push_str(&format!("; the last refusal: {why}"));
    }
    Ok(message)
}

/// The calls one step of a merge moves to the other talkgroup.
pub const MERGE_CALLS_PER_STEP: i64 = 5000;

/// What a merge would do: `from`'s calls go to `into`, then their files are
/// corrected.
#[derive(Debug, Clone, Serialize)]
pub struct MergePreview {
    pub from: Named,
    pub into: Named,
    pub calls: i64,
}

pub async fn merge_preview(
    conn: &mut PgConnection,
    from: i32,
    into: i32,
) -> Result<MergePreview, JobError> {
    if from == into {
        return Err(JobError::Refused(
            "a talkgroup cannot merge into itself".into(),
        ));
    }
    let from = existing(conn, from).await?;
    let into = existing(conn, into).await?;
    if from.system != into.system {
        return Err(JobError::Refused(format!(
            "{} and {} are of different systems",
            from.written(),
            into.written()
        )));
    }
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
        .bind(from.id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(MergePreview { from, into, calls })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Merge {
    from: i32,
    into: i32,
}

/// Adds a job that merges `from` into `into`, once `from`'s name is typed
/// exactly: `from` goes. Returns its id and the number of calls it moves.
pub async fn start_merge(
    conn: &mut PgConnection,
    from: i32,
    into: i32,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, i64), JobError> {
    let preview = merge_preview(conn, from, into).await?;
    confirmed(&preview.from, confirm)?;
    let params = serde_json::to_value(Merge { from, into }).expect("plain data");
    let job = jobs::create(
        &mut *conn,
        Kind::Merge,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    Ok((job, preview.calls))
}

/// A merge's state between steps: moving the calls, then correcting files.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MergeWork {
    correcting: bool,
    calls_moved: i64,
    #[serde(flatten)]
    files: FileWork,
}

/// Marks `into`'s files as not corrected; if it has no name its files carry
/// yet, they carry `from_name`.
async fn mark_uncorrected(
    conn: &mut PgConnection,
    into: i32,
    from_name: Option<String>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE talkgroups SET files_correct = false, \
           files_name = CASE WHEN coalesce(files_name, '') = '' THEN $2 ELSE files_name END \
         WHERE id = $1",
    )
    .bind(into)
    .bind(from_name)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub(crate) async fn merge_step(conn: &mut PgConnection, job: &Job) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Merge>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a merge's".into(),
        ));
    };
    let mut work: MergeWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    if work.correcting {
        let step = correct_files(conn, params.into, &mut work.files, job.done, job.total).await?;
        let step = match step {
            FileStep::Done(done, message) => {
                FileStep::Done(done, format!("{} calls moved; {message}", work.calls_moved))
            }
            other => other,
        };
        return Ok(step.outcome(serde_json::to_value(&work).expect("plain data")));
    }
    if named(&mut *conn, params.into).await?.is_none() {
        return Ok(Outcome::Failed(format!(
            "talkgroup {} is gone; nothing more was moved",
            params.into
        )));
    }
    let from_name: Option<String> = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(params.from)
        .fetch_optional(&mut *conn)
        .await?;
    if from_name.is_none() && work.calls_moved == 0 {
        return Ok(Outcome::Failed(format!(
            "talkgroup {} is gone",
            params.from
        )));
    }
    let first = work.calls_moved == 0;
    let moved = sqlx::query(
        "UPDATE calls SET talkgroup_id = $2 WHERE (id, started_at) IN \
           (SELECT id, started_at FROM calls WHERE talkgroup_id = $1 LIMIT $3)",
    )
    .bind(params.from)
    .bind(params.into)
    .bind(MERGE_CALLS_PER_STEP)
    .execute(&mut *conn)
    .await?
    .rows_affected() as i64;
    work.calls_moved += moved;
    if moved > 0 {
        // The moved files carry the other name from the first move on, so a
        // stop here still leaves the talkgroup marked.
        if first {
            mark_uncorrected(conn, params.into, from_name).await?;
        }
        return Ok(Outcome::More {
            done: work.calls_moved,
            total: job.total.map(|total| total.max(work.calls_moved)),
            message: format!("{} calls moved", work.calls_moved),
            work: serde_json::to_value(&work).expect("plain data"),
        });
    }
    // Every call has moved: the favorites follow, and the talkgroup goes.
    sqlx::query(
        "INSERT INTO favorite_talkgroups (favorite_id, talkgroup_id) \
         SELECT favorite_id, $2 FROM favorite_talkgroups WHERE talkgroup_id = $1 \
         ON CONFLICT DO NOTHING",
    )
    .bind(params.from)
    .bind(params.into)
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM favorite_talkgroups WHERE talkgroup_id = $1")
        .bind(params.from)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM talkgroups WHERE id = $1")
        .bind(params.from)
        .execute(&mut *conn)
        .await?;
    if work.calls_moved > 0 {
        mark_uncorrected(conn, params.into, from_name).await?;
    }
    let to_correct: i64 = match named(&mut *conn, params.into).await? {
        Some(into) => {
            sqlx::query_scalar(
                "SELECT count(*) FROM calls c JOIN locations l ON l.id = c.location_id \
             WHERE c.talkgroup_id = $1 AND l.mode = 'managed' \
               AND split_part(c.rel_path, '/', 2) <> $2",
            )
            .bind(into.id)
            .bind(into.folder())
            .fetch_one(&mut *conn)
            .await?
        }
        None => {
            return Ok(Outcome::Failed(format!(
                "talkgroup {} is gone",
                params.into
            )));
        }
    };
    work.correcting = true;
    Ok(Outcome::More {
        done: 0,
        total: Some(to_correct),
        message: format!(
            "{} calls moved; correcting {to_correct} files",
            work.calls_moved
        ),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}

/// What a purge would do: delete the talkgroup's calls, keeping their files
/// in the holding folder (setting `purge.holding_path`).
#[derive(Debug, Clone, Serialize)]
pub struct PurgePreview {
    pub talkgroup: Named,
    pub calls: i64,
    /// Calls in read-only locations: a purge cannot move their files, so it
    /// refuses to start while there are any.
    pub read_only: i64,
    pub holding_path: Option<String>,
}

pub async fn purge_preview(conn: &mut PgConnection, id: i32) -> Result<PurgePreview, JobError> {
    let talkgroup = existing(conn, id).await?;
    let (calls, read_only): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE l.mode <> 'managed') \
         FROM calls c JOIN locations l ON l.id = c.location_id WHERE c.talkgroup_id = $1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    let holding_path: Option<String> =
        sqlx::query_scalar("SELECT value #>> '{}' FROM settings WHERE key = 'purge.holding_path'")
            .fetch_optional(&mut *conn)
            .await?;
    Ok(PurgePreview {
        talkgroup,
        calls,
        read_only,
        holding_path,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Purge {
    talkgroup_id: i32,
    /// The holding folder as it was when the job started.
    holding: String,
}

/// Adds a job that purges the talkgroup, once its name is typed exactly.
/// Returns its id and the number of calls.
pub async fn start_purge(
    conn: &mut PgConnection,
    id: i32,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, i64), JobError> {
    let preview = purge_preview(conn, id).await?;
    let Some(holding) = preview.holding_path else {
        return Err(JobError::Refused(
            "set purge.holding_path first: the purged calls' files are kept there".into(),
        ));
    };
    if !Path::new(&holding).is_dir() {
        return Err(JobError::Refused(format!(
            "the holding folder {holding} is not a folder this host can see"
        )));
    }
    if preview.read_only > 0 {
        return Err(JobError::Refused(format!(
            "{} of the calls are in read-only locations, whose files a purge cannot move",
            preview.read_only
        )));
    }
    // A file kept in a location's own tree would be the file the purge then
    // removes.
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let kept = real(Path::new(&holding));
    for root in roots(conn).await?.values() {
        let root = real(root);
        if kept.starts_with(&root) || root.starts_with(&kept) {
            return Err(JobError::Refused(format!(
                "the holding folder {holding} overlaps the archive location {}",
                root.display()
            )));
        }
    }
    confirmed(&preview.talkgroup, confirm)?;
    let params = serde_json::to_value(Purge {
        talkgroup_id: id,
        holding,
    })
    .expect("plain data");
    let job = jobs::create(
        &mut *conn,
        Kind::Purge,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    Ok((job, preview.calls))
}

/// Copies the file into the holding folder, with the archive's layout, read
/// back and compared, and gives the copy the file's modification time.
fn hold(old: &Path, holding: &Path, rel_path: &str, data: &[u8]) -> io::Result<()> {
    let mtime = std::fs::metadata(old)?.modified()?;
    let digest = archive::sha256(data);
    let (dir, name) = rel_path.rsplit_once('/').unwrap_or(("", rel_path));
    let rel = match archive::place(holding, dir, name, data.len() as u64, &digest)? {
        archive::Place::Copy(rel) => rel,
        archive::Place::Free(rel) => {
            let path = holding.join(&rel);
            archive::write_part(&path, data, &digest)?;
            archive::finish_part(&path)?;
            rel
        }
    };
    std::fs::File::options()
        .write(true)
        .open(holding.join(&rel))?
        .set_modified(mtime)
}

/// Copies the call's file into the holding folder and deletes its row (its
/// transcripts, labels and queue row go with it). A file that cannot be read
/// or held refuses the call: its row and file stay. The holding folder is not
/// the archive, so the archive's write lock is not taken.
async fn purge_one(
    conn: &mut PgConnection,
    root: &Path,
    holding: &Path,
    call: &CallFile,
) -> sqlx::Result<Handled> {
    let old = root.join(&call.rel_path);
    let kept = match std::fs::read(&old) {
        Ok(data) => match hold(&old, holding, &call.rel_path, &data) {
            Ok(()) => true,
            Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
    };
    sqlx::query("DELETE FROM calls WHERE id = $1 AND started_at = $2")
        .bind(call.id)
        .bind(call.started_at)
        .execute(&mut *conn)
        .await?;
    Ok(if kept {
        Handled::Moved(Gone {
            location_id: call.location_id,
            rel_path: call.rel_path.clone(),
        })
    } else {
        Handled::Missing
    })
}

pub(crate) async fn purge_step(conn: &mut PgConnection, job: &Job) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Purge>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a purge's".into(),
        ));
    };
    let mut work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove).await?;
    work.remove.clear();
    let holding = PathBuf::from(&params.holding);
    if !holding.is_dir() {
        // An unmounted share: try again later rather than lose a file.
        return Err(io_error(io::Error::other(format!(
            "the holding folder {} is not there",
            params.holding
        ))));
    }
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<CallFile> = sqlx::query_as(
        "SELECT c.id, c.started_at, c.location_id, c.rel_path \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.talkgroup_id = $1 AND l.mode = 'managed' \
           AND ($3::timestamptz IS NULL OR (c.started_at, c.id) > ($3, $4)) \
         ORDER BY c.started_at, c.id LIMIT $2",
    )
    .bind(params.talkgroup_id)
    .bind(FILES_PER_STEP)
    .bind(after_at)
    .bind(after_id)
    .fetch_all(&mut *conn)
    .await?;
    if batch.is_empty() {
        let written = named(&mut *conn, params.talkgroup_id)
            .await?
            .map_or_else(|| params.talkgroup_id.to_string(), |t| t.written());
        let mut message = format!(
            "{} calls of {written} purged, their files kept in {}; {} had no file",
            work.moved + work.missing,
            params.holding,
            work.missing
        );
        if work.refused > 0 {
            message.push_str(&format!("; {} refused", work.refused));
        }
        if let Some(why) = &work.last_refusal {
            message.push_str(&format!("; the last refusal: {why}"));
        }
        return Ok(Outcome::Done(job.done, message));
    }
    for call in &batch {
        if let Some(root) = roots.get(&call.location_id) {
            if !root.is_dir() {
                return Err(io_error(io::Error::other(format!(
                    "the location {} is not there",
                    root.display()
                ))));
            }
            match purge_one(conn, root, &holding, call).await? {
                Handled::Moved(gone) => {
                    work.moved += 1;
                    work.remove.push(gone);
                }
                Handled::Refused(why) => {
                    work.refused += 1;
                    work.last_refusal = Some(why);
                }
                Handled::Missing => work.missing += 1,
            }
        }
        work.after = Some((call.started_at, call.id));
    }
    let done = job.done + batch.len() as i64;
    let total = job.total.map(|total| total.max(done));
    Ok(Outcome::More {
        done,
        total,
        message: format!("{} calls purged", jobs::progress(done, total)),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}
