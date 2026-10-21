//! Where a call's file goes in the archive, and how it gets there without
//! being lost or filed twice.
//!
//! A file goes to `<local date>/<talkgroup folder>/<file name>` under the root
//! of the primary location. It is written as
//! `<name>.part`, read back and compared, and only then given its name.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use sha2::{Digest, Sha256};

pub type Digest32 = [u8; 32];

/// Held by every writer of the archive in this process, from choosing a
/// file's name to giving the file that name: the filer and the jobs both
/// run on the lease holder, and two of them must never take one free name.
pub static WRITES: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// The folder of a talkgroup, `<number>-<name>` with any `/` in the name made
/// `_`; `Unknown` for a call without a talkgroup.
pub fn folder(talkgroup: Option<(i64, &str)>) -> String {
    match talkgroup {
        Some((number, name)) => format!("{number}-{}", name.replace('/', "_")),
        None => "Unknown".to_string(),
    }
}

/// The archive path of a file, relative to the root: `<YYYY-MM-DD>/<folder>/<name>`.
/// Refuses a folder or a name that is empty, `.` or `..`, or that holds `/` or NUL.
pub fn rel_path(date: NaiveDate, folder: &str, name: &str) -> Result<String, String> {
    for part in [folder, name] {
        if part.is_empty() || part == "." || part == ".." || part.contains(['/', '\0']) {
            return Err(format!(
                "{part:?} cannot name a folder or a file in the archive"
            ));
        }
    }
    Ok(format!("{}/{folder}/{name}", date.format("%Y-%m-%d")))
}

/// The `n`th name to try for a file: `a.mp3`, then `a_1.mp3`, `a_2.mp3`, ...
pub fn candidate(name: &str, n: u32) -> String {
    if n == 0 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!("{stem}_{n}.{extension}"),
        _ => format!("{name}_{n}"),
    }
}

/// The name a file has while it is being written: `<name>.part`.
pub fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    path.with_file_name(name)
}

pub fn sha256(data: &[u8]) -> Digest32 {
    Sha256::digest(data).into()
}

/// The SHA-256 and the size of a file, read in full.
pub fn file_digest(path: &Path) -> io::Result<(Digest32, u64)> {
    let mut file = File::open(path)?;
    let mut hashing = Hashing(Sha256::new());
    let size = io::copy(&mut file, &mut hashing)?;
    Ok((hashing.0.finalize().into(), size))
}

/// A hasher that `io::copy` can write into (sha2 has no `std` side).
struct Hashing(Sha256);

impl io::Write for Hashing {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.0.update(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Where a file belongs in its folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// An exact copy (same size and SHA-256) is there already, at this path
    /// relative to the root.
    Copy(String),
    /// The first free name, as a path relative to the root.
    Free(String),
}

/// Looks through `name`, `name_1`, `name_2`, ... in `dir` (relative to `root`)
/// for an exact copy of the file, and otherwise returns the first free name.
/// Deletes any part file of those names, left behind by a crash. A missing
/// folder means the first name is free.
pub fn place(
    root: &Path,
    dir: &str,
    name: &str,
    size: u64,
    digest: &Digest32,
) -> io::Result<Place> {
    let mut n = 0;
    loop {
        let rel = format!("{dir}/{}", candidate(name, n));
        let path = root.join(&rel);
        match fs::remove_file(part_path(&path)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        match fs::metadata(&path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Place::Free(rel)),
            Err(error) => return Err(error),
            Ok(meta) if meta.len() == size && file_digest(&path)?.0 == *digest => {
                return Ok(Place::Copy(rel));
            }
            Ok(_) => n += 1,
        }
    }
}

/// The archive's folder for the files of a system no one has named yet.
pub const STAGING: &str = ".scannerpulse-staging";

/// Where a staged file waits: `<archive>/.scannerpulse-staging/<source id>/<path in its recorder's folder>`.
pub fn staging_path(root: &Path, source_id: i16, rel_path: &str) -> PathBuf {
    root.join(STAGING)
        .join(source_id.to_string())
        .join(rel_path)
}

/// Step 1 of filing: writes `data` to the part file of `path`, flushes it to
/// disk, reads it back and compares its size and SHA-256 with `digest`.
pub fn write_part(path: &Path, data: &[u8], digest: &Digest32) -> io::Result<()> {
    let part = part_path(path);
    if let Some(dir) = part.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = File::create(&part)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    let (got, size) = file_digest(&part)?;
    if size != data.len() as u64 || got != *digest {
        let _ = fs::remove_file(&part);
        return Err(io::Error::other(
            "the copy in the archive does not match the file",
        ));
    }
    Ok(())
}

/// Step 2 of filing: gives the part file the name `path`. Never replaces a
/// file: only the lease holder files, so nothing else creates the name between
/// the check and the rename.
pub fn finish_part(path: &Path) -> io::Result<()> {
    if path.try_exists()? {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} exists already", path.display()),
        ));
    }
    fs::rename(part_path(path), path)?;
    // Best effort: some network file systems refuse to flush a folder.
    if let Some(dir) = path.parent() {
        let _ = File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}
