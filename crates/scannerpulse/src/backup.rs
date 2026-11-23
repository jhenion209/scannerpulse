//! `scannerpulse backup`: the nightly backups, run as root by the backup
//! timer that setup writes. pgBackRest takes a full backup on Sundays and a
//! differential one on other days (point-in-time recovery from the WAL it
//! archives); a dump of the database follows, of which the newest
//! `KEEP_DUMPS` are kept. Both run even when the other fails.

use std::path::Path;
use std::process::Command;

use chrono::{Datelike, Weekday};

/// How many dumps are kept.
pub const KEEP_DUMPS: usize = 14;
/// pgBackRest's settings for ScannerPulse, written by setup.
pub const PGBACKREST_CONF: &str = "/etc/pgbackrest/scannerpulse.conf";
/// pgBackRest's name for this database cluster.
pub const STANZA: &str = "scannerpulse";

/// pgBackRest's backup type for `day`.
pub fn kind(day: Weekday) -> &'static str {
    if day == Weekday::Sun { "full" } else { "diff" }
}

/// The dumps among `names` to remove: all but the newest `keep` (the
/// names carry their time, so they sort by it).
pub fn old_dumps(names: &[String], keep: usize) -> Vec<String> {
    let mut dumps: Vec<&String> = names
        .iter()
        .filter(|n| n.starts_with("scannerpulse-") && n.ends_with(".dump"))
        .collect();
    dumps.sort();
    let extra = dumps.len().saturating_sub(keep);
    dumps.into_iter().take(extra).cloned().collect()
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    // Both outputs: pgBackRest writes its errors on its standard output.
    Err(format!(
        "{program} ended with {}: {}",
        output.status,
        crate::setup::apply::said(&output)
    ))
}

/// The kinds of backup, as the `backups` table and the push variables name
/// them.
pub const KINDS: [&str; 2] = ["pgbackrest", "dump"];

/// Each kind of backup taken, and how it went.
pub type Outcomes = Vec<(&'static str, Result<String, String>)>;

/// The settings-file variable holding `kind`'s push (heartbeat) address:
/// it carries a token, so it is not kept in the database.
pub fn push_var(kind: &str) -> Option<&'static str> {
    match kind {
        "pgbackrest" => Some("SCANNERPULSE_BACKUP_PUSH_PGBACKREST"),
        "dump" => Some("SCANNERPULSE_BACKUP_PUSH_DUMP"),
        _ => None,
    }
}

/// Notes that a backup of `kind` has just succeeded.
pub async fn record(db: impl sqlx::PgExecutor<'_>, kind: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO backups (kind, finished_at) VALUES ($1, now()) \
         ON CONFLICT (kind) DO UPDATE SET finished_at = EXCLUDED.finished_at",
    )
    .bind(kind)
    .execute(db)
    .await
    .map(drop)
}

/// Each kind's newest success, by kind.
pub async fn newest(
    db: impl sqlx::PgExecutor<'_>,
) -> sqlx::Result<Vec<(String, chrono::DateTime<chrono::Utc>)>> {
    sqlx::query_as("SELECT kind, finished_at FROM backups ORDER BY kind")
        .fetch_all(db)
        .await
}

/// Calls a push (heartbeat) address, such as an Uptime Kuma push monitor's.
pub async fn push(url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    // The address carries a token: only its host goes in a message.
    let host = reqwest::Url::parse(url)
        .ok()
        .and_then(|u| Some(format!("{}:{}", u.host_str()?, u.port_or_known_default()?)))
        .unwrap_or_else(|| "the push address".into());
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|_| format!("{host} did not answer"))?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("{host} answered {}", response.status()))
    }
}

/// Takes tonight's backups into `backups` (its `pgbackrest` and `dump`
/// folders) from the database at `database_url`: each kind, as `KINDS`
/// names it, with what it did or why it failed. Both run even when the
/// other fails.
/// `as_root`: run as root, which runs pgBackRest as the postgres account;
/// else this is the postgres account already (a database machine's).
pub fn nightly(backups: &Path, database_url: &str, as_root: bool) -> Outcomes {
    let now = chrono::Local::now();
    let kind = kind(now.weekday());
    let config = format!("--config={PGBACKREST_CONF}");
    let stanza = format!("--stanza={STANZA}");
    let kind_arg = format!("--type={kind}");
    let args = [config.as_str(), &stanza, "backup", &kind_arg];
    let pgbackrest = if as_root {
        run(
            "runuser",
            &[&["-u", "postgres", "--", "pgbackrest"][..], &args].concat(),
        )
    } else {
        run("pgbackrest", &args)
    }
    .map(|()| format!("pgBackRest: {kind} backup taken"))
    .map_err(|why| format!("pgBackRest ({kind}): {why}"));
    let dump = dump(
        &backups.join("dump"),
        database_url,
        &now.format("%Y%m%d-%H%M%S").to_string(),
    )
    .map(|name| format!("dump: {name} written"))
    .map_err(|why| format!("dump: {why}"));
    vec![(KINDS[0], pgbackrest), (KINDS[1], dump)]
}

/// Writes `scannerpulse-<stamp>.dump` in `folder`, checks that pg_restore
/// reads it before giving it its name, and removes the oldest beyond
/// `KEEP_DUMPS`.
pub fn dump(folder: &Path, database_url: &str, stamp: &str) -> Result<String, String> {
    dump_with(folder, database_url, stamp, ("pg_dump", "pg_restore"))
}

/// `dump` with the programs it runs: (pg_dump, pg_restore).
fn dump_with(
    folder: &Path,
    database_url: &str,
    stamp: &str,
    (pg_dump, pg_restore): (&str, &str),
) -> Result<String, String> {
    let name = format!("scannerpulse-{stamp}.dump");
    let partial = folder.join(format!(".{name}.partial"));
    let partial_arg = partial.display().to_string();
    let result = run(
        pg_dump,
        &[
            "--format=custom",
            "--file",
            &partial_arg,
            "--dbname",
            database_url,
        ],
    )
    .and_then(|()| run(pg_restore, &["--list", &partial_arg]))
    .and_then(|()| {
        std::fs::rename(&partial, folder.join(&name)).map_err(|error| error.to_string())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result?;
    let names: Vec<String> = std::fs::read_dir(folder)
        .map_err(|error| format!("{}: {error}", folder.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    // The new dump is made: a problem removing old ones says so, with its name.
    for old in old_dumps(&names, KEEP_DUMPS) {
        std::fs::remove_file(folder.join(&old)).map_err(|error| {
            format!("{name} was made, but the old dump {old} could not be removed: {error}")
        })?;
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::{dump_with, kind, old_dumps, run};

    /// Stand-ins for pg_dump (writes its --file) and pg_restore (exits with `restore_code`).
    fn fakes(dir: &std::path::Path, restore_code: i32) -> (String, String) {
        use std::os::unix::fs::PermissionsExt;
        let script = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path.display().to_string()
        };
        (
            script("pg_dump", "echo dump > \"$3\""),
            script("pg_restore", &format!("exit {restore_code}")),
        )
    }

    #[test]
    fn a_dump_is_named_once_pg_restore_reads_it() {
        let (bin, folder) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (pg_dump, pg_restore) = fakes(bin.path(), 0);
        let name = dump_with(
            folder.path(),
            "postgres://x",
            "20261123T031700",
            (&pg_dump, &pg_restore),
        )
        .unwrap();
        assert_eq!(name, "scannerpulse-20261123T031700.dump");
        let left: Vec<_> = std::fs::read_dir(folder.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            left,
            [std::ffi::OsString::from(&name)],
            "no partial file left"
        );
    }

    #[test]
    fn a_dump_pg_restore_cannot_read_is_not_kept() {
        let (bin, folder) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (pg_dump, pg_restore) = fakes(bin.path(), 1);
        assert!(
            dump_with(
                folder.path(),
                "postgres://x",
                "20261123T031700",
                (&pg_dump, &pg_restore)
            )
            .is_err()
        );
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_clean_up_names_the_dump_that_was_made() {
        let (bin, folder) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (pg_dump, pg_restore) = fakes(bin.path(), 0);
        // More old dumps than are kept, the oldest one a folder that cannot be removed as a file.
        std::fs::create_dir(folder.path().join("scannerpulse-20200101T031700.dump")).unwrap();
        for d in 2..=20 {
            std::fs::write(
                folder
                    .path()
                    .join(format!("scannerpulse-202001{d:02}T031700.dump")),
                b"x",
            )
            .unwrap();
        }
        let error = dump_with(
            folder.path(),
            "postgres://x",
            "20261123T031700",
            (&pg_dump, &pg_restore),
        )
        .unwrap_err();
        assert!(
            error.contains("scannerpulse-20261123T031700.dump was made"),
            "{error}"
        );
        assert!(
            folder
                .path()
                .join("scannerpulse-20261123T031700.dump")
                .exists()
        );
    }
    use chrono::Weekday;

    #[test]
    fn sunday_takes_a_full_backup_and_other_days_a_differential_one() {
        assert_eq!(kind(Weekday::Sun), "full");
        for day in [Weekday::Mon, Weekday::Wed, Weekday::Sat] {
            assert_eq!(kind(day), "diff");
        }
    }

    #[test]
    fn only_the_oldest_dumps_beyond_the_kept_number_go() {
        let names: Vec<String> = [
            "scannerpulse-20261003-031700.dump",
            "scannerpulse-20261001-031700.dump",
            ".scannerpulse-20261004-031700.dump.partial",
            "notes.txt",
            "scannerpulse-20261002-031700.dump",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(old_dumps(&names, 2), ["scannerpulse-20261001-031700.dump"]);
        assert!(old_dumps(&names, 14).is_empty());
    }

    #[test]
    fn a_failed_parts_error_keeps_what_it_wrote_on_its_standard_output() {
        // pgBackRest writes its errors there.
        let error = run(
            "/bin/sh",
            &[
                "-c",
                "echo ERROR: [055]: the reason; echo warning >&2; exit 55",
            ],
        )
        .unwrap_err();
        assert!(
            error.contains("ERROR: [055]: the reason") && error.ends_with("warning"),
            "{error}"
        );
    }
}
