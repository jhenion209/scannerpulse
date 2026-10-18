//! The watcher, on every host: finds the mp3 files in the enabled sources and
//! records each one in `ingest_files` once it has stopped changing. It acts on
//! file system events where they are delivered, and lists every source again
//! every `ingest.rescan_seconds`, because a network share does not always
//! deliver events.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::store::{Seen, Source};

/// A file is taken when its size and modification time have not changed for this long.
pub const STABLE_FOR: Duration = Duration::from_secs(2);

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
}

impl Watcher {
    pub fn new(sources: Vec<Source>, stable_for: Duration) -> Watcher {
        Watcher {
            sources,
            stable_for,
            files: HashMap::new(),
            reported: HashSet::new(),
        }
    }

    /// The source paths, for the file system watch.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.sources.iter().map(|source| source.path.as_path())
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
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => self.observe(source_id, rel_path, &meta, now),
            _ => {
                self.files.remove(&(source_id, rel_path));
            }
        }
    }

    /// Lists every source again. Returns the files that have settled and every
    /// file recorded before and still there, so that their rows show they are
    /// still in the source. A source that cannot be listed refreshes nothing.
    pub fn rescan(&mut self, now: Instant) -> Vec<Batch> {
        let mut still_there = HashSet::new();
        let mut listed = HashSet::new();
        for source in self.sources.clone() {
            let found = match list(&source.path, &mut self.reported) {
                Ok(found) => found,
                Err(error) => {
                    crate::log::warn(format!(
                        "cannot list the source {}: {error}",
                        source.path.display()
                    ));
                    continue;
                }
            };
            listed.insert(source.id);
            for (rel_path, meta) in found {
                still_there.insert((source.id, rel_path.clone()));
                self.observe(source.id, rel_path, &meta, now);
            }
        }
        // A file gone from a source that answered is forgotten.
        self.files
            .retain(|key, _| !listed.contains(&key.0) || still_there.contains(key));
        let mut batches = self.settle(now);
        for (key, sighting) in &self.files {
            if sighting.recorded && listed.contains(&key.0) {
                push(&mut batches, key, sighting);
            }
        }
        batches
    }

    /// The files that have stopped changing and are not recorded yet. Each is
    /// looked at again: one that changed starts its wait again.
    pub fn settle(&mut self, now: Instant) -> Vec<Batch> {
        let mut batches = Vec::new();
        let due: Vec<(i16, String)> = self
            .files
            .iter()
            .filter(|(_, s)| !s.recorded && now.duration_since(s.since) >= self.stable_for)
            .map(|(key, _)| key.clone())
            .collect();
        for key in due {
            let Some(path) = self.path_of(&key) else {
                continue;
            };
            let Ok(meta) = fs::metadata(&path) else {
                self.files.remove(&key);
                continue;
            };
            let unchanged = self
                .files
                .get(&key)
                .is_some_and(|s| s.size == meta.len() && Some(s.mtime) == meta.modified().ok());
            if unchanged {
                push(&mut batches, &key, &self.files[&key]);
            } else {
                self.observe(key.0, key.1, &meta, now);
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

    fn observe(&mut self, source_id: i16, rel_path: String, meta: &fs::Metadata, now: Instant) {
        let (size, mtime) = (
            meta.len(),
            meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        );
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

    fn path_of(&self, key: &(i16, String)) -> Option<PathBuf> {
        let source = self.sources.iter().find(|source| source.id == key.0)?;
        Some(source.path.join(&key.1))
    }
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
/// cannot be stored, so it is reported once and left out.
fn list(
    root: &Path,
    reported: &mut HashSet<PathBuf>,
) -> std::io::Result<Vec<(String, fs::Metadata)>> {
    let mut found = Vec::new();
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
                if reported.insert(path.clone()) {
                    crate::log::warn(format!(
                        "{} is not a UTF-8 name; ingest leaves it alone",
                        path.display()
                    ));
                }
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
                found.push((rel.to_string(), meta));
            }
        }
    }
    Ok(found)
}
