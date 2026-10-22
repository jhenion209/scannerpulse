//! The filer: takes waiting files, oldest first, and files each one into the
//! archive in four steps, each safe against a crash:
//!
//! 1. copy the file into its folder as `<name>.part` and check the copy;
//! 2. give the copy its name;
//! 3. in one transaction, record the call and mark the file done;
//! 4. delete the file from the source.
//!
//! The next pass repairs a crash: before step 1 the filer looks for an exact
//! copy of the file in its folder, and records it instead of copying again.
//! Only the lease holder files, and it checks its lease before every step.
//! A file that cannot be filed is deleted too, and recorded as rejected.
//!
//! A file that is a copy of a call another recorder filed is no new call:
//! the longer recording is kept ([`store::same_call`]). A copy no longer than
//! the call's recording is deleted; a longer one replaces it, in four steps
//! each safe against a crash: it is written into the archive and checked;
//! one transaction points the call at it, its id and start kept; the old
//! recording is deleted from the archive; the file is deleted from its
//! recorder's folder.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;

use crate::archive::{self, Place};
use crate::localtime::TimeHow;
use crate::reader::{self, FileCall, ReadError};
use crate::store::{self, Location, MISSING, NewCall, RecordError, Waiting, limited};
use crate::watcher::{self, Hung};

/// Whether this host may file now, and under which name it holds the lease.
/// The lease answers `held` from its local deadline, without a database call.
pub trait Fence: Send + Sync {
    fn held(&self) -> bool;
    fn holder(&self) -> &str;
}

/// How long a file waits after a failure before it is tried again.
pub const RETRY_AFTER: Duration = Duration::from_secs(60);

/// How often the archive is searched for a file that is missing from its source.
pub const MISSING_SEARCH_EVERY: Duration = Duration::from_secs(300);

const DEFAULT_DB_LIMIT: Duration = Duration::from_secs(30);

/// What happened to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Filed, and recorded as this call; how its start became an instant.
    Filed(i64, TimeHow),
    /// Its archive copy had been recorded as this call already.
    AlreadyFiled(i64),
    /// It cannot be filed, for this reason; it is deleted.
    Rejected(String),
    /// Its system, of this name, is unknown: it waits in the staging folder.
    Staged(String),
    /// A copy of this call from another recorder, no longer than the call's
    /// recording, or of a call a person worked on: deleted.
    Duplicate(i64),
    /// A longer copy of this call from another recorder: the call's
    /// recording now.
    Replaced(i64),
    /// Not filed this time: it stays waiting and is tried again after `retry`.
    Later { why: String, retry: Duration },
    /// The lease was lost; nothing more is written.
    LeaseLost,
}

fn later(why: String) -> Outcome {
    Outcome::Later {
        why,
        retry: RETRY_AFTER,
    }
}

pub struct Filer {
    pool: PgPool,
    fence: Arc<dyn Fence>,
    location: Location,
    zone: Tz,
    missing_grace: Duration,
    /// The limit on each database call.
    db_limit: Duration,
    /// Files waiting for a retry, by row id, and when they may be tried.
    retry_at: HashMap<i64, Instant>,
    /// The shortest call that is queued for transcription; below 0 while
    /// transcription is off.
    transcribe_from: Option<Arc<AtomicI32>>,
    read: Reader,
    folder_limit: Duration,
    /// The recorders' folders whose share stopped answering a read.
    hung: Hung,
    /// This host's name: the files left in a location other than the main
    /// archive are deleted only once this host found it usable.
    host: String,
    /// The last sweep took a full batch: see [`Filer::backlog`].
    backlog: bool,
}

/// How a recorder's file is read ([`reader::read_path`]).
pub type Reader = fn(&Path, Tz) -> Result<(FileCall, Vec<u8>), ReadError>;

impl Filer {
    pub fn new(
        pool: PgPool,
        fence: Arc<dyn Fence>,
        location: Location,
        zone: Tz,
        missing_grace: Duration,
    ) -> Filer {
        Filer {
            pool,
            fence,
            location,
            zone,
            missing_grace,
            db_limit: DEFAULT_DB_LIMIT,
            retry_at: HashMap::new(),
            transcribe_from: None,
            read: reader::read_path,
            folder_limit: watcher::FOLDER_LIMIT,
            hung: Hung::default(),
            host: String::new(),
            backlog: false,
        }
    }

    /// Sets this host's name, as its ingest records what it finds in each
    /// location.
    pub fn with_host(mut self, host: &str) -> Filer {
        self.host = host.to_string();
        self
    }

    /// Reads the recorders' files with `read` (default [`reader::read_path`]).
    pub fn with_reader(mut self, read: Reader) -> Filer {
        self.read = read;
        self
    }

    /// Sets how long a file's read may take (default
    /// [`watcher::FOLDER_LIMIT`]); its folder is then left alone until the
    /// read ends.
    pub fn with_folder_limit(mut self, limit: Duration) -> Filer {
        self.folder_limit = limit;
        self
    }

    /// Queues the filed calls for transcription by `rule`: the shortest call
    /// that is queued, in milliseconds, or below 0 while transcription is off.
    /// The talkgroup's own switch is read when the call is recorded.
    pub fn with_transcription(mut self, rule: Arc<AtomicI32>) -> Filer {
        self.transcribe_from = Some(rule);
        self
    }

    /// Sets the limit on each database call (default 30 s); a call that takes
    /// longer is a database error.
    pub fn with_db_limit(mut self, limit: Duration) -> Filer {
        self.db_limit = limit;
        self
    }

    /// Files up to `limit` waiting files, oldest first, and leaves out the
    /// files that wait for a retry. Stops at a lost lease. A database error
    /// ends the pass; the file it happened on stays waiting.
    pub async fn pass(&mut self, limit: i64) -> sqlx::Result<Vec<(Waiting, Outcome)>> {
        self.sweep(limit).await?;
        let now = Instant::now();
        self.retry_at.retain(|_, at| *at > now);
        let skip: Vec<i64> = self.retry_at.keys().copied().collect();
        let rows = limited(self.db_limit, store::waiting(&self.pool, limit, &skip)).await?;
        let mut done = Vec::with_capacity(rows.len());
        for row in rows {
            let outcome = self.file_one(&row).await?;
            let lost = outcome == Outcome::LeaseLost;
            done.push((row, outcome));
            if lost {
                break;
            }
        }
        Ok(done)
    }

    /// Files one waiting file.
    pub async fn file_one(&mut self, row: &Waiting) -> sqlx::Result<Outcome> {
        let outcome = self.try_file(row).await?;
        match &outcome {
            Outcome::Later { retry, .. } => {
                self.retry_at.insert(row.id, Instant::now() + *retry);
            }
            _ => {
                self.retry_at.remove(&row.id);
            }
        }
        Ok(outcome)
    }

    async fn try_file(&mut self, row: &Waiting) -> sqlx::Result<Outcome> {
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        if let (Some(id), Some(started_at)) = (row.call_id, row.call_started_at) {
            return self.dealt_with(row, (id, started_at)).await;
        }
        let (folder, path) = self.place_of(row);
        // In a thread of its own, as is the deletion: a recorder's share that
        // stopped answering holds up no other recorder's files (a read on
        // the runtime's worker would hold every task).
        let (read, zone, file) = (self.read, self.zone, path.clone());
        let job: watcher::Job<Result<(FileCall, Vec<u8>), ReadError>> =
            Box::new(move || Ok(read(&file, zone)));
        let answered = watcher::run_bounded(
            vec![(folder.clone(), job)],
            &mut self.hung,
            self.folder_limit,
        )
        .await
        .pop()
        .unwrap_or_else(|| Err(io::Error::other("the read was not run")));
        let read = match answered {
            Ok(read) => read,
            Err(error) => {
                return Ok(later(format!(
                    "{}: its folder {} {error}",
                    row.rel_path,
                    folder.display()
                )));
            }
        };
        let (call, data) = match read {
            Ok(read) => read,
            Err(ReadError::Rejected(why)) => {
                return self.reject_file(row, &folder, &path, why).await;
            }
            Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return self.missing(row).await;
            }
            Err(ReadError::Io(error)) => {
                return Ok(later(format!("cannot read {}: {error}", path.display())));
            }
        };
        if !row.staged
            && let Some(system) = call.described.system.as_deref()
            && !limited(self.db_limit, store::system_known(&self.pool, system)).await?
        {
            return self.stage(row, &folder, &path, &data, &call).await;
        }
        if let Some(same) = self.same_call(row, &call).await? {
            if same.replaced_by(call.duration_ms) {
                return self.replace(row, &folder, &path, &data, &call, same).await;
            }
            let why = if same.kept {
                format!(
                    "a copy of call {} from {}, which keeps its recording: a person worked on it",
                    same.id, same.recorder
                )
            } else {
                format!(
                    "a copy of call {} from {}: {} long, the call's {}",
                    same.id,
                    same.recorder,
                    seconds(call.duration_ms),
                    seconds(same.duration_ms)
                )
            };
            return self
                .remove_copy(row, &folder, &path, &data, &call, &same, &why)
                .await;
        }
        let name = file_name(&row.rel_path);
        let talkgroup = call
            .described
            .talkgroup
            .as_ref()
            .map(|tg| (tg.number, tg.name.as_str()));
        let rel =
            match archive::rel_path(call.local_start.date(), &archive::folder(talkgroup), name) {
                Ok(rel) => rel,
                Err(why) => return self.reject_file(row, &folder, &path, why).await,
            };
        let dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
        let digest = archive::sha256(&data);
        let writing = archive::WRITES.lock().await;
        let place = match archive::place(&self.location.root, dir, name, data.len() as u64, &digest)
        {
            Ok(place) => place,
            Err(error) => return Ok(later(format!("cannot look into the archive: {error}"))),
        };
        let rel = match place {
            // A crash after step 2 or 3 left the file in the archive.
            Place::Copy(rel) => {
                if let Some(found) = limited(
                    self.db_limit,
                    store::call_at(&self.pool, self.location.id, &rel, call.started_at),
                )
                .await?
                {
                    if !self.fence.held() {
                        return Ok(Outcome::LeaseLost);
                    }
                    limited(self.db_limit, store::mark_done(&self.pool, row.id, found)).await?;
                    if self.fence.held() {
                        self.delete_source(&folder, &path, &data, &call).await;
                    }
                    return Ok(Outcome::AlreadyFiled(found.0));
                }
                rel
            }
            Place::Free(rel) => {
                let target = self.location.root.join(&rel);
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost);
                }
                if let Err(error) = archive::write_part(&target, &data, &digest) {
                    return Ok(later(format!("cannot copy into the archive: {error}")));
                }
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost);
                }
                if let Err(error) = archive::finish_part(&target) {
                    return Ok(later(format!(
                        "cannot name the copy in the archive: {error}"
                    )));
                }
                rel
            }
        };
        let id = match self.record(row, &call, &rel).await? {
            Ok(id) => id,
            Err(outcome) => return Ok(outcome),
        };
        drop(writing);
        if self.fence.held() {
            self.delete_source(&folder, &path, &data, &call).await;
        }
        note_time(&row.rel_path, &call);
        Ok(Outcome::Filed(id, call.time_how))
    }

    /// Step 3. Err holds the outcome when the call was not recorded.
    async fn record(
        &self,
        row: &Waiting,
        call: &FileCall,
        rel: &str,
    ) -> sqlx::Result<Result<i64, Outcome>> {
        if let Err(error) = limited(
            self.db_limit,
            store::ensure_partition(&self.pool, call.started_at),
        )
        .await
        {
            // lock_not_available: another session held the tables for more than 10 s.
            if has_code(&error, "55P03") {
                return Ok(Err(later(format!(
                    "the month's partitions could not be created: {error}"
                ))));
            }
            return Err(error);
        }
        if !self.fence.held() {
            return Ok(Err(Outcome::LeaseLost));
        }
        let new = NewCall {
            call,
            source_id: row.source_id,
            location_id: self.location.id,
            rel_path: rel,
            transcribe_from_ms: self
                .transcribe_from
                .as_ref()
                .map(|rule| rule.load(Ordering::SeqCst))
                .filter(|from| *from >= 0),
        };
        let recorded = tokio::time::timeout(self.db_limit, async {
            let mut conn = self.pool.acquire().await?;
            store::record(&mut conn, self.fence.holder(), row.id, &new).await
        })
        .await
        .unwrap_or_else(|_| Err(RecordError::Database(store::timed_out(self.db_limit))));
        match recorded {
            Ok((id, _)) => Ok(Ok(id)),
            Err(RecordError::LeaseLost) => Ok(Err(Outcome::LeaseLost)),
            // unique_violation: a call has the path already; the next pass finds it.
            Err(RecordError::Database(error)) if has_code(&error, "23505") => Ok(Err(later(
                format!("a call has this archive path already: {error}"),
            ))),
            Err(RecordError::Database(error)) => Err(error),
        }
    }

    /// A file dealt with before, as this call, and seen again unchanged: a
    /// stop or a failed deletion left it in its recorder's folder. It is done
    /// again and deleted, without a read: its call may have another file or
    /// another recording since.
    async fn dealt_with(
        &mut self,
        row: &Waiting,
        call: (i64, DateTime<Utc>),
    ) -> sqlx::Result<Outcome> {
        let (folder, path) = self.place_of(row);
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        limited(self.db_limit, store::mark_done(&self.pool, row.id, call)).await?;
        if let (Some(size), Some(mtime), true) = (
            row.size.and_then(|n| u64::try_from(n).ok()),
            row.file_mtime,
            self.fence.held(),
        ) {
            match self.remove_bounded(&folder, &path, size, mtime).await {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => crate::log::warn(format!(
                    "{} was filed but could not be deleted: {error}",
                    path.display()
                )),
            }
        }
        Ok(Outcome::AlreadyFiled(call.0))
    }

    /// The call another recorder filed that the file's call is a copy of.
    async fn same_call(
        &self,
        row: &Waiting,
        call: &FileCall,
    ) -> sqlx::Result<Option<store::SameCall>> {
        let (Some(system), Some(talkgroup)) = (
            call.described.system.as_deref(),
            call.described.talkgroup.as_ref(),
        ) else {
            return Ok(None);
        };
        let Some(talkgroup) = limited(
            self.db_limit,
            store::talkgroup_of(&self.pool, system, talkgroup.number),
        )
        .await?
        else {
            return Ok(None);
        };
        limited(
            self.db_limit,
            store::same_call(&self.pool, talkgroup, call, row.source_id),
        )
        .await
    }

    /// A copy of a call another recorder filed that does not replace its
    /// recording: deleted from its recorder's folder, never written to the
    /// archive, then recorded as a duplicate of the call, for the reason
    /// `why`. Deleted first, as a rejected file is: a stop between the two
    /// leaves a row that ends as missing, never a file left behind.
    #[allow(clippy::too_many_arguments)]
    async fn remove_copy(
        &mut self,
        row: &Waiting,
        folder: &Path,
        path: &Path,
        data: &[u8],
        call: &FileCall,
        same: &store::SameCall,
        why: &str,
    ) -> sqlx::Result<Outcome> {
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        match self
            .remove_bounded(folder, path, data.len() as u64, call.file_mtime)
            .await
        {
            Ok(true) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Ok(false) => {
                return Ok(later(format!(
                    "{} changed since it was read; it is read again",
                    path.display()
                )));
            }
            Err(error) => {
                return Ok(later(format!(
                    "{} is a copy of call {} but could not be deleted: {error}",
                    path.display(),
                    same.id
                )));
            }
        }
        let recorded = tokio::time::timeout(self.db_limit, async {
            let mut conn = self.pool.acquire().await?;
            store::duplicate(
                &mut conn,
                self.fence.holder(),
                row.id,
                same,
                row.source_id,
                (data.len() as i64, call.file_mtime),
                why,
            )
            .await
        })
        .await
        .unwrap_or_else(|_| Err(RecordError::Database(store::timed_out(self.db_limit))));
        match recorded {
            Ok(()) => Ok(Outcome::Duplicate(same.id)),
            Err(RecordError::LeaseLost) => Ok(Outcome::LeaseLost),
            Err(RecordError::Database(error)) => Err(error),
        }
    }

    /// A longer copy of a call another recorder filed: the call's recording
    /// now. It is written into the archive under the call's start
    /// ([`archive::copy_name`]) in its talkgroup folder of the day, the call
    /// is pointed at it ([`store::replace`]), the old recording is deleted,
    /// and then the file. A call that changed meanwhile leaves the copy to
    /// be read again.
    async fn replace(
        &mut self,
        row: &Waiting,
        folder: &Path,
        path: &Path,
        data: &[u8],
        call: &FileCall,
        same: store::SameCall,
    ) -> sqlx::Result<Outcome> {
        let call_file = file_name(&same.rel_path);
        let (Some(name), Some(start)) = (
            archive::copy_name(call_file, file_name(&row.rel_path)),
            reader::start_time(call_file),
        ) else {
            let why = format!(
                "a copy of call {} from {}, whose file name gives no start to name a copy by",
                same.id, same.recorder
            );
            return self
                .remove_copy(row, folder, path, data, call, &same, &why)
                .await;
        };
        let talkgroup = call
            .described
            .talkgroup
            .as_ref()
            .map(|tg| (tg.number, tg.name.as_str()));
        let rel = match archive::rel_path(start.date(), &archive::folder(talkgroup), &name) {
            Ok(rel) => rel,
            Err(why) => return self.reject_file(row, folder, path, why).await,
        };
        let dir = rel.rsplit_once('/').map_or("", |(dir, _)| dir);
        let digest = archive::sha256(data);
        let writing = archive::WRITES.lock().await;
        let place =
            match archive::place(&self.location.root, dir, &name, data.len() as u64, &digest) {
                Ok(place) => place,
                Err(error) => return Ok(later(format!("cannot look into the archive: {error}"))),
            };
        let rel = match place {
            // Left by a stop before the transaction, unless a call has it.
            Place::Copy(rel) => {
                if limited(
                    self.db_limit,
                    store::named(&self.pool, self.location.id, &rel),
                )
                .await?
                {
                    return Ok(later(format!("a call has {rel} already")));
                }
                rel
            }
            Place::Free(rel) => {
                let target = self.location.root.join(&rel);
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost);
                }
                if let Err(error) = archive::write_part(&target, data, &digest) {
                    return Ok(later(format!("cannot copy into the archive: {error}")));
                }
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost);
                }
                if let Err(error) = archive::finish_part(&target) {
                    return Ok(later(format!(
                        "cannot name the copy in the archive: {error}"
                    )));
                }
                rel
            }
        };
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        let new = NewCall {
            call,
            source_id: row.source_id,
            location_id: self.location.id,
            rel_path: &rel,
            transcribe_from_ms: self
                .transcribe_from
                .as_ref()
                .map(|rule| rule.load(Ordering::SeqCst))
                .filter(|from| *from >= 0),
        };
        let replaced = tokio::time::timeout(self.db_limit, async {
            let mut conn = self.pool.acquire().await?;
            store::replace(
                &mut conn,
                self.fence.holder(),
                row.id,
                &same,
                &new,
                data.len() as i64,
            )
            .await
        })
        .await
        .unwrap_or_else(|_| Err(RecordError::Database(store::timed_out(self.db_limit))));
        let why = match replaced {
            Ok(true) => None,
            Ok(false) => Some(format!(
                "call {} changed while its longer copy was filed; it is read again",
                same.id
            )),
            // lock_not_available: a job's step held the call or its talkgroup.
            Err(RecordError::Database(error)) if has_code(&error, "55P03") => Some(format!(
                "call {} was held by a job while its longer copy was filed; it is read again",
                same.id
            )),
            Err(RecordError::LeaseLost) => return Ok(Outcome::LeaseLost),
            Err(RecordError::Database(error)) if has_code(&error, "23505") => {
                return Ok(later(format!(
                    "a call has this archive path already: {error}"
                )));
            }
            Err(RecordError::Database(error)) => return Err(error),
        };
        if let Some(why) = why {
            // No call has the copy: it goes, and the file is read again.
            if let Err(error) = fs::remove_file(self.location.root.join(&rel)) {
                crate::log::warn(format!("cannot remove {rel}: {error}"));
            }
            return Ok(later(why));
        }
        // The filer deletes in its own location at once: an old recording in
        // another stays listed as a leftover, for the sweep.
        if same.location_id == self.location.id {
            self.remove_leftover(&store::Leftover {
                location_id: self.location.id,
                root: self.location.root.display().to_string(),
                rel_path: same.rel_path.clone(),
                managed: true,
                marked: false,
            })
            .await?;
        }
        drop(writing);
        if self.fence.held() {
            self.delete_source(folder, path, data, call).await;
        }
        Ok(Outcome::Replaced(same.id))
    }

    /// Deletes up to `limit` archive files no call names any more, in every
    /// location this host can use: a recording a longer copy replaced, which
    /// a stop left behind, or a moved call's old file.
    async fn sweep(&mut self, limit: i64) -> sqlx::Result<()> {
        self.backlog = false;
        let leftovers = limited(
            self.db_limit,
            store::leftovers(&self.pool, &self.host, limit),
        )
        .await?;
        if leftovers.is_empty() || !self.fence.held() {
            return Ok(());
        }
        let taken = leftovers.len();
        let mut forgotten = 0;
        let _writing = archive::WRITES.lock().await;
        // Another location's files are deleted in a thread of their own per
        // location, for a limit: a share that stops answering holds up
        // neither the filer nor the archive's writes, and is left alone until
        // its thread ends. Its own check shows it on its card and on Status.
        let mut elsewhere: Vec<(PathBuf, Vec<store::Leftover>)> = Vec::new();
        for left in leftovers {
            if !self.fence.held() {
                break;
            }
            if !(left.marked && left.managed) {
                if self.remove_leftover(&left).await? {
                    forgotten += 1;
                }
            } else if self.named(&left).await? {
                self.forget(&left).await?;
                forgotten += 1;
            } else {
                let root = PathBuf::from(&left.root);
                match elsewhere.iter_mut().find(|(at, _)| *at == root) {
                    Some((_, lefts)) => lefts.push(left),
                    None => elsewhere.push((root, vec![left])),
                }
            }
        }
        let jobs = elsewhere
            .iter()
            .map(|(root, lefts)| {
                let (at, rels): (PathBuf, Vec<String>) = (
                    root.clone(),
                    lefts.iter().map(|left| left.rel_path.clone()).collect(),
                );
                let job: watcher::Job<Vec<io::Result<()>>> = Box::new(move || {
                    if !at.join(crate::storage::MARKER).is_file() {
                        // Unmounted since its last check: its files are out
                        // of reach, not gone, and stay listed.
                        return Err(io::Error::other("not mounted"));
                    }
                    Ok(rels.iter().map(|rel| remove_left(&at.join(rel))).collect())
                });
                (root.clone(), job)
            })
            .collect();
        let results = watcher::run_bounded(jobs, &mut self.hung, self.folder_limit).await;
        for ((_, lefts), result) in elsewhere.iter().zip(results) {
            let Ok(removed) = result else { continue };
            for (left, removed) in lefts.iter().zip(removed) {
                match removed {
                    Ok(()) => {
                        self.forget(left).await?;
                        forgotten += 1;
                    }
                    // It stays listed, and the next pass tries again.
                    Err(error) => crate::log::warn(format!(
                        "cannot remove {}: {error}",
                        Path::new(&left.root).join(&left.rel_path).display()
                    )),
                }
            }
        }
        // A full batch that moved on: more may wait, and the next pass comes
        // at once. Leftovers that cannot go yet do not keep it from resting.
        self.backlog = taken as i64 == limit && forgotten > 0;
        Ok(())
    }

    /// Whether the last pass swept a full batch of leftovers: more may be
    /// waiting (a purge's files), so the next pass need not wait.
    pub fn backlog(&self) -> bool {
        self.backlog
    }

    /// Whether a call names the leftover's file again.
    async fn named(&self, left: &store::Leftover) -> sqlx::Result<bool> {
        limited(
            self.db_limit,
            store::named(&self.pool, left.location_id, &left.rel_path),
        )
        .await
    }

    async fn forget(&self, left: &store::Leftover) -> sqlx::Result<()> {
        limited(
            self.db_limit,
            store::forget_leftover(&self.pool, left.location_id, &left.rel_path),
        )
        .await
    }

    /// Deletes an archive file that no call names any more, then its folder
    /// if that is empty, and forgets it. A call that names it again keeps it;
    /// a read-only location's file stays. Called with the archive's writes
    /// held, so that no writer takes the file between the check and the
    /// deletion. Returns whether it was forgotten.
    async fn remove_leftover(&self, left: &store::Leftover) -> sqlx::Result<bool> {
        if left.managed && !self.named(left).await? {
            let path = Path::new(&left.root).join(&left.rel_path);
            if let Err(error) = remove_left(&path) {
                // It stays listed, and the next pass tries again.
                crate::log::warn(format!("cannot remove {}: {error}", path.display()));
                return Ok(false);
            }
        }
        self.forget(left).await?;
        Ok(true)
    }

    /// The folder and path of a waiting file: in its recorder's folder, or in
    /// the archive's staging folder once staged.
    fn place_of(&self, row: &Waiting) -> (PathBuf, PathBuf) {
        if row.staged {
            let path = archive::staging_path(&self.location.root, row.source_id, &row.rel_path);
            (self.location.root.join(archive::STAGING), path)
        } else {
            let folder = PathBuf::from(&row.source_path);
            let path = folder.join(&row.rel_path);
            (folder, path)
        }
    }

    /// A file of a system no one has named yet: copied into the archive's
    /// staging folder and checked, with its modification time kept, then
    /// recorded as staged, then deleted from its recorder's folder. It waits
    /// there until the administrator names its system.
    async fn stage(
        &mut self,
        row: &Waiting,
        folder: &Path,
        path: &Path,
        data: &[u8],
        call: &FileCall,
    ) -> sqlx::Result<Outcome> {
        let system = call.described.system.clone().unwrap_or_default();
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        let target = archive::staging_path(&self.location.root, row.source_id, &row.rel_path);
        let digest = archive::sha256(data);
        let copied = archive::write_part(&target, data, &digest)
            .and_then(|()| archive::replace_with_part(&target))
            .and_then(|()| {
                let mtime: std::time::SystemTime = call.file_mtime.into();
                fs::File::options()
                    .write(true)
                    .open(&target)
                    .and_then(|file| file.set_modified(mtime))
            });
        if let Err(error) = copied {
            return Ok(later(format!(
                "cannot copy it into the staging folder: {error}"
            )));
        }
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        let talkgroup = call
            .described
            .talkgroup
            .as_ref()
            .map(|tg| format!("{} {}", tg.number, tg.name));
        limited(
            self.db_limit,
            store::stage(&self.pool, row.id, &system, talkgroup.as_deref()),
        )
        .await?;
        if self.fence.held() {
            self.delete_source(folder, path, data, call).await;
        }
        Ok(Outcome::Staged(system))
    }

    /// A file that cannot be filed: deleted from its recorder's folder, since
    /// nothing may stay there, then recorded as rejected with the reason. A
    /// file that changed since the watcher saw it stays, to be read again.
    /// Deleted first: a stop between the two leaves a row that ends as
    /// missing, never a file left behind.
    async fn reject_file(
        &mut self,
        row: &Waiting,
        folder: &Path,
        path: &Path,
        why: String,
    ) -> sqlx::Result<Outcome> {
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        let unchanged = match (row.size.and_then(|n| u64::try_from(n).ok()), row.file_mtime) {
            (Some(size), Some(mtime)) => self.remove_bounded(folder, path, size, mtime).await,
            _ => Ok(false),
        };
        match unchanged {
            Ok(true) => {}
            Ok(false) => {
                return Ok(later(format!(
                    "{} changed since it was seen; it is read again",
                    path.display()
                )));
            }
            Err(error) => crate::log::warn(format!(
                "{} is rejected ({why}) but could not be deleted: {error}",
                path.display()
            )),
        }
        self.reject(row, why).await
    }

    async fn reject(&self, row: &Waiting, why: String) -> sqlx::Result<Outcome> {
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost);
        }
        limited(self.db_limit, store::reject(&self.pool, row.id, &why)).await?;
        Ok(Outcome::Rejected(why))
    }

    /// A waiting file that is gone from its source. The same file filed
    /// before is done again at once with its call. Otherwise the other host may have
    /// filed it before it lost the lease, so after the grace the archive is
    /// searched for a copy of the same name and size, in the folders of the
    /// name's date and the day on each side.
    async fn missing(&mut self, row: &Waiting) -> sqlx::Result<Outcome> {
        // Filed before, then seen again by a rescan just before step 4
        // deleted it: done again with its call, without a search. Another
        // recording under the same name takes the usual path.
        if let (true, Some(id), Some(started_at)) =
            (row.same_file, row.call_id, row.call_started_at)
        {
            if !self.fence.held() {
                return Ok(Outcome::LeaseLost);
            }
            limited(
                self.db_limit,
                store::mark_done(&self.pool, row.id, (id, started_at)),
            )
            .await?;
            return Ok(Outcome::AlreadyFiled(id));
        }
        let gone = (Utc::now() - row.seen_at).to_std().unwrap_or_default();
        if gone < self.missing_grace {
            return Ok(Outcome::Later {
                why: format!("{MISSING} from its source; waiting for the grace to end"),
                retry: (self.missing_grace - gone).min(RETRY_AFTER),
            });
        }
        let name = file_name(&row.rel_path);
        let (Some(start), Some(size)) = (
            reader::start_time(name),
            row.size.and_then(|size| u64::try_from(size).ok()),
        ) else {
            return self.reject(row, MISSING.to_string()).await;
        };
        let copies = match find_copies(&self.location.root, start.date(), name, size) {
            Ok(copies) => copies,
            Err(error) => {
                return Ok(Outcome::Later {
                    why: format!("cannot search the archive: {error}"),
                    retry: MISSING_SEARCH_EVERY,
                });
            }
        };
        let path = match copies.as_slice() {
            [] => return self.reject(row, MISSING.to_string()).await,
            [path] => path,
            more => {
                let why = format!(
                    "{MISSING}: {} files of the same name and size in the archive",
                    more.len()
                );
                return self.reject(row, why).await;
            }
        };
        let Some(rel) = path
            .strip_prefix(&self.location.root)
            .ok()
            .and_then(|rel| rel.to_str())
        else {
            return self.reject(row, MISSING.to_string()).await;
        };
        let call = match reader::read_path(path, self.zone) {
            Ok((call, _)) => call,
            Err(error) => {
                return Ok(Outcome::Later {
                    why: format!("cannot read its archive copy {rel}: {error}"),
                    retry: MISSING_SEARCH_EVERY,
                });
            }
        };
        if let Some(found) = limited(
            self.db_limit,
            store::call_at(&self.pool, self.location.id, rel, call.started_at),
        )
        .await?
        {
            if !self.fence.held() {
                return Ok(Outcome::LeaseLost);
            }
            limited(self.db_limit, store::mark_done(&self.pool, row.id, found)).await?;
            return Ok(Outcome::AlreadyFiled(found.0));
        }
        Ok(match self.record(row, &call, rel).await? {
            Ok(id) => {
                note_time(&row.rel_path, &call);
                Outcome::Filed(id, call.time_how)
            }
            Err(outcome) => outcome,
        })
    }

    /// Deletes a recorder's file if it is unchanged, in a thread of its own,
    /// as the file was read ([`watcher::run_bounded`]): true when deleted,
    /// false when it changed.
    async fn remove_bounded(
        &mut self,
        folder: &Path,
        path: &Path,
        size: u64,
        mtime: DateTime<Utc>,
    ) -> io::Result<bool> {
        let file = path.to_path_buf();
        let job: watcher::Job<bool> = Box::new(move || remove_if_unchanged(&file, size, mtime));
        watcher::run_bounded(
            vec![(folder.to_path_buf(), job)],
            &mut self.hung,
            self.folder_limit,
        )
        .await
        .pop()
        .unwrap_or_else(|| Err(io::Error::other("the deletion was not run")))
    }

    /// Step 4. Leaves the file when it changed since it was read: it is then
    /// read again as a new file. A folder that does not answer keeps it: a
    /// later rescan finds it filed.
    async fn delete_source(&mut self, folder: &Path, path: &Path, data: &[u8], call: &FileCall) {
        match self
            .remove_bounded(folder, path, data.len() as u64, call.file_mtime)
            .await
        {
            Ok(true) => {}
            Ok(false) => crate::log::warn(format!(
                "{} changed after it was read; it stays in its source",
                path.display()
            )),
            Err(error) => crate::log::warn(format!(
                "{} was filed but could not be deleted: {error}",
                path.display()
            )),
        }
    }
}

/// One line for a call whose start was shifted or guessed.
fn note_time(rel_path: &str, call: &FileCall) {
    let how = match call.time_how {
        TimeHow::Gap => "lies in the spring gap; it is shifted forward to",
        TimeHow::EarlierDefault => "falls in the repeated hour; the earlier instant is guessed,",
        TimeHow::Plain | TimeHow::ByMtime => return,
    };
    crate::log::info(format!(
        "{rel_path}: its local time {} {how} {}",
        call.local_start, call.started_at
    ));
}

/// Deletes the file when its size and modification time are still `size` and
/// `mtime`, to the microsecond the database keeps. False when it changed, and
/// then it stays.
pub fn remove_if_unchanged(path: &Path, size: u64, mtime: DateTime<Utc>) -> io::Result<bool> {
    let meta = fs::metadata(path)?;
    let modified = DateTime::<Utc>::from(meta.modified()?);
    if meta.len() != size || modified.timestamp_micros() != mtime.timestamp_micros() {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

/// The files named `name` with `size` bytes in the talkgroup folders of `day`
/// and of the day on each side.
pub fn find_copies(root: &Path, day: NaiveDate, name: &str, size: u64) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for offset in [-1, 0, 1] {
        let date = day + TimeDelta::days(offset);
        let folders = match fs::read_dir(root.join(date.format("%Y-%m-%d").to_string())) {
            Ok(folders) => folders,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for folder in folders {
            let path = folder?.path().join(name);
            match fs::metadata(&path) {
                Ok(meta) if meta.is_file() && meta.len() == size => found.push(path),
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(found)
}

/// A length in seconds, to the tenth: `4.2 s`.
fn seconds(ms: i32) -> String {
    format!("{:.1} s", f64::from(ms) / 1000.0)
}

/// The last part of a path inside a source.
fn file_name(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

fn has_code(error: &sqlx::Error, code: &str) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some(code))
}

/// Deletes a file no call names, then its folder if that is empty. A file
/// already gone is deleted too.
fn remove_left(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => {
            if let Some(dir) = path.parent() {
                // Only an empty folder goes.
                let _ = fs::remove_dir(dir);
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
