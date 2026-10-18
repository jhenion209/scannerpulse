//! `scannerpulse uninstall`: removes what setup made (the services, their
//! units, the settings, the account and the program) and keeps the archive,
//! the database and the backups unless the person names each to go.

use std::path::{Path, PathBuf};

use inquire::{Confirm, Text};

use super::apply;
use super::steps::{ACCOUNT, Action, DATABASE, ETC, Step, run};
use super::system::System;
use crate::backup::PGBACKREST_CONF;

/// The units setup writes.
pub const UNITS: [&str; 4] = [
    "scannerpulse-web.service",
    "scannerpulse-ingest.service",
    "scannerpulse-backup.timer",
    "scannerpulse-backup.service",
];

/// What the person asked to go besides the services.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Remove {
    pub database: bool,
    pub backups: bool,
    pub archive: bool,
}

/// What this machine has of setup's work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Found {
    /// The units of `UNITS` that are installed.
    pub units: Vec<String>,
    /// pgBackRest's settings for ScannerPulse exist (setup turned on WAL archiving).
    pub pgbackrest: bool,
    /// The scannerpulse account exists.
    pub account: bool,
    /// The folders the database names: the sources and the archive (None
    /// when the database does not answer).
    pub recordings: Vec<PathBuf>,
    pub archive: Option<PathBuf>,
    /// SCANNERPULSE_BACKUPS from the settings file.
    pub backups: Option<PathBuf>,
    /// The site's port, from SCANNERPULSE_WEB_LISTEN in the settings file.
    pub port: Option<u16>,
    /// The folders above that are not there (an unmounted share, a deleted
    /// folder): their access entries cannot be taken off.
    pub gone: Vec<PathBuf>,
    /// The database is this machine's, not one the person runs elsewhere.
    pub local_db: bool,
}

/// Whether `DATABASE_URL` (unset: setup's own) names this machine.
pub fn local_database(url: Option<&str>) -> bool {
    url.is_none_or(|url| {
        reqwest::Url::parse(url).is_ok_and(|u| {
            matches!(
                u.host_str(),
                None | Some("localhost" | "127.0.0.1" | "[::1]" | "")
            )
        })
    })
}

fn postgres(title: &str, sql: &str) -> Step {
    Step {
        title: title.into(),
        action: Action::Run {
            program: "runuser".into(),
            args: [
                "-u",
                "postgres",
                "--",
                "psql",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
            ]
            .map(String::from)
            .to_vec(),
            input: Some(sql.into()),
            env: Vec::new(),
            unless: None,
        },
    }
}

/// The steps that undo setup on `system`, keeping what `remove` does not name.
pub fn steps(found: &Found, remove: Remove, system: &System) -> Result<Vec<Step>, String> {
    if remove.database && !found.local_db {
        return Err("the database is one you run on another machine: uninstall leaves it; drop it there if it should go".into());
    }
    if remove.archive && found.archive.is_none() {
        return Err("the archive's folder is recorded in the database, which does not answer: it cannot be removed this way".into());
    }
    if let (true, Some(archive)) = (remove.archive, &found.archive)
        && found.gone.contains(archive)
    {
        return Err(format!(
            "the archive {} is not there (not mounted?): mount it, or leave out --remove-archive",
            archive.display()
        ));
    }
    if remove.backups && found.backups.is_none() {
        return Err("the settings file does not say where the backups are".into());
    }
    let mut steps = Vec::new();
    let has = |unit: &str| found.units.iter().any(|u| u == unit);
    let running: Vec<&str> = UNITS[..3].iter().copied().filter(|u| has(u)).collect();
    if !running.is_empty() {
        let mut args = vec!["disable", "--now"];
        args.extend(&running);
        steps.push(run(
            "Stop ScannerPulse's services and timer",
            "systemctl",
            &args,
        ));
    }
    if has(UNITS[3]) {
        steps.push(run(
            "Stop a backup under way",
            "systemctl",
            &["stop", UNITS[3]],
        ));
    }
    let files: Vec<String> = UNITS
        .iter()
        .map(|u| format!("/etc/systemd/system/{u}"))
        .collect();
    let mut args = vec!["-f"];
    args.extend(files.iter().map(String::as_str));
    if let Some(port) = found.port {
        steps.push(super::steps::firewall(false, port));
    }
    steps.push(run("Remove the services' units", "rm", &args));
    steps.push(run(
        "Read the services again",
        "systemctl",
        &["daemon-reload"],
    ));
    if found.pgbackrest {
        steps.push(postgres(
            "Stop sending PostgreSQL's WAL to pgBackRest",
            "ALTER SYSTEM RESET archive_mode;\nALTER SYSTEM RESET archive_command;\n",
        ));
        steps.push(run(
            "Restart PostgreSQL 17",
            "systemctl",
            &["restart", system.postgres_service()],
        ));
        steps.push(run(
            "Remove pgBackRest's settings for ScannerPulse",
            "rm",
            &["-f", PGBACKREST_CONF],
        ));
    }
    if remove.database {
        steps.push(postgres(
            "Remove the database and its role",
            &format!(
                "DROP DATABASE IF EXISTS {DATABASE} WITH (FORCE);\nDROP ROLE IF EXISTS {ACCOUNT};\n"
            ),
        ));
    }
    if found.account {
        let mut folders: Vec<&Path> = found.recordings.iter().map(PathBuf::as_path).collect();
        if !remove.archive {
            folders.extend(found.archive.as_deref());
        }
        for folder in folders
            .into_iter()
            .filter(|f| !found.gone.iter().any(|g| g == f))
        {
            let path = folder.display().to_string();
            steps.push(super::steps::setfacl(
                &format!("Take the scannerpulse account's access off {path}"),
                &[
                    "-R",
                    "-x",
                    &format!("u:{ACCOUNT}"),
                    "-x",
                    &format!("d:u:{ACCOUNT}"),
                    &path,
                ],
            ));
        }
    }
    if let (true, Some(backups)) = (remove.backups, &found.backups) {
        steps.push(run(
            "Remove the backups",
            "rm",
            &["-rf", &backups.display().to_string()],
        ));
    }
    if let (true, Some(archive)) = (remove.archive, &found.archive) {
        let a = archive.display().to_string();
        let kept = ".scannerpulse-backups";
        let keep_backups = !remove.backups
            && found
                .backups
                .as_ref()
                .is_some_and(|b| b.starts_with(archive));
        if keep_backups && found.backups.as_deref() != Some(&archive.join(kept)) {
            return Err(format!(
                "the backups {} are inside the archive: name --remove-backups as well, or move them first",
                found
                    .backups
                    .as_ref()
                    .map_or(String::new(), |b| b.display().to_string())
            ));
        }
        steps.push(if keep_backups {
            run(
                "Remove everything in the archive except the backups",
                "find",
                &[
                    &a,
                    "-mindepth",
                    "1",
                    "-maxdepth",
                    "1",
                    "!",
                    "-name",
                    kept,
                    "-exec",
                    "rm",
                    "-rf",
                    "--",
                    "{}",
                    "+",
                ],
            )
        } else {
            run(
                "Remove everything in the archive",
                "find",
                &[&a, "-mindepth", "1", "-delete"],
            )
        });
        if found.account {
            steps.push(super::steps::setfacl(
                "Take the scannerpulse account's access off the archive's folder",
                &[
                    "-x",
                    &format!("u:{ACCOUNT}"),
                    "-x",
                    &format!("d:u:{ACCOUNT}"),
                    &a,
                ],
            ));
        }
    }
    steps.push(run(
        "Remove the settings, the password file and the certificate",
        "rm",
        &["-rf", ETC],
    ));
    if found.account {
        steps.push(run(
            "Remove the scannerpulse account",
            "userdel",
            &["--remove", ACCOUNT],
        ));
    }
    steps.push(run("Remove the program", "rm", &["-f", super::BINARY]));
    Ok(steps)
}

/// What the database says the folders are: the sources, and the primary
/// location; and how many calls it holds.
async fn folders() -> Option<(Vec<PathBuf>, Option<PathBuf>, i64)> {
    use sqlx::Connection;
    let url = std::env::var("DATABASE_URL").ok()?;
    let connect = sqlx::PgConnection::connect(&url);
    let mut conn = tokio::time::timeout(std::time::Duration::from_secs(5), connect)
        .await
        .ok()?
        .ok()?;
    let sources: Vec<String> = sqlx::query_scalar("SELECT path FROM sources ORDER BY id")
        .fetch_all(&mut conn)
        .await
        .ok()?;
    let archive: Option<String> = sqlx::query_scalar("SELECT root FROM locations WHERE is_primary")
        .fetch_optional(&mut conn)
        .await
        .ok()?;
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&mut conn)
        .await
        .ok()?;
    Some((
        sources.into_iter().map(PathBuf::from).collect(),
        archive.map(PathBuf::from),
        calls,
    ))
}

/// The question `--remove-archive` asks before its path is typed.
fn archive_question(archive: &Path, calls: i64) -> String {
    let noun = if calls == 1 { "call" } else { "calls" };
    format!(
        "The archive {} holds {calls} {noun}. --remove-archive deletes them all, with their recordings, and they cannot be got back. Delete them?",
        archive.display()
    )
}

/// Asks the person to type `name` before `what` goes.
fn typed(what: &str, name: &str) -> Result<bool, String> {
    let answer = Text::new(&format!(
        "{what} will be deleted for good. Type {name} to go ahead:"
    ))
    .prompt()
    .map_err(|error| error.to_string())?;
    Ok(answer.trim() == name)
}

/// Undoes setup on this machine, as root.
pub async fn run_uninstall(remove: Remove) -> Result<(), String> {
    if !rustix::process::geteuid().is_root() {
        return Err("uninstall removes services and an account: run it as root (sudo /usr/local/bin/scannerpulse uninstall)".into());
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err("uninstall asks before it removes anything: run it in a terminal (ssh -t for a remote machine)".into());
    }
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    let system = super::system::detect(&os_release, std::env::consts::ARCH)?;
    let (recordings, archive, calls) = folders().await.unwrap_or_default();
    let gone: Vec<PathBuf> = recordings
        .iter()
        .chain(&archive)
        .filter(|folder| !folder.exists())
        .cloned()
        .collect();
    let found = Found {
        units: UNITS
            .iter()
            .filter(|u| Path::new("/etc/systemd/system").join(u).exists())
            .map(|u| u.to_string())
            .collect(),
        pgbackrest: Path::new(PGBACKREST_CONF).exists(),
        account: std::fs::read_to_string("/etc/passwd")
            .is_ok_and(|p| p.lines().any(|l| l.starts_with(&format!("{ACCOUNT}:")))),
        recordings,
        archive,
        backups: std::env::var("SCANNERPULSE_BACKUPS")
            .ok()
            .map(PathBuf::from),
        port: std::env::var("SCANNERPULSE_WEB_LISTEN")
            .ok()
            .and_then(|listen| listen.parse::<std::net::SocketAddr>().ok())
            .map(|listen| listen.port()),
        gone,
        local_db: local_database(std::env::var("DATABASE_URL").ok().as_deref()),
    };
    let steps = steps(&found, remove, &system)?;
    println!("\nUninstall will:");
    for (n, step) in steps.iter().enumerate() {
        println!("  {:>2}. {}", n + 1, step.title);
    }
    if found.account {
        for folder in &found.gone {
            println!(
                "{} is not there, so its access entries stay.",
                folder.display()
            );
        }
    }
    let kept: Vec<String> = [
        (!remove.archive).then(|| {
            found.archive.as_ref().map_or("the archive".into(), |a| {
                format!("the archive {}", a.display())
            })
        }),
        (!remove.database).then(|| {
            if found.local_db {
                format!("the database {DATABASE}")
            } else {
                "the database you run".into()
            }
        }),
        (!remove.backups).then(|| {
            found.backups.as_ref().map_or("the backups".into(), |b| {
                format!("the backups {}", b.display())
            })
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !kept.is_empty() {
        println!("It keeps {}.", kept.join(", "));
    }
    let mut go = Confirm::new("Uninstall ScannerPulse?")
        .with_default(false)
        .prompt()
        .map_err(|error| error.to_string())?;
    if go && remove.database {
        go = typed(&format!("The database {DATABASE}"), DATABASE)?;
    }
    if let (true, true, Some(backups)) = (go, remove.backups, &found.backups) {
        go = typed("The backups", &backups.display().to_string())?;
    }
    if let (true, true, Some(archive)) = (go, remove.archive, &found.archive) {
        go = Confirm::new(&archive_question(archive, calls))
            .with_default(false)
            .prompt()
            .map_err(|error| error.to_string())?
            && typed("Every call in the archive", &archive.display().to_string())?;
    }
    if !go {
        println!("Nothing was changed.");
        return Ok(());
    }
    let context = apply::Context {
        database_url: String::new(),
        db_password: String::new(),
        cert: PathBuf::new(),
    };
    apply::apply(&steps, &context, true).await?;
    println!("\nScannerPulse is uninstalled.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Found, Remove, UNITS, archive_question, steps};
    use crate::setup::steps::{Action, Step};
    use crate::setup::system::detect;
    use std::path::{Path, PathBuf};

    #[test]
    fn removing_the_archive_asks_first_naming_its_folder_and_its_calls() {
        let question = archive_question(Path::new("/srv/archive"), 1200);
        assert!(question.starts_with("The archive /srv/archive holds 1200 calls."));
        assert!(question.contains("--remove-archive deletes them all"));
        assert!(question.ends_with("Delete them?"));
        assert!(archive_question(Path::new("/srv/archive"), 1).contains("holds 1 call."));
    }

    fn found() -> Found {
        Found {
            units: UNITS.map(String::from).to_vec(),
            pgbackrest: true,
            account: true,
            recordings: vec![PathBuf::from("/srv/recordings")],
            archive: Some(PathBuf::from("/srv/archive")),
            backups: Some(PathBuf::from("/srv/archive/.scannerpulse-backups")),
            port: Some(443),
            gone: vec![],
            local_db: true,
        }
    }

    #[test]
    fn a_folder_that_is_not_there_keeps_its_entries_and_cannot_be_removed() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let unmounted = Found {
            gone: vec![
                PathBuf::from("/srv/recordings"),
                PathBuf::from("/srv/archive"),
            ],
            ..found()
        };
        let text = lines(&steps(&unmounted, Remove::default(), &system).unwrap()).join("\n");
        assert!(!text.contains("setfacl"), "{text}");
        assert!(text.contains("userdel --remove scannerpulse"), "{text}");
        let half = Found {
            gone: vec![PathBuf::from("/srv/recordings")],
            ..found()
        };
        let text = lines(&steps(&half, Remove::default(), &system).unwrap()).join("\n");
        assert!(!text.contains("/srv/recordings"), "{text}");
        assert!(text.contains("setfacl -R -x u:scannerpulse -x d:u:scannerpulse /srv/archive"));
        let error = steps(
            &unmounted,
            Remove {
                archive: true,
                ..Remove::default()
            },
            &system,
        )
        .unwrap_err();
        assert_eq!(
            error,
            "the archive /srv/archive is not there (not mounted?): mount it, or leave out --remove-archive"
        );
    }

    #[test]
    fn the_sites_port_is_closed_in_firewalld_when_it_is_known() {
        let system = detect("ID=\"rocky\"\nVERSION_ID=\"9.6\"\n", "x86_64").unwrap();
        let all = steps(&found(), Remove::default(), &system).unwrap();
        let close = all
            .iter()
            .find(|s| s.title == "Close port 443 in the firewall, if firewalld runs")
            .unwrap();
        let Action::Run { program, args, .. } = &close.action else {
            panic!()
        };
        assert_eq!(program, "/bin/sh");
        assert_eq!(
            args,
            &[
                "-c",
                "if systemctl is-active --quiet firewalld; then firewall-cmd --quiet --permanent --remove-port=443/tcp && firewall-cmd --quiet --reload; fi"
            ]
        );
        let unknown = Found {
            port: None,
            ..found()
        };
        let text = lines(&steps(&unknown, Remove::default(), &system).unwrap()).join("\n");
        assert!(!text.contains("firewall"), "{text}");
    }

    fn lines(steps: &[Step]) -> Vec<String> {
        steps
            .iter()
            .map(|s| match &s.action {
                Action::Run {
                    program,
                    args,
                    input,
                    ..
                } => {
                    format!(
                        "{program} {} {}",
                        args.join(" "),
                        input.clone().unwrap_or_default()
                    )
                }
                _ => panic!("uninstall only runs commands"),
            })
            .collect()
    }

    #[test]
    fn a_plain_uninstall_keeps_the_archive_the_database_and_the_backups() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let all = lines(&steps(&found(), Remove::default(), &system).unwrap());
        let text = all.join("\n");
        assert!(text.contains("systemctl disable --now scannerpulse-web.service scannerpulse-ingest.service scannerpulse-backup.timer"));
        assert!(
            text.contains("ALTER SYSTEM RESET archive_command"),
            "{text}"
        );
        assert!(text.contains("setfacl -R -x u:scannerpulse -x d:u:scannerpulse /srv/archive"));
        // A share without POSIX ACLs must not stop the uninstall.
        let access: Vec<&String> = all
            .iter()
            .filter(|l| l.contains("-x u:scannerpulse"))
            .collect();
        assert_eq!(access.len(), 2, "{text}");
        assert!(access.iter().all(|l| l.starts_with("sh -c ")), "{text}");
        assert!(
            !text.contains("DROP DATABASE")
                && !text.contains("rm -rf /srv")
                && !text.contains("-delete"),
            "{text}"
        );
        assert_eq!(
            all.last().unwrap().trim(),
            "rm -f /usr/local/bin/scannerpulse"
        );
    }

    #[test]
    fn each_named_part_goes_and_the_archive_keeps_its_folder() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let all = Remove {
            database: true,
            backups: true,
            archive: true,
        };
        let text = lines(&steps(&found(), all, &system).unwrap()).join("\n");
        assert!(text.contains("DROP DATABASE IF EXISTS scannerpulse WITH (FORCE);"));
        assert!(text.contains("rm -rf /srv/archive/.scannerpulse-backups"));
        assert!(text.contains("find /srv/archive -mindepth 1 -delete"));
        assert!(!text.contains("setfacl -R -x u:scannerpulse -x d:u:scannerpulse /srv/archive"));
    }

    #[test]
    fn removing_the_archive_alone_keeps_the_backups_inside_it() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let only = Remove {
            archive: true,
            ..Remove::default()
        };
        let text = lines(&steps(&found(), only, &system).unwrap()).join("\n");
        assert!(
            text.contains(
                "find /srv/archive -mindepth 1 -maxdepth 1 ! -name .scannerpulse-backups -exec rm -rf -- {} +"
            ),
            "{text}"
        );
        assert!(!text.contains("rm -rf /srv/archive/.scannerpulse-backups"));
        assert!(!text.contains("-mindepth 1 -delete"), "{text}");
        assert!(text.contains("setfacl -x u:scannerpulse -x d:u:scannerpulse /srv/archive"));
    }

    #[test]
    fn backups_elsewhere_inside_the_archive_must_be_named_too() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let moved = Found {
            backups: Some(PathBuf::from("/srv/archive/keep/b")),
            ..found()
        };
        let only = Remove {
            archive: true,
            ..Remove::default()
        };
        let error = steps(&moved, only, &system).unwrap_err();
        assert!(error.contains("--remove-backups"), "{error}");
    }

    #[test]
    fn a_half_done_setup_uninstalls_what_it_has_and_cannot_lose_an_unknown_archive() {
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let half = Found {
            units: vec![],
            pgbackrest: false,
            account: true,
            ..Found::default()
        };
        let text = lines(&steps(&half, Remove::default(), &system).unwrap()).join("\n");
        assert!(
            !text.contains("systemctl disable") && !text.contains("ALTER SYSTEM"),
            "{text}"
        );
        assert!(text.contains("userdel --remove scannerpulse"));
        let error = steps(
            &half,
            Remove {
                archive: true,
                ..Remove::default()
            },
            &system,
        )
        .unwrap_err();
        assert!(error.contains("does not answer"), "{error}");
    }

    #[test]
    fn a_database_you_run_is_never_dropped_by_uninstall() {
        use super::local_database;
        assert!(local_database(None));
        assert!(local_database(Some(
            "postgres://scannerpulse@localhost:5432/scannerpulse"
        )));
        assert!(!local_database(Some(
            "postgres://sp@db.example.org:5433/calls"
        )));
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let theirs = Found {
            local_db: false,
            pgbackrest: false,
            ..found()
        };
        let error = steps(
            &theirs,
            Remove {
                database: true,
                ..Remove::default()
            },
            &system,
        )
        .unwrap_err();
        assert!(error.contains("one you run on another machine"), "{error}");
        assert!(steps(&theirs, Remove::default(), &system).is_ok());
    }
}
