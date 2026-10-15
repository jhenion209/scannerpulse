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
use crate::store::{self, Location, MISSING, NewCall, RecordError, Source, Waiting, limited};

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
    /// It cannot be filed, for this reason; it stays where it is.
    Rejected(String),
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
    sources: HashMap<i16, Source>,
    zone: Tz,
    missing_grace: Duration,
    /// The limit on each database call.
    db_limit: Duration,
    /// Files waiting for a retry, by row id, and when they may be tried.
    retry_at: HashMap<i64, Instant>,
    /// The shortest call that is queued for transcription; below 0 while
    /// transcription is off.
    transcribe_from: Option<Arc<AtomicI32>>,
}

impl Filer {
    pub fn new(
        pool: PgPool,
        fence: Arc<dyn Fence>,
        location: Location,
        sources: Vec<Source>,
        zone: Tz,
        missing_grace: Duration,
    ) -> Filer {
        Filer {
            pool,
            fence,
            location,
            sources: sources.into_iter().map(|s| (s.id, s)).collect(),
            zone,
            missing_grace,
            db_limit: DEFAULT_DB_LIMIT,
            retry_at: HashMap::new(),
            transcribe_from: None,
        }
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
        let Some(source) = self.sources.get(&row.source_id) else {
            return Ok(later(format!("source {} is not enabled", row.source_id)));
        };
        let path = source.path.join(&row.rel_path);
        let (call, data) = match reader::read_path(&path, self.zone) {
            Ok(read) => read,
            Err(ReadError::Rejected(why)) => return self.reject(row, why).await,
            Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                return self.missing(row).await;
            }
            Err(ReadError::Io(error)) => {
                return Ok(later(format!("cannot read {}: {error}", path.display())));
            }
        };
        let name = file_name(&row.rel_path);
        let talkgroup = call
            .described
            .talkgroup
            .as_ref()
            .map(|tg| (tg.number, tg.name.as_str()));
        let rel =
            match archive::rel_path(call.local_start.date(), &archive::folder(talkgroup), name) {
                Ok(rel) => rel,
                Err(why) => return self.reject(row, why).await,
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
                        self.delete_source(&path, &data, &call);
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
        drop(writing);
        let id = match self.record(row, &call, &rel).await? {
            Ok(id) => id,
            Err(outcome) => return Ok(outcome),
        };
        if self.fence.held() {
            self.delete_source(&path, &data, &call);
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

    /// Step 4. Leaves the file when it changed since it was read: it is then
    /// read again as a new file.
    fn delete_source(&self, path: &Path, data: &[u8], call: &FileCall) {
        match remove_if_unchanged(path, data.len() as u64, call.file_mtime) {
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
/// `mtime`. False when it changed, and then it stays.
pub fn remove_if_unchanged(path: &Path, size: u64, mtime: DateTime<Utc>) -> io::Result<bool> {
    let meta = fs::metadata(path)?;
    if meta.len() != size || DateTime::<Utc>::from(meta.modified()?) != mtime {
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

/// The last part of a path inside a source.
fn file_name(rel_path: &str) -> &str {
    rel_path.rsplit('/').next().unwrap_or(rel_path)
}

fn has_code(error: &sqlx::Error, code: &str) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some(code))
}
