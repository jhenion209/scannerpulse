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

/// The file an administrator puts in the holding folder to show that it is
/// the real one, mounted: an unmounted share's mount point is an empty folder
/// without it.
pub const HOLDING_SENTINEL: &str = ".scannerpulse-holding";

/// The calls one step of a correction takes.
pub const FILES_PER_STEP: i64 = 50;

/// SQL: the call `c` was filed through a mapped spelling of its system, so
/// its tag still names the spelling.
macro_rules! mapped {
    () => {
        "EXISTS (SELECT 1 FROM mapped_calls m WHERE m.call_id = c.id AND m.started_at = c.started_at)"
    };
}

/// SQL: the call `c` waits for a correction to the folder `$2`, or to its
/// system's name.
macro_rules! to_correct {
    () => {
        concat!("(split_part(c.rel_path, '/', 2) <> $2 OR ", mapped!(), ")")
    };
}

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
        concat!(
            "SELECT count(*) FILTER (WHERE l.mode = 'managed'), count(*) FILTER (WHERE l.mode <> 'managed') \
             FROM calls c JOIN locations l ON l.id = c.location_id \
             WHERE c.talkgroup_id = $1 AND ",
            to_correct!()
        ),
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
    /// A purge's first step has checked the holding folder.
    #[serde(default)]
    marked: bool,
    /// The holding folder's canonical path, as the first step found it.
    #[serde(default)]
    holding: Option<String>,
    /// A purge's calls in read-only locations: only their records went.
    #[serde(default)]
    records_only: i64,
}

/// A call whose file a step handles.
#[derive(Debug, sqlx::FromRow)]
struct CallFile {
    id: i64,
    started_at: DateTime<Utc>,
    location_id: i16,
    rel_path: String,
    /// Filed through a mapped spelling: its tag names the spelling.
    #[sqlx(default)]
    mapped: bool,
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

pub(crate) fn io_error(error: io::Error) -> sqlx::Error {
    sqlx::Error::Io(error)
}

/// The archive lock, or an error that makes the step roll back and try
/// again: a step holds rows the filer may be waiting for while it waits for
/// the lock, so it never waits long.
pub(crate) async fn writes() -> sqlx::Result<tokio::sync::MutexGuard<'static, ()>> {
    tokio::time::timeout(std::time::Duration::from_secs(5), archive::WRITES.lock())
        .await
        .map_err(|_| io_error(io::Error::other("the archive is busy; trying again")))
}

/// Which directory each folder is: its device and inode. A share unmounted
/// (or mounted) while a step runs makes the same path a different directory.
pub(crate) fn identities(folders: &[&Path]) -> Vec<Option<(u64, u64)>> {
    use std::os::unix::fs::MetadataExt;
    folders
        .iter()
        .map(|folder| {
            std::fs::metadata(folder)
                .ok()
                .map(|meta| (meta.dev(), meta.ino()))
        })
        .collect()
}

/// Err when a folder became another directory during the step: the step then
/// rolls back, so no row names a file written under a mount point.
pub(crate) fn same_identities(
    folders: &[&Path],
    before: &[Option<(u64, u64)>],
) -> sqlx::Result<()> {
    for (folder, (now, then)) in folders.iter().zip(identities(folders).iter().zip(before)) {
        if now != then {
            return Err(io_error(io::Error::other(format!(
                "{} changed during the step (unmounted?)",
                folder.display()
            ))));
        }
    }
    Ok(())
}

/// Removes the files a committed step replaced, unless a call names one
/// again, and then their folder when it is empty, each in a thread of its
/// own for `limit` at most. A location that does not answer leaves the files
/// left to the filer's sweep, which leaves a hung share alone.
async fn remove_gone(
    conn: &mut PgConnection,
    roots: &HashMap<i16, PathBuf>,
    gone: &[Gone],
    limit: std::time::Duration,
) -> sqlx::Result<()> {
    for (n, file) in gone.iter().enumerate() {
        // Per file, so that the filer's write of the same name cannot slip
        // between the check and the removal.
        let _writing = writes().await?;
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
        let removing = path.clone();
        let removed = bounded(limit, move || {
            match std::fs::remove_file(&removing) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            if let Some(dir) = removing.parent() {
                // Only an empty folder goes.
                let _ = std::fs::remove_dir(dir);
            }
            Ok(())
        })
        .await;
        match removed {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                crate::log::warn(format!(
                    "{} {error}: the filer's sweep deletes the files left",
                    root.display()
                ));
                return list_gone(conn, &gone[n..]).await;
            }
            Err(error) => crate::log::warn(format!("cannot remove {}: {error}", path.display())),
        }
    }
    Ok(())
}

/// Lists replaced files for the filer's sweep, which deletes each once no
/// call names it.
async fn list_gone(conn: &mut PgConnection, gone: &[Gone]) -> sqlx::Result<()> {
    for file in gone {
        sqlx::query(
            "INSERT INTO archive_leftovers (location_id, rel_path) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(file.location_id)
        .bind(&file.rel_path)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// The first name in `dir` for a file of `data`: one no call has, no leftover
/// lists and no other file holds, or an exact copy left by a step that did
/// not commit. A listed leftover is never taken again: the sweep, which
/// cannot see this step's uncommitted calls, would delete it.
pub(crate) async fn free_rel(
    conn: &mut PgConnection,
    root: &Path,
    location_id: i16,
    dir: &str,
    name: &str,
    data: &[u8],
    limit: std::time::Duration,
) -> sqlx::Result<io::Result<(String, bool)>> {
    let digest = archive::sha256(data);
    let mut n = 0;
    loop {
        let rel = format!("{dir}/{}", archive::candidate(name, n));
        n += 1;
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM calls WHERE location_id = $1 AND rel_path = $2) \
                 OR EXISTS (SELECT 1 FROM archive_leftovers WHERE location_id = $1 AND rel_path = $2)",
        )
        .bind(location_id)
        .bind(&rel)
        .fetch_one(&mut *conn)
        .await?;
        if taken {
            continue;
        }
        let (path, len) = (root.join(&rel), data.len() as u64);
        let found = bounded(limit, move || {
            let _ = std::fs::remove_file(archive::part_path(&path));
            match std::fs::metadata(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Some(false)),
                Err(error) => Err(error),
                Ok(meta) if meta.len() == len => {
                    let (found, _) = archive::file_digest(&path)?;
                    Ok((found == digest).then_some(true))
                }
                Ok(_) => Ok(None),
            }
        })
        .await;
        match found {
            Ok(Some(copy)) => return Ok(Ok((rel, copy))),
            Ok(None) => {}
            Err(error) => return Ok(Err(error)),
        }
    }
}

/// Runs `work` in a thread of its own for `limit` at most: a share that stops
/// answering then holds up no runtime worker, nor the archive's writes its
/// caller holds. A thread that outlives the limit is left to end by itself,
/// and the error's kind is `TimedOut`.
pub(crate) async fn bounded<T: Send + 'static>(
    limit: std::time::Duration,
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(result)) => result,
        Ok(Err(join)) => Err(io::Error::other(join.to_string())),
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("did not answer within {} s", limit.as_secs()),
        )),
    }
}

/// A call's file as [`read_file`] found it.
enum Read {
    File(Vec<u8>, std::time::SystemTime),
    /// Not there, in a folder that is.
    Missing,
    /// Its folder is not there either: an unmounted share would otherwise
    /// look like a share of missing files.
    NoFolder,
}

/// Reads the file at `path` and its modification time, in a thread of its
/// own for `limit` at most.
async fn read_file(path: &Path, limit: std::time::Duration) -> io::Result<Read> {
    let path = path.to_path_buf();
    bounded(limit, move || match std::fs::read(&path) {
        Ok(data) => Ok(Read::File(data, std::fs::metadata(&path)?.modified()?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(if path.parent().is_some_and(|dir| dir.is_dir()) {
                Read::Missing
            } else {
                Read::NoFolder
            })
        }
        Err(error) => Err(error),
    })
    .await
}

/// Each folder's identity ([`identities`]) and whether it is a folder now,
/// read in a thread of its own for `limit` at most.
async fn folders_now(
    folders: &[PathBuf],
    limit: std::time::Duration,
) -> io::Result<(Vec<Option<(u64, u64)>>, Vec<bool>)> {
    let folders = folders.to_vec();
    bounded(limit, move || {
        let paths: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
        Ok((
            identities(&paths),
            folders.iter().map(|f| f.is_dir()).collect(),
        ))
    })
    .await
}

/// Err unless every folder is still the one it was when the step began
/// ([`same_identities`]), read in a thread of its own for `limit` at most.
pub(crate) async fn unchanged(
    folders: &[PathBuf],
    before: &[Option<(u64, u64)>],
    limit: std::time::Duration,
) -> sqlx::Result<()> {
    let (folders, before) = (folders.to_vec(), before.to_vec());
    bounded(limit, move || {
        let paths: Vec<&Path> = folders.iter().map(PathBuf::as_path).collect();
        same_identities(&paths, &before).map_err(|error| io::Error::other(error.to_string()))
    })
    .await
    .map_err(io_error)
}

/// Whether `root` was found a folder by [`folders_now`].
fn present(folders: &[PathBuf], here: &[bool], root: &Path) -> bool {
    folders
        .iter()
        .zip(here)
        .any(|(folder, here)| folder == root && *here)
}

/// What correcting one call's file did.
enum Handled {
    /// The row names the new file; the old one, if it is another, goes in
    /// the next step.
    Moved(Option<Gone>),
    Refused(String),
    Missing,
    /// Its file did not answer within the limit: a share that stopped
    /// answering.
    Hung(String),
}

/// Whether the call still has the file its step's batch read; it is locked
/// for the rest of the step. A longer copy from another recorder may have
/// replaced its recording since (the filer waits for the lock a short while
/// only). The current path, or `None` when the call is gone.
async fn locked_file(
    conn: &mut PgConnection,
    call: &CallFile,
) -> sqlx::Result<Option<(i16, String)>> {
    sqlx::query_as(
        "SELECT location_id, rel_path FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE",
    )
    .bind(call.id)
    .bind(call.started_at)
    .fetch_optional(&mut *conn)
    .await
}

/// Writes the call's file under `to`'s folder and name, with `to` in its
/// title and, for a mapped call, `system` in its tag, and moves the row to
/// it. A mapped call's file already where it belongs is rewritten there. A
/// call whose file changed since the batch was read is refused before
/// anything is written.
async fn correct_one(
    conn: &mut PgConnection,
    root: &Path,
    call: &CallFile,
    to: &Talkgroup,
    folder: &str,
    system: &str,
    limit: std::time::Duration,
) -> sqlx::Result<Handled> {
    if locked_file(conn, call).await? != Some((call.location_id, call.rel_path.clone())) {
        return Ok(Handled::Refused(format!(
            "call {} changed while its file was corrected",
            call.id
        )));
    }
    let parts: Vec<&str> = call.rel_path.split('/').collect();
    let [day, _, name] = parts[..] else {
        return Ok(Handled::Refused(format!(
            "{} is not <day>/<folder>/<file>",
            call.rel_path
        )));
    };
    let hung = |error: &io::Error| error.kind() == io::ErrorKind::TimedOut;
    let (data, mtime) = match read_file(&root.join(&call.rel_path), limit).await {
        Ok(Read::File(data, mtime)) => (data, mtime),
        Ok(Read::Missing) => return Ok(Handled::Missing),
        Ok(Read::NoFolder) => {
            return Ok(Handled::Refused(format!(
                "{}: its folder is not there",
                call.rel_path
            )));
        }
        Err(error) if hung(&error) => {
            return Ok(Handled::Hung(format!("{} {error}", call.rel_path)));
        }
        Err(error) => {
            return Ok(Handled::Refused(format!("{}: {error}", call.rel_path)));
        }
    };
    let retagged = match crate::retag::retag(name, &data, to, call.mapped.then_some(system)) {
        Ok(retagged) => retagged,
        Err(why) => return Ok(Handled::Refused(format!("{}: {why}", call.rel_path))),
    };
    let dir = format!("{day}/{folder}");
    // The name is chosen and given under the lock, so that the filer, which
    // takes the same one, never takes it too.
    let writing = writes().await?;
    let in_place = format!("{dir}/{}", retagged.name) == call.rel_path;
    let (rel, copy) = if in_place {
        (call.rel_path.clone(), false)
    } else {
        match free_rel(
            conn,
            root,
            call.location_id,
            &dir,
            &retagged.name,
            &retagged.data,
            limit,
        )
        .await?
        {
            Ok(found) => found,
            Err(error) if hung(&error) => {
                return Ok(Handled::Hung(format!("{} {error}", root.display())));
            }
            // A name the file system refuses (too long, a file where a folder
            // should be) refuses the call; the job goes on.
            Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
        }
    };
    let path = root.join(&rel);
    let bytes = retagged.data;
    let written = bounded(limit, move || {
        if !copy {
            let digest = archive::sha256(&bytes);
            archive::write_part(&path, &bytes, &digest)?;
            // After this the new file is an orphan if the step fails: the
            // next attempt finds it as an exact copy and reuses it. A file
            // rewritten in place holds the old tag or the new one, and the
            // next attempt rewrites it again.
            if in_place {
                archive::replace_with_part(&path)?;
            } else {
                archive::finish_part(&path)?;
            }
        }
        // A later import reads a time in the repeated hour with the file's
        // modification time, so the new file keeps the old one's.
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(mtime)
    })
    .await;
    match written {
        Ok(()) => {}
        Err(error) if hung(&error) => {
            return Ok(Handled::Hung(format!("{} {error}", root.display())));
        }
        Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
    }
    drop(writing);
    if call.mapped {
        sqlx::query("DELETE FROM mapped_calls WHERE call_id = $1 AND started_at = $2")
            .bind(call.id)
            .bind(call.started_at)
            .execute(&mut *conn)
            .await?;
    }
    if in_place {
        return Ok(Handled::Moved(None));
    }
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
    Ok(Handled::Moved(Some(Gone {
        location_id: call.location_id,
        rel_path: call.rel_path.clone(),
    })))
}

/// Removes the files that the job's last committed step replaced: a stopped
/// job does it before it ends.
pub(crate) async fn release(
    conn: &mut PgConnection,
    job: &Job,
    limit: std::time::Duration,
) -> sqlx::Result<()> {
    let work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove, limit).await
}

pub(crate) async fn correct_step(
    conn: &mut PgConnection,
    job: &Job,
    limit: std::time::Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Correct>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a correction's".into(),
        ));
    };
    let mut work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let step = correct_files(
        conn,
        params.talkgroup_id,
        &mut work,
        job.done,
        job.total,
        limit,
    )
    .await?;
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
    limit: std::time::Duration,
) -> sqlx::Result<FileStep> {
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove, limit).await?;
    work.remove.clear();
    let Some(talkgroup) = named_locked(&mut *conn, talkgroup_id).await? else {
        return Ok(FileStep::Failed(format!(
            "talkgroup {talkgroup_id} is gone"
        )));
    };
    let folder = talkgroup.folder();
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<CallFile> = sqlx::query_as(concat!(
        "SELECT c.id, c.started_at, c.location_id, c.rel_path, ",
        mapped!(),
        " AS mapped \
             FROM calls c JOIN locations l ON l.id = c.location_id \
             WHERE c.talkgroup_id = $1 AND l.mode = 'managed' AND ",
        to_correct!(),
        " AND ($3::timestamptz IS NULL OR (c.started_at, c.id) > ($3, $4)) \
             ORDER BY c.started_at, c.id LIMIT $5"
    ))
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
    let folders: Vec<PathBuf> = roots.values().cloned().collect();
    let corrected_so_far = |what: &str, moved| {
        format!("{what}: {moved} files were corrected; start the correction again once it answers")
    };
    let (before, here) = match folders_now(&folders, limit).await {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(FileStep::Failed(corrected_so_far(
                &format!("a location {error}"),
                work.moved,
            )));
        }
        Err(error) => return Err(io_error(error)),
    };
    for call in &batch {
        if let Some(root) = roots.get(&call.location_id) {
            if !present(&folders, &here, root) {
                return Err(io_error(io::Error::other(format!(
                    "the location {} is not there",
                    root.display()
                ))));
            }
            match correct_one(conn, root, call, &to, &folder, &talkgroup.system, limit).await? {
                Handled::Moved(gone) => {
                    work.moved += 1;
                    work.remove.extend(gone);
                }
                Handled::Refused(why) => {
                    work.refused += 1;
                    work.last_refusal = Some(why);
                }
                Handled::Missing => work.missing += 1,
                Handled::Hung(why) => {
                    // The calls corrected before it stay so, once their
                    // folders are checked; their old files go to the sweep.
                    unchanged(&folders, &before, limit).await?;
                    list_gone(conn, &work.remove).await?;
                    return Ok(FileStep::Failed(corrected_so_far(&why, work.moved)));
                }
            }
        }
        work.after = Some((call.started_at, call.id));
    }
    unchanged(&folders, &before, limit).await?;
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
/// it is left in another folder of a location it may write. A read-only
/// location's files are never written, so they keep their names and their
/// own layout. Returns the job's last message.
async fn finish_correction(
    conn: &mut PgConnection,
    talkgroup: &Named,
    folder: &str,
    work: &FileWork,
) -> sqlx::Result<String> {
    let (left, read_only): (i64, i64) = sqlx::query_as(concat!(
        "SELECT count(*), count(*) FILTER (WHERE l.mode <> 'managed') FROM calls c \
         JOIN locations l ON l.id = c.location_id WHERE c.talkgroup_id = $1 AND ",
        to_correct!()
    ))
    .bind(talkgroup.id)
    .bind(folder)
    .fetch_one(&mut *conn)
    .await?;
    if left == read_only {
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

pub(crate) async fn merge_step(
    conn: &mut PgConnection,
    job: &Job,
    limit: std::time::Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Merge>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a merge's".into(),
        ));
    };
    let mut work: MergeWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    if work.correcting {
        let step = correct_files(
            conn,
            params.into,
            &mut work.files,
            job.done,
            job.total,
            limit,
        )
        .await?;
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
            sqlx::query_scalar(concat!(
                "SELECT count(*) FROM calls c JOIN locations l ON l.id = c.location_id \
                     WHERE c.talkgroup_id = $1 AND l.mode = 'managed' AND ",
                to_correct!()
            ))
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
    /// Calls in read-only locations: only their records go, and their files
    /// stay where they are.
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
    /// The setting as it was when the job started; the host that runs the job
    /// checks it.
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
    confirmed(&preview.talkgroup, confirm)?;
    let filling: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM jobs WHERE kind = 'merge' AND state IN ('queued', 'running') \
           AND (params->>'into')::int = $1 LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if filling.is_some() {
        return Err(JobError::Refused(
            "a merge into this talkgroup is waiting or running; purge after it".into(),
        ));
    }
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

/// The holding folder `raw` as a purge keeps it, resolved, or why it cannot:
/// it is a folder here, apart from every archive location and ingest source
/// in `others` (a file kept in an archive tree would be the file the purge
/// then removes, and a held file in an ingest source would be ingested
/// again), and it holds the sentinel.
fn holding_checked(raw: &Path, others: &[(&str, PathBuf)]) -> Result<PathBuf, String> {
    if !raw.is_dir() {
        return Err(format!(
            "the holding folder {} is not a folder this host can see",
            raw.display()
        ));
    }
    let kept = std::fs::canonicalize(raw)
        .map_err(|error| format!("cannot read the holding folder {}: {error}", raw.display()))?;
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for (what, other) in others {
        let other = real(other);
        if kept.starts_with(&other) || other.starts_with(&kept) {
            return Err(format!(
                "the holding folder {} overlaps the {what} {}",
                kept.display(),
                other.display()
            ));
        }
    }
    if !kept.join(HOLDING_SENTINEL).is_file() {
        return Err(format!(
            "the holding folder {} has no {HOLDING_SENTINEL} file: put an empty one there to show it is the real folder, mounted",
            kept.display()
        ));
    }
    Ok(kept)
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
/// the archive, so the archive's write lock is not taken. The file is the
/// one the call has now, which a longer copy may have replaced since the
/// batch was read; a call gone or moved to another location is refused.
async fn purge_one(
    conn: &mut PgConnection,
    root: &Path,
    holding: &Path,
    call: &CallFile,
    limit: std::time::Duration,
) -> sqlx::Result<Handled> {
    let call = &match locked_file(conn, call).await? {
        Some((location_id, rel_path)) if location_id == call.location_id => CallFile {
            id: call.id,
            started_at: call.started_at,
            location_id,
            rel_path,
            mapped: call.mapped,
        },
        _ => {
            return Ok(Handled::Refused(format!(
                "call {} changed while it was purged",
                call.id
            )));
        }
    };
    // Read and held in a thread of its own, for `limit` at most.
    let (old, kept_in, rel) = (
        root.join(&call.rel_path),
        holding.to_path_buf(),
        call.rel_path.clone(),
    );
    let held = bounded(limit, move || match std::fs::read(&old) {
        Ok(data) => hold(&old, &kept_in, &rel, &data).map(|()| Some(true)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Missing only when its folder is there: an unmounted share
            // would otherwise look like a share of missing files.
            Ok(old
                .parent()
                .is_some_and(|dir| dir.is_dir())
                .then_some(false))
        }
        Err(error) => Err(error),
    })
    .await;
    let kept = match held {
        Ok(Some(true)) => true,
        Ok(Some(false)) => {
            crate::log::warn(format!(
                "purge: call {} had no file at {}; its row is deleted",
                call.id, call.rel_path
            ));
            false
        }
        Ok(None) => {
            return Ok(Handled::Refused(format!(
                "{}: its folder is not there",
                call.rel_path
            )));
        }
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(Handled::Hung(format!("{} {error}", call.rel_path)));
        }
        Err(error) => return Ok(Handled::Refused(format!("{}: {error}", call.rel_path))),
    };
    sqlx::query("DELETE FROM calls WHERE id = $1 AND started_at = $2")
        .bind(call.id)
        .bind(call.started_at)
        .execute(&mut *conn)
        .await?;
    Ok(if kept {
        Handled::Moved(Some(Gone {
            location_id: call.location_id,
            rel_path: call.rel_path.clone(),
        }))
    } else {
        Handled::Missing
    })
}

/// Deletes the record of a call in a read-only location, whose file stays:
/// false when the call is gone or has another file than the batch read.
async fn forget_one(conn: &mut PgConnection, call: &CallFile) -> sqlx::Result<bool> {
    let gone = sqlx::query(
        "DELETE FROM calls WHERE id = $1 AND started_at = $2 AND location_id = $3 AND rel_path = $4",
    )
    .bind(call.id)
    .bind(call.started_at)
    .bind(call.location_id)
    .bind(&call.rel_path)
    .execute(&mut *conn)
    .await?;
    Ok(gone.rows_affected() == 1)
}

pub(crate) async fn purge_step(
    conn: &mut PgConnection,
    job: &Job,
    limit: std::time::Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Purge>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a purge's".into(),
        ));
    };
    let mut work: FileWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let roots = roots(conn).await?;
    remove_gone(conn, &roots, &work.remove, limit).await?;
    work.remove.clear();
    let purged_so_far = |what: &str, work: &FileWork| {
        format!(
            "{what}: {} calls were purged; start the purge again once it answers",
            work.moved + work.missing + work.records_only
        )
    };
    if !work.marked {
        // The first step runs on the host that holds the job's lease, which
        // is where the folder must be seen: check it here, once, in a thread
        // of its own for `limit` at most.
        let sources: Vec<String> = sqlx::query_scalar("SELECT path FROM sources")
            .fetch_all(&mut *conn)
            .await?;
        let raw = PathBuf::from(&params.holding);
        let others: Vec<(&'static str, PathBuf)> = roots
            .values()
            .map(|root| ("archive location", root.clone()))
            .chain(
                sources
                    .into_iter()
                    .map(|path| ("ingest source", path.into())),
            )
            .collect();
        let kept = match bounded(limit, move || Ok(holding_checked(&raw, &others))).await {
            Ok(Ok(kept)) => kept,
            Ok(Err(why)) => return Ok(Outcome::Failed(why)),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                return Ok(Outcome::Failed(purged_so_far(
                    &format!("the holding folder {} {error}", params.holding),
                    &work,
                )));
            }
            Err(error) => return Err(io_error(error)),
        };
        work.marked = true;
        work.holding = Some(kept.to_string_lossy().into_owned());
    }
    let holding_path = work.holding.clone().unwrap_or_default();
    let holding = PathBuf::from(&holding_path);
    let (after_at, after_id) = work.after.unzip();
    let batch: Vec<CallFile> = sqlx::query_as(
        "SELECT c.id, c.started_at, c.location_id, c.rel_path \
         FROM calls c \
         WHERE c.talkgroup_id = $1 \
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
            holding_path,
            work.missing
        );
        if work.records_only > 0 {
            message.push_str(&format!(
                "; {} in a read-only location lost only {}",
                work.records_only,
                if work.records_only == 1 {
                    "its record"
                } else {
                    "their records"
                }
            ));
        }
        if work.refused > 0 {
            message.push_str(&format!("; {} refused", work.refused));
        }
        if let Some(why) = &work.last_refusal {
            message.push_str(&format!("; the last refusal: {why}"));
        }
        let remain: i64 = sqlx::query_scalar("SELECT count(*) FROM calls WHERE talkgroup_id = $1")
            .bind(params.talkgroup_id)
            .fetch_one(&mut *conn)
            .await?;
        if remain > 0 {
            message.push_str(&format!(
                "; {remain} remain (refused, or filed during the purge)"
            ));
        }
        return Ok(Outcome::Done(job.done, message));
    }
    // Which directories these are now, before the sentinel is looked at: a
    // share unmounted before this is caught by the sentinel, one unmounted
    // after it by the check at the end of the step.
    let mut folders: Vec<PathBuf> = roots.values().cloned().collect();
    folders.push(holding.clone());
    let (before, here) = match folders_now(&folders, limit).await {
        Ok(found) => found,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(Outcome::Failed(purged_so_far(
                &format!("a location, or the holding folder, {error}"),
                &work,
            )));
        }
        Err(error) => return Err(io_error(error)),
    };
    let sentinel = holding.join(HOLDING_SENTINEL);
    let marked = match bounded(limit, move || Ok(sentinel.is_file())).await {
        Ok(marked) => marked,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(Outcome::Failed(purged_so_far(
                &format!("the holding folder {holding_path} {error}"),
                &work,
            )));
        }
        Err(error) => return Err(io_error(error)),
    };
    if !marked {
        // An unmounted share hides the sentinel: try again later rather than
        // lose a file.
        return Err(io_error(io::Error::other(format!(
            "the holding folder {holding_path} has no {HOLDING_SENTINEL} (unmounted?)"
        ))));
    }
    let read_only: Vec<i16> =
        sqlx::query_scalar("SELECT id FROM locations WHERE mode <> 'managed'")
            .fetch_all(&mut *conn)
            .await?;
    for call in &batch {
        if read_only.contains(&call.location_id) {
            // Its file is never changed: only the record goes.
            if forget_one(conn, call).await? {
                work.records_only += 1;
            } else {
                work.refused += 1;
                work.last_refusal = Some(format!("call {} changed while it was purged", call.id));
            }
        } else if let Some(root) = roots.get(&call.location_id) {
            if !present(&folders, &here, root) {
                return Err(io_error(io::Error::other(format!(
                    "the location {} is not there",
                    root.display()
                ))));
            }
            match purge_one(conn, root, &holding, call, limit).await? {
                Handled::Moved(gone) => {
                    work.moved += 1;
                    work.remove.extend(gone);
                }
                Handled::Refused(why) => {
                    work.refused += 1;
                    work.last_refusal = Some(why);
                }
                Handled::Missing => work.missing += 1,
                Handled::Hung(why) => {
                    // The calls purged before it stay so, once their folders
                    // are checked; their files in the archive go to the sweep.
                    unchanged(&folders, &before, limit).await?;
                    list_gone(conn, &work.remove).await?;
                    return Ok(Outcome::Failed(purged_so_far(&why, &work)));
                }
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
        message: format!("{} calls purged", jobs::progress(done, total)),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_swapped_during_a_step_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("share");
        std::fs::create_dir(&folder).unwrap();
        let folders = [folder.as_path()];
        let before = identities(&folders);
        assert!(same_identities(&folders, &before).is_ok());
        // An unmount leaves the same path naming another directory.
        std::fs::rename(&folder, dir.path().join("away")).unwrap();
        std::fs::create_dir(&folder).unwrap();
        assert!(same_identities(&folders, &before).is_err());
    }

    /// The batch's path of a call, and the path a longer copy gave it since.
    const OLD: &str = "2031-01-15/215-Old Name/20310115_120000SYSB__TO_215_FROM_4021.mp3";
    const LONGER: &str = "2031-01-15/215-Old Name/20310115_120000SYSB__TO_215_FROM_4021_1.mp3";

    /// A file of SYSB talkgroup 215 "Old Name" at `rel_path` under `root`
    /// (invented values): its tag, then 28 silent frames.
    fn write(root: &Path, rel_path: &str) {
        use id3::TagLike;
        let mut tag = id3::Tag::new();
        tag.set_title("215\"Old Name\"");
        tag.set_text("TIT1", "SYSB");
        let mut data = Vec::new();
        tag.write_to(&mut data, id3::Version::Id3v24).unwrap();
        let mut frame = [0u8; 144];
        frame[..4].copy_from_slice(&[0xFF, 0xF3, 0x48, 0xC4]);
        data.extend(frame.repeat(28));
        let path = root.join(rel_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    /// The call of that talkgroup whose file is `rel_path` in an archive at
    /// `root`, as the step read it in its batch at `read`.
    async fn call_at(pool: &sqlx::PgPool, root: &Path, rel_path: &str, read: &str) -> CallFile {
        let location: i16 = sqlx::query_scalar(
            "INSERT INTO locations (name, root, mode, is_primary) \
             VALUES ('primary', $1, 'managed', true) RETURNING id",
        )
        .bind(root.to_str().unwrap())
        .fetch_one(pool)
        .await
        .unwrap();
        let source: i16 = sqlx::query_scalar(
            "INSERT INTO sources (name, kind, path) VALUES ('live', 'sdrtrunk', '/srv/in') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let talkgroup: i32 = sqlx::query_scalar(
            "WITH s AS (INSERT INTO systems (name) VALUES ('SYSB') RETURNING id) \
             INSERT INTO talkgroups (system_id, number, name) SELECT id, 215, 'Old Name' FROM s RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let at: DateTime<Utc> = "2031-01-15T18:00:00Z".parse().unwrap();
        crate::store::ensure_partition(pool, at).await.unwrap();
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
             VALUES ($1, 1000, $2, $3, $4, $5) RETURNING id",
        )
        .bind(at)
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind(rel_path)
        .fetch_one(pool)
        .await
        .unwrap();
        CallFile {
            id,
            started_at: at,
            location_id: location,
            rel_path: read.to_string(),
            mapped: false,
        }
    }

    // A longer copy replaced the call's recording after the step read its
    // batch: the purge holds and deletes the file the call has now.
    #[sqlx::test(migrations = "./migrations")]
    async fn a_purge_takes_the_file_the_call_has_now(pool: sqlx::PgPool) {
        let (root, holding) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(root.path(), LONGER);
        let read = call_at(&pool, root.path(), LONGER, OLD).await;
        let mut conn = pool.acquire().await.unwrap();
        let handled = purge_one(
            &mut conn,
            root.path(),
            holding.path(),
            &read,
            jobs::FILE_LIMIT,
        )
        .await
        .unwrap();
        assert!(
            matches!(&handled, Handled::Moved(Some(gone)) if gone.rel_path == LONGER),
            "the purge did not take the call's file"
        );
        assert!(holding.path().join(LONGER).is_file());
        let left: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 0);
    }

    // The same for a correction: it refuses the call before it writes a file
    // that no call would name.
    #[sqlx::test(migrations = "./migrations")]
    async fn a_correction_refuses_a_call_whose_file_changed_since_its_batch(pool: sqlx::PgPool) {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), OLD);
        write(root.path(), LONGER);
        let read = call_at(&pool, root.path(), LONGER, OLD).await;
        let to = Talkgroup {
            number: 215,
            name: "New Name".to_string(),
        };
        let mut conn = pool.acquire().await.unwrap();
        let handled = correct_one(
            &mut conn,
            root.path(),
            &read,
            &to,
            "215-New Name",
            "SYSB",
            jobs::FILE_LIMIT,
        )
        .await
        .unwrap();
        assert!(matches!(handled, Handled::Refused(_)));
        assert!(
            !root.path().join("2031-01-15/215-New Name").exists(),
            "the correction wrote a file no call names"
        );
        let now: String = sqlx::query_scalar("SELECT rel_path FROM calls")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(now, LONGER);
    }
}
