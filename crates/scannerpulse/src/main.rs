use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use chrono::SecondsFormat;
use scannerpulse::MIGRATOR;
use scannerpulse::lease;
use scannerpulse::localtime::TimeHow;
use scannerpulse::reader::{self, FileCall, ReadError};
use serde_json::json;
use sqlx::postgres::PgConnection;
use sqlx::{Connection, Row};

/// How long a command waits for the database server to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
usage: scannerpulse db-update [--quiet]
       scannerpulse lease
       scannerpulse read --zone ZONE FILE...
       scannerpulse --version
       scannerpulse --help

db-update   create or update the database schema (reads DATABASE_URL)
lease       show which host holds the processing lease, and each host's
            heartbeat; exit 0 when the lease has a valid holder
read        print what ingest reads from each file, one JSON line per file;
            ZONE is the time zone of the times in the file names";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    DbUpdate { quiet: bool },
    Lease,
    Read { zone: String, files: Vec<String> },
    Version,
    Help,
}

/// What the command line asks for; `None` when it is not understood. The words
/// must match exactly: a mistyped flag must not run an update.
fn parse(args: &[&str]) -> Option<Command> {
    match args {
        ["db-update"] => Some(Command::DbUpdate { quiet: false }),
        ["db-update", "--quiet"] => Some(Command::DbUpdate { quiet: true }),
        ["lease"] => Some(Command::Lease),
        ["read", "--zone", zone, files @ ..] if !files.is_empty() => Some(Command::Read {
            zone: zone.to_string(),
            files: files.iter().map(|file| file.to_string()).collect(),
        }),
        ["--version"] => Some(Command::Version),
        ["--help"] => Some(Command::Help),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    // Lossy, so an argument that is not valid UTF-8 gets the usage text, not a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match parse(&args) {
        Some(Command::DbUpdate { quiet }) => match db_update(quiet).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("db-update failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Lease) => lease_status().await,
        Some(Command::Read { zone, files }) => read(&zone, &files),
        Some(Command::Version) => {
            println!("scannerpulse {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// One connection to DATABASE_URL, not a pool: a refused connection fails at
/// once with the real error. An address that never answers would wait for the
/// operating system's own limit, which is minutes, so give up after
/// CONNECT_TIMEOUT.
async fn connect() -> Result<PgConnection, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let conn = tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect(&url))
        .await
        .map_err(|_| {
            format!(
                "the database server did not answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )
        })??;
    Ok(conn)
}

async fn lease_status() -> ExitCode {
    let status = match connect().await {
        Ok(mut conn) => lease::status(&mut conn).await.map_err(Into::into),
        Err(error) => Err(error),
    };
    match status {
        Ok(status) => {
            for line in status.lines() {
                println!("{line}");
            }
            if status.valid() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("lease: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn db_update(quiet: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = connect().await?;
    let before = applied(&mut conn).await?;
    MIGRATOR.run(&mut conn).await?;
    if !quiet {
        let rows =
            sqlx::query("SELECT version, description FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&mut conn)
                .await?;
        for row in &rows {
            let version: i64 = row.get("version");
            let description: String = row.get("description");
            let state = if before.contains(&version) {
                "present"
            } else {
                "applied now"
            };
            println!("{version:04} {description}: {state}");
        }
        println!(
            "schema is up to date ({} applied now)",
            rows.len() - before.len()
        );
    }
    Ok(())
}

/// Prints one JSON line per file. Exits 0 when every file was read.
fn read(zone: &str, files: &[String]) -> ExitCode {
    let Ok(zone) = zone.parse::<chrono_tz::Tz>() else {
        eprintln!("read: {zone:?} is not a known time zone");
        return ExitCode::from(2);
    };
    let mut all_read = true;
    for file in files {
        let line = match reader::read_path(Path::new(file), zone) {
            Ok((call, _)) => call_json(file, &call),
            Err(ReadError::Rejected(why)) => {
                all_read = false;
                json!({"file": file, "rejected": why})
            }
            Err(ReadError::Io(error)) => {
                all_read = false;
                json!({"file": file, "error": error.to_string()})
            }
        };
        println!("{line}");
    }
    if all_read {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn call_json(file: &str, call: &FileCall) -> serde_json::Value {
    let d = &call.described;
    json!({
        "file": file,
        "started_at": call.started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "time_how": match call.time_how {
            TimeHow::Plain => "plain",
            TimeHow::ByMtime => "by_mtime",
            TimeHow::EarlierDefault => "earlier_default",
            TimeHow::Gap => "gap",
        },
        "duration_ms": call.duration_ms,
        "file_mtime": call.file_mtime.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        "system": d.system,
        "site": d.site,
        "talkgroup": d.talkgroup.as_ref().map(|t| t.number),
        "talkgroup_name": d.talkgroup.as_ref().map(|t| &t.name),
        "title": d.title,
        "radio_id": d.radio_id,
        "radio_alias": d.radio_alias,
    })
}

/// Versions already applied; empty when the database has never been updated.
async fn applied(conn: &mut PgConnection) -> sqlx::Result<Vec<i64>> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(&mut *conn)
        .await
}

#[cfg(test)]
mod tests {
    use super::{Command, parse};

    #[test]
    fn db_update_runs_with_or_without_quiet() {
        assert_eq!(
            parse(&["db-update"]),
            Some(Command::DbUpdate { quiet: false })
        );
        assert_eq!(
            parse(&["db-update", "--quiet"]),
            Some(Command::DbUpdate { quiet: true })
        );
    }

    #[test]
    fn version_and_help_stand_alone() {
        assert_eq!(parse(&["--version"]), Some(Command::Version));
        assert_eq!(parse(&["--help"]), Some(Command::Help));
    }

    #[test]
    fn lease_takes_no_argument() {
        assert_eq!(parse(&["lease"]), Some(Command::Lease));
        assert_eq!(parse(&["lease", "--quiet"]), None);
    }

    #[test]
    fn read_takes_a_zone_and_one_file_or_more() {
        assert_eq!(
            parse(&["read", "--zone", "UTC", "a.mp3", "b.mp3"]),
            Some(Command::Read {
                zone: "UTC".to_string(),
                files: vec!["a.mp3".to_string(), "b.mp3".to_string()],
            })
        );
        assert_eq!(parse(&["read", "--zone", "UTC"]), None);
        assert_eq!(parse(&["read", "a.mp3"]), None);
    }

    #[test]
    fn anything_else_is_refused() {
        let refused: [&[&str]; 9] = [
            &[],
            &["nonsense"],
            &["db-update", "--quite"],
            &["db-update", "--help"],
            &["db-update", "--quiet", "--quiet"],
            &["db-update", "extra"],
            &["--quiet"],
            &["--version", "db-update"],
            &["--help", "--version"],
        ];
        for args in refused {
            assert_eq!(parse(args), None, "{args:?}");
        }
    }
}
