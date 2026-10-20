//! The two folders setup asks for, and the only things that refuse one: it
//! must be an absolute path to a folder that exists, that this machine can
//! write, with free space, and the two may not sit inside each other. A
//! folder on the system disk is allowed; setup says what filling it means.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The least free space setup accepts in a folder.
pub const MIN_FREE: u64 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// The folder, with links and `..` resolved.
    pub path: PathBuf,
    pub free_bytes: u64,
    /// On the same file system as `/`.
    pub on_system_disk: bool,
}

/// `path` as setup can use it, or why not.
pub fn check(path: &Path) -> Result<Folder, String> {
    if !path.is_absolute() {
        return Err(format!(
            "{} is not a full path: start it with /",
            path.display()
        ));
    }
    let meta = std::fs::metadata(path).map_err(|_| {
        format!(
            "{} does not exist: make the folder first, or give another",
            path.display()
        )
    })?;
    if !meta.is_dir() {
        return Err(format!("{} is a file, not a folder", path.display()));
    }
    let real =
        std::fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let probe = real.join(format!(".scannerpulse-write-check-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|error| format!("{} cannot be written: {error}", real.display()))?;
    let stats =
        rustix::fs::statvfs(&real).map_err(|error| format!("{}: {error}", real.display()))?;
    let free_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
    if free_bytes < MIN_FREE {
        return Err(format!(
            "{} has {} free; setup needs at least {}",
            real.display(),
            size(free_bytes),
            size(MIN_FREE)
        ));
    }
    let root = std::fs::metadata("/").map_err(|error| format!("/: {error}"))?;
    Ok(Folder {
        path: real,
        free_bytes,
        on_system_disk: meta.dev() == root.dev(),
    })
}

/// The recordings folder and the archive may not be the same folder or sit
/// inside each other: the archive would be read as new recordings, or the
/// recordings filed into themselves.
pub fn apart(recordings: &Folder, archive: &Folder) -> Result<(), String> {
    let (a, b) = (&recordings.path, &archive.path);
    if a.starts_with(b) || b.starts_with(a) {
        Err(format!(
            "the recordings folder {} and the archive {} must not be the same folder or inside each other",
            a.display(),
            b.display()
        ))
    } else {
        Ok(())
    }
}

/// Records the archive as the primary location and the recordings folder as
/// the first recorder, watched, or points the existing ones at these folders.
pub async fn record(
    conn: &mut sqlx::PgConnection,
    recordings: &Path,
    archive: &Path,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO locations (name, root, mode, is_primary) VALUES ('archive', $1, 'managed', true) \
         ON CONFLICT (name) DO UPDATE SET root = EXCLUDED.root",
    )
    .bind(archive.display().to_string())
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO sources (name, kind, path) VALUES ('recordings', 'sdrtrunk', $1) \
         ON CONFLICT (name) DO UPDATE SET path = EXCLUDED.path, enabled = true, \
             intake = 'watched', removed_at = NULL",
    )
    .bind(recordings.display().to_string())
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// A size as people read it: `12.3 GB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{apart, check, size};
    use std::path::Path;

    #[test]
    fn a_folder_that_exists_and_can_be_written_is_taken() {
        let dir = tempfile::tempdir().unwrap();
        let folder = check(dir.path()).unwrap();
        assert_eq!(folder.path, std::fs::canonicalize(dir.path()).unwrap());
        assert!(folder.free_bytes >= super::MIN_FREE);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "the write check leaves nothing"
        );
    }

    #[test]
    fn a_relative_missing_or_file_path_is_refused_with_why() {
        assert!(
            check(Path::new("archive"))
                .unwrap_err()
                .contains("not a full path")
        );
        assert!(
            check(Path::new("/no/such/folder/here"))
                .unwrap_err()
                .contains("does not exist")
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert!(check(&file).unwrap_err().contains("is a file"));
    }

    #[test]
    fn the_two_folders_must_be_apart() {
        let dir = tempfile::tempdir().unwrap();
        let inner = dir.path().join("archive");
        std::fs::create_dir(&inner).unwrap();
        let outer = check(dir.path()).unwrap();
        let archive = check(&inner).unwrap();
        assert!(apart(&outer, &archive).is_err());
        assert!(apart(&archive, &outer).is_err());
        assert!(apart(&outer, &outer).is_err());
        let other = tempfile::tempdir().unwrap();
        assert!(apart(&check(other.path()).unwrap(), &archive).is_ok());
    }

    #[test]
    fn sizes_read_as_people_read_them() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(1_500_000), "1.5 MB");
        assert_eq!(size(12_300_000_000), "12.3 GB");
    }
}
