//! The watcher, on every host: finds the mp3 files in the watched recorders'
//! folders and records each one in `ingest_files` once it has stopped
//! changing. It acts on file system events where they are delivered, and
//! lists every folder again every `ingest.rescan_seconds`, because a network
//! share does not always deliver events. A recorder taken in when asked is
//! only counted, until it is asked: then the files it held at that moment are
//! recorded once.
//!
//! Ingest lists the folders and reads the files' sizes in threads of their
//! own, with [`list_all`] and [`stat_all`], so that a recorder's share that
//! stops answering holds only its own thread, never the other recorders.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crate::store::{Seen, Source};

/// A file is taken when its size and modification time have not changed for this long.
pub const STABLE_FOR: Duration = Duration::from_secs(2);

/// How long a folder's listing, or the reading of its new files' sizes, may
/// take in one round. A folder that takes longer is left alone until its
/// thread ends.
pub const FOLDER_LIMIT: Duration = Duration::from_secs(20);

/// An audio file a listing found: its path inside the folder, its size and
/// its modification time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub rel_path: String,
    pub size: u64,
    pub mtime: SystemTime,
}

/// A folder's audio files, and the names in it that are not UTF-8.
#[derive(Debug, Default)]
pub struct Listing {
    pub found: Vec<Found>,
    pub bad_names: Vec<PathBuf>,
}

/// How a folder is listed ([`list_folder`]).
pub type Lister = fn(&Path) -> io::Result<Listing>;

/// How a file's size and modification time are read ([`stat_file`]).
pub type Stat = fn(&Path) -> io::Result<(u64, SystemTime)>;

/// A file as the watcher knows it: its source and its path inside it.
pub type Key = (i16, String);

/// A sighting to look at again, the folder it is in, and its path.
pub type Due = (Key, PathBuf, PathBuf);

/// The sizes and modification times read for the files due.
pub type Stats = Vec<(Key, io::Result<(u64, SystemTime)>)>;

/// Work for a thread of its own ([`run_bounded`]).
pub type Job<T> = Box<dyn FnOnce() -> io::Result<T> + Send>;

/// Files to record for one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batch {
    pub source_id: i16,
    pub files: Vec<Seen>,
}

#[derive(Debug, Clone, Copy)]
struct Sighting {
    size: u64,
    mtime: SystemTime,
    /// When the size or modification time last changed.
    since: Instant,
    /// Written to the database since it last changed.
    recorded: bool,
}

pub struct Watcher {
    sources: Vec<Source>,
    stable_for: Duration,
    files: HashMap<(i16, String), Sighting>,
    /// Names that are not UTF-8, reported once each.
    reported: HashSet<PathBuf>,
    /// What the last rescan found in each source: its audio files, or why it
    /// could not be listed.
    listings: Vec<(i16, Result<usize, String>)>,
}

impl Watcher {
    pub fn new(sources: Vec<Source>, stable_for: Duration) -> Watcher {
        Watcher {
            sources,
            stable_for,
            files: HashMap::new(),
            reported: HashSet::new(),
            listings: Vec::new(),
        }
    }

    /// Watches `sources` from now on, forgetting the files of any source
    /// gone or moved. True when the folders to watch changed, so that the
    /// file system watch must be made again.
    pub fn set_sources(&mut self, sources: Vec<Source>) -> bool {
        let folders = |list: &[Source]| -> Vec<(i16, PathBuf)> {
            list.iter().map(|s| (s.id, s.path.clone())).collect()
        };
        let (before, after) = (folders(&self.sources), folders(&sources));
        // A file is known by its source and its path inside it: a source
        // whose folder moved starts afresh.
        let kept: HashSet<i16> = before
            .iter()
            .filter(|folder| after.contains(folder))
            .map(|(id, _)| *id)
            .collect();
        self.files.retain(|key, _| kept.contains(&key.0));
        self.listings.retain(|(id, _)| kept.contains(id));
        self.sources = sources;
        before != after
    }

    /// What the last rescan found in each watched source: its audio files, or
    /// why it could not be listed.
    pub fn listings(&self) -> &[(i16, Result<usize, String>)] {
        &self.listings
    }

    /// The source paths, for the file system watch.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.sources.iter().map(|source| source.path.as_path())
    }

    /// The watched sources and their folders, to list.
    pub fn folders(&self) -> Vec<(i16, PathBuf)> {
        self.sources
            .iter()
            .map(|s| (s.id, s.path.clone()))
            .collect()
    }

    /// Reports each name that is not UTF-8 once: it cannot be stored, so
    /// ingest leaves it alone.
    pub fn report(&mut self, bad_names: &[PathBuf]) {
        for path in bad_names {
            if self.reported.insert(path.clone()) {
                crate::log::warn(format!(
                    "{} is not a UTF-8 name; ingest leaves it alone",
                    path.display()
                ));
            }
        }
    }

    /// A path from a file system event: looked at now.
    pub fn touch(&mut self, path: &Path, now: Instant) {
        let Some((source_id, rel_path)) = self.inside(path) else {
            return;
        };
        // The rules of `list`: an mp3, nothing hidden on the way, no link.
        if !is_audio(path) || rel_path.split('/').any(|part| part.starts_with('.')) {
            return;
        }
        // An event comes from a folder that answers: read here, not in a thread.
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => self.observe(
                source_id,
                rel_path,
                meta.len(),
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                now,
            ),
            _ => {
                self.files.remove(&(source_id, rel_path));
            }
        }
    }

    /// Lists every source again, here, and looks at the files due. Returns
    /// the files that have settled and every file recorded before and still
    /// there. Ingest does the same through [`list_all`], [`Watcher::rescan_with`],
    /// [`stat_all`] and [`Watcher::settle_with`].
    pub fn rescan(&mut self, now: Instant) -> Vec<Batch> {
        let listed = self
            .folders()
            .into_iter()
            .map(|(id, folder)| (id, list_folder(&folder)))
            .collect();
        let mut batches = self.rescan_with(now, listed);
        batches.extend(self.settle(now));
        batches
    }

    /// Takes the listings of the watched sources. A source missing from
    /// `listed`, or whose listing failed, refreshes nothing. Returns every
    /// file recorded before and still there, so that their rows show they are
    /// still in the source; the files that settled come from
    /// [`Watcher::settle_with`].
    pub fn rescan_with(
        &mut self,
        now: Instant,
        listed: Vec<(i16, io::Result<Listing>)>,
    ) -> Vec<Batch> {
        let mut still_there = HashSet::new();
        let mut answered = HashSet::new();
        self.listings.clear();
        for (id, result) in listed {
            let Some(source) = self.sources.iter().find(|s| s.id == id).cloned() else {
                continue;
            };
            let listing = match result {
                Ok(listing) => listing,
                Err(error) => {
                    crate::log::warn(format!(
                        "cannot list the source {}: {error}",
                        source.path.display()
                    ));
                    self.listings
                        .push((source.id, Err(format!("cannot be listed: {error}"))));
                    continue;
                }
            };
            self.report(&listing.bad_names);
            self.listings.push((source.id, Ok(listing.found.len())));
            answered.insert(source.id);
            for found in listing.found {
                still_there.insert((source.id, found.rel_path.clone()));
                self.observe(source.id, found.rel_path, found.size, found.mtime, now);
            }
        }
        // A file gone from a source that answered is forgotten.
        self.files
            .retain(|key, _| !answered.contains(&key.0) || still_there.contains(key));
        let mut batches = Vec::new();
        for (key, sighting) in &self.files {
            if sighting.recorded && answered.contains(&key.0) {
                push(&mut batches, key, sighting);
            }
        }
        batches
    }

    /// The files that have stopped changing and are not recorded yet, looked
    /// at again here. Ingest does the same through [`Watcher::due`],
    /// [`stat_all`] and [`Watcher::settle_with`].
    pub fn settle(&mut self, now: Instant) -> Vec<Batch> {
        let stats = self
            .due(now)
            .into_iter()
            .map(|(key, _, path)| (key, stat_file(&path)))
            .collect();
        self.settle_with(now, stats)
    }

    /// The files that have not changed for `stable_for` and are not recorded
    /// yet: each to look at again, with its folder.
    pub fn due(&self, now: Instant) -> Vec<Due> {
        self.files
            .iter()
            .filter(|(_, s)| !s.recorded && now.duration_since(s.since) >= self.stable_for)
            .filter_map(|(key, _)| {
                let source = self.sources.iter().find(|source| source.id == key.0)?;
                Some((key.clone(), source.path.clone(), source.path.join(&key.1)))
            })
            .collect()
    }

    /// Takes the sizes and modification times of the files due. A file that
    /// did not change has settled; one that changed starts its wait again;
    /// one that is gone is forgotten. A file missing from `stats` (its folder
    /// did not answer) waits.
    pub fn settle_with(&mut self, now: Instant, stats: Stats) -> Vec<Batch> {
        let mut batches = Vec::new();
        for (key, stat) in stats {
            let Ok((size, mtime)) = stat else {
                self.files.remove(&key);
                continue;
            };
            let unchanged = self
                .files
                .get(&key)
                .is_some_and(|s| s.size == size && s.mtime == mtime);
            if unchanged {
                push(&mut batches, &key, &self.files[&key]);
            } else if self.files.contains_key(&key) {
                self.observe(key.0, key.1, size, mtime, now);
            }
        }
        batches
    }

    /// The batches were written: their files need no writing until they change
    /// or the next rescan.
    pub fn recorded(&mut self, batches: &[Batch]) {
        for batch in batches {
            for file in &batch.files {
                if let Some(s) = self
                    .files
                    .get_mut(&(batch.source_id, file.rel_path.clone()))
                {
                    s.recorded = true;
                }
            }
        }
    }

    fn observe(
        &mut self,
        source_id: i16,
        rel_path: String,
        size: u64,
        mtime: SystemTime,
        now: Instant,
    ) {
        let sighting = self.files.entry((source_id, rel_path)).or_insert(Sighting {
            size,
            mtime,
            since: now,
            recorded: false,
        });
        if sighting.size != size || sighting.mtime != mtime {
            *sighting = Sighting {
                size,
                mtime,
                since: now,
                recorded: false,
            };
        }
    }

    /// The source and the path inside it of a path from an event.
    fn inside(&self, path: &Path) -> Option<(i16, String)> {
        self.sources.iter().find_map(|source| {
            let rel = path.strip_prefix(&source.path).ok()?;
            Some((source.id, rel.to_str()?.to_string()))
        })
    }
}

/// The files to bring in from a recorder taken in when asked: those its
/// folder held when it was asked (`asked`). A file written since waits for
/// the next request.
pub fn to_bring_in(found: &[Found], asked: SystemTime) -> Vec<Seen> {
    found
        .iter()
        .filter(|file| file.mtime <= asked)
        .map(|file| Seen {
            rel_path: file.rel_path.clone(),
            size: i64::try_from(file.size).unwrap_or(i64::MAX),
            file_mtime: file.mtime.into(),
        })
        .collect()
}

/// The folders whose listing or reading outlived its limit. Their thread is
/// left to finish (a read on a hung share cannot be stopped), and nothing
/// more is asked of the folder until it has.
#[derive(Debug, Default)]
pub struct Hung(HashMap<PathBuf, Arc<AtomicBool>>);

impl Hung {
    /// Whether a thread of an earlier round still waits on `folder`.
    pub fn holds(&mut self, folder: &Path) -> bool {
        self.0.retain(|_, done| !done.load(Ordering::SeqCst));
        self.0.contains_key(folder)
    }

    fn left(&mut self, folder: PathBuf, done: Arc<AtomicBool>) {
        self.0.insert(folder, done);
    }
}

fn did_not_answer(limit: Duration) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "did not answer within {} s; it is left alone until it does",
            limit.as_secs_f32()
        ),
    )
}

fn still_hung() -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        "still does not answer: an earlier read of it has not ended",
    )
}

/// Runs each job in a thread of its own, all at once, for `limit` at most,
/// and returns their results in order. A job whose folder is still hung from
/// an earlier round is not run; a thread that outlives the limit is left in
/// `hung` under its folder.
pub async fn run_bounded<T: Send + 'static>(
    jobs: Vec<(PathBuf, Job<T>)>,
    hung: &mut Hung,
    limit: Duration,
) -> Vec<io::Result<T>> {
    let started = tokio::time::Instant::now();
    let mut running = Vec::new();
    for (folder, work) in jobs {
        if hung.holds(&folder) {
            running.push((folder, None));
            continue;
        }
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let task = tokio::task::spawn_blocking(move || {
            let result = work();
            flag.store(true, Ordering::SeqCst);
            result
        });
        running.push((folder, Some((done, task))));
    }
    let mut results = Vec::new();
    for (folder, task) in running {
        let Some((done, task)) = task else {
            results.push(Err(still_hung()));
            continue;
        };
        results.push(match tokio::time::timeout_at(started + limit, task).await {
            Ok(Ok(result)) => result,
            Ok(Err(join)) => Err(io::Error::other(join.to_string())),
            Err(_) => {
                hung.left(folder, done);
                Err(did_not_answer(limit))
            }
        });
    }
    results
}

/// Lists each folder in a thread of its own ([`run_bounded`]). A folder
/// still hung from an earlier round is not listed.
pub async fn list_all(
    folders: Vec<(i16, PathBuf)>,
    lister: Lister,
    hung: &mut Hung,
    limit: Duration,
) -> Vec<(i16, io::Result<Listing>)> {
    let ids: Vec<i16> = folders.iter().map(|(id, _)| *id).collect();
    let jobs = folders
        .into_iter()
        .map(|(_, folder)| {
            let path = folder.clone();
            let work: Job<Listing> = Box::new(move || lister(&path));
            (folder, work)
        })
        .collect();
    ids.into_iter()
        .zip(run_bounded(jobs, hung, limit).await)
        .collect()
}

/// Reads the sizes and modification times of the files due, a thread per
/// folder ([`run_bounded`]). The files of a folder that did not answer are
/// left out: they wait.
pub async fn stat_all(due: Vec<Due>, stat: Stat, hung: &mut Hung, limit: Duration) -> Stats {
    type Files = Vec<(Key, PathBuf)>;
    let mut by_folder: HashMap<PathBuf, Files> = HashMap::new();
    for (key, folder, path) in due {
        by_folder.entry(folder).or_default().push((key, path));
    }
    let groups: Vec<(PathBuf, Files)> = by_folder.into_iter().collect();
    let folders: Vec<PathBuf> = groups.iter().map(|(folder, _)| folder.clone()).collect();
    let jobs = groups
        .into_iter()
        .map(|(folder, files)| {
            let work: Job<Stats> = Box::new(move || {
                Ok(files
                    .into_iter()
                    .map(|(key, path)| (key, stat(&path)))
                    .collect())
            });
            (folder, work)
        })
        .collect();
    let mut stats = Vec::new();
    for (folder, read) in folders.iter().zip(run_bounded(jobs, hung, limit).await) {
        match read {
            Ok(read) => stats.extend(read),
            Err(error) => crate::log::warn(format!(
                "the new files of {} wait: the folder {error}",
                folder.display()
            )),
        }
    }
    stats
}

/// A file's size and modification time.
pub fn stat_file(path: &Path) -> io::Result<(u64, SystemTime)> {
    let meta = fs::metadata(path)?;
    Ok((meta.len(), meta.modified()?))
}

fn push(batches: &mut Vec<Batch>, key: &(i16, String), sighting: &Sighting) {
    let seen = Seen {
        rel_path: key.1.clone(),
        size: i64::try_from(sighting.size).unwrap_or(i64::MAX),
        file_mtime: sighting.mtime.into(),
    };
    match batches.iter_mut().find(|batch| batch.source_id == key.0) {
        Some(batch) => batch.files.push(seen),
        None => batches.push(Batch {
            source_id: key.0,
            files: vec![seen],
        }),
    }
}

/// An mp3 file by its name: `.mp3` in any case, not hidden.
fn is_audio(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    !name.starts_with('.')
        && name
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("mp3"))
}

/// The mp3 files under `root`, with their paths inside it. Hidden files and
/// folders are left out, and links are not followed. A name that is not UTF-8
/// cannot be stored: it is left out and named in `bad_names`.
pub fn list_folder(root: &Path) -> io::Result<Listing> {
    let mut listing = Listing::default();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            // The source itself must answer; a folder inside it may vanish.
            Err(error) if folder == root => return Err(error),
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                listing.bad_names.push(path);
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            if kind.is_dir() {
                folders.push(path);
            } else if kind.is_file()
                && is_audio(&path)
                && let (Ok(meta), Some(rel)) = (
                    entry.metadata(),
                    path.strip_prefix(root).ok().and_then(|rel| rel.to_str()),
                )
            {
                listing.found.push(Found {
                    rel_path: rel.to_string(),
                    size: meta.len(),
                    mtime: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
        }
    }
    Ok(listing)
}
