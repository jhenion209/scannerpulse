//! `scannerpulse archive move NEW [--switch]`: the archive moved to another
//! folder, usually a network share that a second ScannerPulse machine can
//! mount too (5E1). The person copies the archive, as the command says;
//! without `--switch` the command compares the two and says what is missing.
//! `--switch`, as root, pauses filing, copies the calls filed since the
//! person's copy, compares again, then points the install at the new folder
//! and starts filing again. The old archive stays as it was, so moving back
//! is the same command the other way.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::steps::ACCOUNT;

/// The backups' folder in the archive: it stays where it is, for
/// `scannerpulse backups location` to move.
pub const BACKUPS: &str = ".scannerpulse-backups";

/// How many differences are named; the rest are counted.
const NAMED: usize = 10;

/// What the old archive holds that the new one does not, file by file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Compared {
    /// The old archive's calls' files, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// Files the new folder lacks, by their path in the archive.
    pub missing: Vec<String>,
    /// Files the new folder has with another size.
    pub differ: Vec<String>,
}

impl Compared {
    pub fn same(&self) -> bool {
        self.missing.is_empty() && self.differ.is_empty()
    }
}

/// Whether a path in the archive is one of its calls' files: not the
/// backups' folder, the install's mark, or a file the filer has not finished
/// (`.part`).
fn counted(rel: &str) -> bool {
    !(rel == BACKUPS
        || rel.starts_with(&format!("{BACKUPS}/"))
        || rel == super::second::MARK
        || rel.ends_with(".part"))
}

/// Compares the old archive with the new folder: every file of the old one
/// must be in the new one, at the same path, with the same size. `progress`
/// hears of every 10,000 files.
pub fn compare(old: &Path, new: &Path, progress: impl Fn(u64)) -> std::io::Result<Compared> {
    let mut compared = Compared::default();
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(old.join(&folder))? {
            let entry = entry?;
            let rel = folder.join(entry.file_name());
            let rel_text = rel.to_string_lossy().to_string();
            if !counted(&rel_text) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                folders.push(rel);
            } else if kind.is_file() {
                note(&mut compared, new, rel_text, entry.metadata()?.len());
                if compared.files % 10_000 == 0 {
                    progress(compared.files);
                }
            }
        }
    }
    compared.missing.sort();
    compared.differ.sort();
    Ok(compared)
}

/// Counts the old archive's file `rel`, of `size` bytes, in `compared`, and
/// notes it when the new folder lacks it or holds another size.
fn note(compared: &mut Compared, new: &Path, rel: String, size: u64) {
    compared.files += 1;
    compared.bytes += size;
    match std::fs::metadata(new.join(&rel)) {
        Ok(there) if there.is_file() && there.len() == size => {}
        Ok(_) => compared.differ.push(rel),
        Err(_) => compared.missing.push(rel),
    }
}

/// Compares only the archive's files `rels`, as `compare` does; a file the
/// old archive no longer holds is left out.
pub fn compare_these(old: &Path, new: &Path, rels: &[String]) -> Compared {
    let mut compared = Compared::default();
    for rel in rels {
        if let Ok(here) = std::fs::metadata(old.join(rel))
            && here.is_file()
        {
            note(&mut compared, new, rel.clone(), here.len());
        }
    }
    compared
}

/// The differences in words: how many, and the first few by name.
pub fn differences(compared: &Compared) -> String {
    let mut lines = Vec::new();
    for (what, list) in [
        ("missing", &compared.missing),
        ("of another size", &compared.differ),
    ] {
        if list.is_empty() {
            continue;
        }
        lines.push(format!("{} file(s) {what} in the new folder:", list.len()));
        for rel in list.iter().take(NAMED) {
            lines.push(format!("  {rel}"));
        }
        if list.len() > NAMED {
            lines.push(format!("  and {} more", list.len() - NAMED));
        }
    }
    lines.join("\n")
}

/// The copy the person runs, as the service's own account, so that the
/// copied folders are its own; the backups stay behind.
pub fn copy_line(old: &Path, new: &Path) -> String {
    format!(
        "sudo -u {ACCOUNT} rsync -a --info=progress2 --exclude=/{BACKUPS}/ --exclude=/{} {}/ {}/",
        super::second::MARK,
        old.display(),
        new.display()
    )
}

/// The folders a `RequiresMountsFor=` line names, each as written: quoted
/// names may hold spaces.
fn mount_names(folders: &str) -> Vec<String> {
    let mut names = Vec::new();
    let (mut name, mut quoted, mut escaped) = (String::new(), false, false);
    for c in folders.chars() {
        match c {
            _ if escaped => {
                name.push(c);
                escaped = false;
            }
            '\\' => {
                name.push(c);
                escaped = true;
            }
            '"' => {
                name.push(c);
                quoted = !quoted;
            }
            ' ' if !quoted => {
                if !name.is_empty() {
                    names.push(std::mem::take(&mut name));
                }
            }
            _ => name.push(c),
        }
    }
    if !name.is_empty() {
        names.push(name);
    }
    names
}

/// A unit file's text with the archive's folder `old` in its
/// `RequiresMountsFor=` line made `new`; none when the line does not name it.
pub fn moved_unit(text: &str, old: &Path, new: &Path) -> Option<String> {
    let (old, new) = (super::steps::quoted(old), super::steps::quoted(new));
    let mut found = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| match line.strip_prefix("RequiresMountsFor=") {
            Some(folders) if mount_names(folders).contains(&old) => {
                found = true;
                let names: Vec<String> = mount_names(folders)
                    .into_iter()
                    .map(|name| if name == old { new.clone() } else { name })
                    .collect();
                format!("RequiresMountsFor={}", names.join(" "))
            }
            _ => line.to_string(),
        })
        .collect();
    found.then(|| lines.join("\n") + "\n")
}

/// The new folder may not be a recordings folder, nor inside one, nor hold
/// one: the watcher would take the archive's files for new recordings, and
/// the filer delete them as filed. Nor may it be the backups' folder or
/// inside it; it may hold it, as setup's own layout does.
pub fn apart_from(new: &Path, sources: &[PathBuf], backups: Option<&Path>) -> Result<(), String> {
    let nested = sources
        .iter()
        .map(PathBuf::as_path)
        .find(|source| new.starts_with(source) || source.starts_with(new))
        .or(backups.filter(|backups| new.starts_with(backups)));
    if let Some(other) = nested {
        return Err(format!(
            "{} and {} must not be the same folder or inside each other",
            new.display(),
            other.display()
        ));
    }
    Ok(())
}

/// The calls' files of the old archive written at `since` or later. Only
/// the filer writes there, a new file each time, so a folder unchanged since
/// `since` gained no file and its files are not read: on a large archive
/// only the day's folders are.
pub fn changed_since(old: &Path, since: std::time::SystemTime) -> std::io::Result<Vec<String>> {
    let mut changed = Vec::new();
    let mut folders = vec![(PathBuf::new(), true)];
    while let Some((folder, fresh)) = folders.pop() {
        for entry in std::fs::read_dir(old.join(&folder))? {
            let entry = entry?;
            let rel = folder.join(entry.file_name());
            let rel_text = rel.to_string_lossy().to_string();
            if !counted(&rel_text) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_dir() {
                let fresh = entry.metadata()?.modified()? >= since;
                folders.push((rel, fresh));
            } else if fresh && kind.is_file() && entry.metadata()?.modified()? >= since {
                changed.push(rel_text);
            }
        }
    }
    changed.sort();
    Ok(changed)
}

/// Points the archive (the primary location) at `new`.
pub async fn point_at(conn: &mut sqlx::PgConnection, new: &Path) -> sqlx::Result<u64> {
    sqlx::query("UPDATE locations SET root = $1 WHERE is_primary")
        .bind(new.display().to_string())
        .execute(conn)
        .await
        .map(|done| done.rows_affected())
}

/// The archive's folder now.
pub async fn current(conn: &mut sqlx::PgConnection) -> sqlx::Result<Option<PathBuf>> {
    let root: Option<String> = sqlx::query_scalar("SELECT root FROM locations WHERE is_primary")
        .fetch_optional(conn)
        .await?;
    Ok(root.map(PathBuf::from))
}

/// Copies `$1` to `$2`, its folders made as needed. Its time is kept where
/// the folder allows; an SMB share refuses the rest of `cp -p` (its owner
/// and mode), which the share decides anyway.
const COPY: &str = "mkdir -p -- \"$(dirname -- \"$2\")\" && cp -- \"$1\" \"$2\" && { touch -r \"$1\" -- \"$2\" 2>/dev/null; true; }";

/// Copies the old archive's file `rel` into the new folder as the service's
/// account.
fn copy_as_account(old: &Path, new: &Path, rel: &str) -> Result<(), String> {
    let output = Command::new("runuser")
        .args(["-u", ACCOUNT, "--", "sh", "-c"])
        .arg(COPY)
        .arg("sh")
        .arg(old.join(rel))
        .arg(new.join(rel))
        .output()
        .map_err(|error| format!("cannot run runuser: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{rel} could not be copied: {}",
            super::apply::said(&output)
        ))
    }
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|error| format!("cannot run systemctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl {}: {}",
            args.join(" "),
            super::apply::said(&output)
        ))
    }
}

/// Checks that `new` is a folder the account reaches and writes, and says
/// whether it is a network share.
fn usable(new: &Path, say: &impl Fn(&str)) -> Result<(), String> {
    if !new.is_absolute() || !new.is_dir() {
        return Err(format!("{} is not a folder here", new.display()));
    }
    let share = Command::new("sh")
        .arg("-c")
        .arg(super::second::SHARE_CHECK)
        .env("ARCHIVE", new)
        .output()
        .is_ok_and(|output| output.status.success());
    if !share {
        say(&format!(
            "{} is not a network share: a second ScannerPulse machine could not use it",
            new.display()
        ));
    }
    let reach = Command::new("runuser")
        .args(["-u", ACCOUNT, "--", "sh", "-c"])
        .arg(super::steps::reach(ACCOUNT))
        .arg("sh")
        .arg(new)
        .output()
        .map_err(|error| format!("cannot run runuser: {error}"))?;
    if !reach.status.success() {
        return Err(super::apply::said(&reach));
    }
    Ok(())
}

/// `scannerpulse archive move NEW [--switch]`, as root on the ScannerPulse
/// machine of an install without a second one.
pub async fn run(
    conn: &mut sqlx::PgConnection,
    new: &Path,
    switch: bool,
    say: impl Fn(&str),
) -> Result<String, String> {
    let settings = std::fs::read_to_string(super::steps::env_path())
        .map_err(|_| "this machine runs no ScannerPulse site".to_string())?;
    let plan_here = Path::new(super::plan::PLAN).exists();
    if super::second::is_second(&settings, plan_here)
        || (plan_here
            && super::plan::Plan::read(Path::new(super::plan::PLAN))?
                .second
                .is_some())
    {
        return Err("this install has a second ScannerPulse machine: its archive is a share already, and both machines would need to move together".into());
    }
    let old = current(conn)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("the database names no archive")?;
    let new = new
        .canonicalize()
        .map_err(|error| format!("{}: {error}", new.display()))?;
    if new == old {
        return Err(format!("{} is the archive already", old.display()));
    }
    if new.starts_with(&old) || old.starts_with(&new) {
        return Err(
            "the new folder may not be inside the archive, nor the archive inside it".into(),
        );
    }
    let sources: Vec<PathBuf> = sqlx::query_scalar::<_, String>("SELECT path FROM sources")
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|path| canonical(PathBuf::from(path)))
        .collect();
    let backups = std::env::var_os("SCANNERPULSE_BACKUPS").map(|b| canonical(PathBuf::from(b)));
    apart_from(&new, &sources, backups.as_deref())?;
    usable(&new, &say)?;
    // Whatever is filed from here on is looked at again in the pause.
    // ponytail: a minute's margin for the archive's clock (a share's server);
    // a larger skew would miss the calls filed during the comparison.
    let started = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    say(&format!(
        "Comparing {} with {}...",
        old.display(),
        new.display()
    ));
    let compared = compare(&old, &new, |n| say(&format!("  {n} files compared")))
        .map_err(|error| error.to_string())?;
    say(&format!(
        "The archive at {} holds {} file(s), {}.",
        old.display(),
        compared.files,
        super::folders::size(compared.bytes)
    ));
    if !switch {
        return Ok(if compared.same() {
            format!(
                "{} holds every one of them. To move to it, run: sudo /usr/local/bin/scannerpulse archive move {} --switch",
                new.display(),
                new.display()
            )
        } else {
            format!(
                "{}\nCopy them with:\n  {}\nthen run this again.",
                differences(&compared),
                copy_line(&old, &new)
            )
        });
    }
    if compared.differ.len() + compared.missing.len() > 1_000 {
        return Err(format!(
            "{}\nCopy them first with:\n  {}",
            differences(&compared),
            copy_line(&old, &new)
        ));
    }
    // Both units are moved in words before anything stops.
    let mut units = Vec::new();
    for unit in ["scannerpulse-web.service", "scannerpulse-ingest.service"] {
        let path = Path::new("/etc/systemd/system").join(unit);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let moved = moved_unit(&text, &old, &new).ok_or_else(|| {
            format!(
                "{} does not name {} on its RequiresMountsFor= line, so nothing was changed; put it back as setup wrote it, then try again",
                path.display(),
                old.display()
            )
        })?;
        units.push((path, moved));
    }
    let _lock = crate::update::exclusive(Path::new(crate::update::LOCK))?;
    // Ctrl-C, a closed terminal or a stop wait for the end of the switch,
    // and the guard starts filing again however it ends, a panic included.
    use tokio::signal::unix::{SignalKind, signal};
    let _held = [
        SignalKind::interrupt(),
        SignalKind::hangup(),
        SignalKind::terminate(),
    ]
    .into_iter()
    .map(signal)
    .collect::<std::io::Result<Vec<_>>>()
    .map_err(|error| format!("cannot hold off signals: {error}"))?;
    let _filing = Filing;
    say("Pausing filing (the recordings wait in their folder)...");
    systemctl(&["stop", "scannerpulse-ingest.service"])?;
    let switched = async {
        say("Looking for the calls filed since the comparison began...");
        let mut late: Vec<String> = compared
            .missing
            .iter()
            .chain(&compared.differ)
            .cloned()
            .collect();
        late.extend(changed_since(&old, started).map_err(|error| error.to_string())?);
        late.sort();
        late.dedup();
        let since = compare_these(&old, &new, &late);
        let copy: Vec<&String> = since.missing.iter().chain(&since.differ).collect();
        if !copy.is_empty() {
            say(&format!(
                "Copying {} file(s) filed since the copy...",
                copy.len()
            ));
        }
        for rel in &copy {
            copy_as_account(&old, &new, rel)?;
        }
        let last = compare_these(&old, &new, &late);
        if !last.same() {
            return Err(format!(
                "the new folder still differs, so nothing was switched:\n{}",
                differences(&last)
            ));
        }
        if !copy.is_empty() {
            say(&format!("Copied {} file(s).", copy.len()));
        }
        for (path, moved) in &units {
            std::fs::write(path, moved).map_err(|error| format!("{}: {error}", path.display()))?;
        }
        systemctl(&["daemon-reload"])?;
        point_at(conn, &new)
            .await
            .map_err(|error| error.to_string())?;
        if plan_here {
            super::plan::Plan::update(Path::new(super::plan::PLAN), |plan| {
                plan.archive = new.display().to_string();
            })?;
        }
        Ok(())
    }
    .await;
    say("Filing again...");
    let restarted = systemctl(&[
        "restart",
        "scannerpulse-web.service",
        "scannerpulse-ingest.service",
    ]);
    switched?;
    restarted?;
    Ok(moved_text(&old, &new, old.join(BACKUPS).is_dir()))
}

/// The path with its links resolved, or as it is when it is not there.
fn canonical(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

/// Starts filing when dropped, so that a switch that ends early, by an error
/// or a panic, does not leave it stopped; a no-op when filing runs.
struct Filing;

impl Drop for Filing {
    fn drop(&mut self) {
        let _ = systemctl(&["start", "scannerpulse-ingest.service"]);
    }
}

/// What the person is told once the archive has moved from `old` to `new`;
/// `backups` when the backups' folder is in the old one.
fn moved_text(old: &Path, new: &Path, backups: bool) -> String {
    let (old, new) = (old.display(), new.display());
    let keep = if backups {
        format!(
            ", but keep {old}/{BACKUPS} while it holds the backups (`scannerpulse backups location FOLDER` moves them)"
        )
    } else {
        String::new()
    };
    format!(
        "The archive is now {new}. The old one at {old} is kept as it was: remove its calls once you are satisfied{keep}. To move back, run this command with {old}."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn the_new_folder_must_hold_every_call_at_the_same_path_and_size() {
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for (rel, bytes) in [
            ("2026-10-19/100-Fire/a.mp3", &b"aaaa"[..]),
            ("2026-10-19/100-Fire/b.mp3", b"bb"),
            ("2026-10-20/Unknown/c.mp3", b"c"),
        ] {
            write(old.path(), rel, bytes);
        }
        // Not calls: the backups, the mark, a file not yet filed.
        write(old.path(), ".scannerpulse-backups/dump/x.sql", b"dump");
        write(old.path(), ".scannerpulse-install", b"id");
        write(old.path(), "2026-10-20/Unknown/d.mp3.part", b"half");
        write(new.path(), "2026-10-19/100-Fire/a.mp3", b"aaaa");
        write(new.path(), "2026-10-19/100-Fire/b.mp3", b"b");
        let compared = compare(old.path(), new.path(), |_| {}).unwrap();
        assert_eq!(compared.files, 3);
        assert_eq!(compared.bytes, 7);
        assert_eq!(compared.missing, vec!["2026-10-20/Unknown/c.mp3"]);
        assert_eq!(compared.differ, vec!["2026-10-19/100-Fire/b.mp3"]);
        assert!(!compared.same());
        let words = differences(&compared);
        assert!(
            words.contains("1 file(s) missing") && words.contains("c.mp3"),
            "{words}"
        );
        write(new.path(), "2026-10-19/100-Fire/b.mp3", b"bb");
        write(new.path(), "2026-10-20/Unknown/c.mp3", b"c");
        assert!(compare(old.path(), new.path(), |_| {}).unwrap().same());
    }

    #[test]
    fn a_file_filed_since_is_copied_with_its_folders_and_its_time() {
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        write(old.path(), "2026-10-19/Unknown/late.mp3", b"late");
        let source = old.path().join("2026-10-19/Unknown/late.mp3");
        let made = std::process::Command::new("touch")
            .args(["-d", "2026-10-19 12:00:00"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(made.success());
        let copied = std::process::Command::new("sh")
            .args(["-c", COPY, "sh"])
            .arg(&source)
            .arg(new.path().join("2026-10-19/Unknown/late.mp3"))
            .status()
            .unwrap();
        assert!(copied.success());
        let there = new.path().join("2026-10-19/Unknown/late.mp3");
        assert_eq!(std::fs::read(&there).unwrap(), b"late");
        assert_eq!(
            std::fs::metadata(&there).unwrap().modified().unwrap(),
            std::fs::metadata(&source).unwrap().modified().unwrap()
        );
        assert!(compare(old.path(), new.path(), |_| {}).unwrap().same());
    }

    #[test]
    fn the_backups_are_named_only_where_they_are() {
        let (old, new) = (Path::new("/srv/archive"), Path::new("/srv/share"));
        let with = moved_text(old, new, true);
        assert!(
            with.contains("keep /srv/archive/.scannerpulse-backups"),
            "{with}"
        );
        let back = moved_text(new, old, false);
        assert!(!back.contains(".scannerpulse-backups"), "{back}");
        assert!(back.contains("run this command with /srv/share"), "{back}");
    }

    #[test]
    fn the_new_folder_stays_apart_from_the_recordings_and_the_backups() {
        let sources = vec![PathBuf::from("/srv/recordings")];
        let backups = Some(PathBuf::from("/srv/backups"));
        assert!(apart_from(Path::new("/srv/shared"), &sources, backups.as_deref()).is_ok());
        // Setup's own layout, moved back to: the backups inside the archive.
        let inside = Some(PathBuf::from("/srv/archive/.scannerpulse-backups"));
        assert!(apart_from(Path::new("/srv/archive"), &sources, inside.as_deref()).is_ok());
        // Inside the recordings folder, the watcher would take the archive's
        // files for new recordings, and the filer would delete them.
        for refused in ["/srv/recordings/archive", "/srv", "/srv/backups/archive"] {
            let error = apart_from(Path::new(refused), &sources, backups.as_deref()).unwrap_err();
            assert!(error.contains("inside each other"), "{refused}: {error}");
        }
    }

    #[test]
    fn only_files_changed_since_a_moment_are_looked_at_again() {
        let old = tempfile::tempdir().unwrap();
        write(old.path(), "2026-10-18/1-A/before.mp3", b"b");
        let touched = std::process::Command::new("touch")
            .args(["-d", "2026-10-18 12:00:00"])
            .arg(old.path().join("2026-10-18/1-A/before.mp3"))
            .status()
            .unwrap();
        assert!(touched.success());
        let since = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        write(old.path(), "2026-10-19/Unknown/late.mp3", b"late");
        write(old.path(), ".scannerpulse-backups/dump/x.sql", b"dump");
        // A folder unchanged since then gained no file: its files are not
        // read, so a pause reads only the day's folders.
        write(old.path(), "2026-10-17/1-A/kept.mp3", b"k");
        let touched = std::process::Command::new("touch")
            .args(["-d", "2026-10-17 12:00:00"])
            .arg(old.path().join("2026-10-17/1-A"))
            .status()
            .unwrap();
        assert!(touched.success());
        assert_eq!(
            changed_since(old.path(), since).unwrap(),
            vec!["2026-10-19/Unknown/late.mp3".to_string()]
        );
    }

    #[test]
    fn only_the_files_named_are_compared_again() {
        let (old, new) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        for rel in ["d/a.mp3", "d/b.mp3", "d/c.mp3"] {
            write(old.path(), rel, b"abc");
        }
        write(new.path(), "d/a.mp3", b"abc");
        write(new.path(), "d/c.mp3", b"c");
        let named: Vec<String> = ["d/a.mp3", "d/b.mp3", "d/c.mp3", "d/gone.mp3"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            compare_these(old.path(), new.path(), &named),
            Compared {
                files: 3,
                bytes: 9,
                missing: vec!["d/b.mp3".into()],
                differ: vec!["d/c.mp3".into()],
            }
        );
    }

    #[test]
    fn many_differences_are_counted_after_the_first_few() {
        let compared = Compared {
            missing: (0..25).map(|n| format!("2026-10-19/1-A/{n}.mp3")).collect(),
            ..Compared::default()
        };
        let words = differences(&compared);
        assert!(words.contains("25 file(s) missing"), "{words}");
        assert!(words.contains("and 15 more"), "{words}");
        assert_eq!(words.lines().count(), 1 + NAMED + 1);
    }

    #[test]
    fn the_copy_runs_as_the_account_and_leaves_the_backups_behind() {
        let line = copy_line(Path::new("/srv/archive"), Path::new("/srv/shared/archive"));
        assert_eq!(
            line,
            "sudo -u scannerpulse rsync -a --info=progress2 --exclude=/.scannerpulse-backups/ --exclude=/.scannerpulse-install /srv/archive/ /srv/shared/archive/"
        );
    }

    #[test]
    fn the_services_wait_for_the_new_folders_mount() {
        let unit = "[Unit]\nDescription=x\nRequiresMountsFor=\"/srv/recordings\" \"/srv/archive\"\n\n[Service]\nUser=scannerpulse\n";
        let moved = moved_unit(
            unit,
            Path::new("/srv/archive"),
            Path::new("/srv/shared/archive"),
        )
        .unwrap();
        assert!(
            moved.contains("RequiresMountsFor=\"/srv/recordings\" \"/srv/shared/archive\"\n"),
            "{moved}"
        );
        assert!(moved.contains("User=scannerpulse"), "{moved}");
        // A folder with a space (setup allows one) is one quoted name.
        let spaced = "RequiresMountsFor=\"/srv/recordings\" \"/srv/my archive\"\n";
        assert_eq!(
            moved_unit(
                spaced,
                Path::new("/srv/my archive"),
                Path::new("/srv/shared")
            )
            .unwrap(),
            "RequiresMountsFor=\"/srv/recordings\" \"/srv/shared\"\n"
        );
        // A unit that does not name the archive is left alone.
        assert_eq!(
            moved_unit(unit, Path::new("/srv/other"), Path::new("/x")),
            None
        );
    }
}
