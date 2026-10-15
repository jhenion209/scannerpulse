use std::io::Write;
use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, SecondsFormat};
use scannerpulse::MIGRATOR;
use scannerpulse::backfill::{self, Ask};
use scannerpulse::localtime::TimeHow;
use scannerpulse::reader::{self, FileCall, ReadError};
use scannerpulse::{ingest, lease, settings};
use serde_json::json;
use sqlx::postgres::PgConnection;
use sqlx::{Connection, Row};

/// How long a command waits for the database server to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
usage: scannerpulse db-update [--quiet]
       scannerpulse ingest
       scannerpulse lease
       scannerpulse read --zone ZONE FILE...
       scannerpulse transcribe --talkgroup SYSTEM/NUMBER --from DAY --to DAY [--yes]
       scannerpulse --version
       scannerpulse --help

db-update   create or update the database schema (reads DATABASE_URL)
ingest      watch the sources and file their calls into the archive, sharing
            the work with the other host through the lease, and transcribe
            them when transcription is on (reads DATABASE_URL,
            SCANNERPULSE_HOST, SCANNERPULSE_PREFERRED, and
            SCANNERPULSE_TRANSCRIPTION_API_KEY for servers that need a key)
lease       show which host holds the processing lease, and each host's
            heartbeat; exit 0 when the lease has a valid holder
read        print what ingest reads from each file, one JSON line per file;
            ZONE is the time zone of the times in the file names
transcribe  queue the past calls of a talkgroup for transcription, from DAY to
            DAY (YYYY-MM-DD, local days, both included): the calls never tried
            and at least the minimum length; asks first unless --yes";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    DbUpdate {
        quiet: bool,
    },
    Ingest,
    Lease,
    Read {
        zone: String,
        files: Vec<String>,
    },
    Transcribe {
        talkgroup: String,
        from: String,
        to: String,
        yes: bool,
    },
    Version,
    Help,
}

/// What the command line asks for; `None` when it is not understood. The words
/// must match exactly: a mistyped flag must not run an update.
fn parse(args: &[&str]) -> Option<Command> {
    match args {
        ["db-update"] => Some(Command::DbUpdate { quiet: false }),
        ["db-update", "--quiet"] => Some(Command::DbUpdate { quiet: true }),
        ["ingest"] => Some(Command::Ingest),
        ["lease"] => Some(Command::Lease),
        ["read", "--zone", zone, files @ ..] if !files.is_empty() => Some(Command::Read {
            zone: zone.to_string(),
            files: files.iter().map(|file| file.to_string()).collect(),
        }),
        [
            "transcribe",
            "--talkgroup",
            talkgroup,
            "--from",
            from,
            "--to",
            to,
            rest @ ..,
        ] if matches!(rest, [] | ["--yes"]) => Some(Command::Transcribe {
            talkgroup: talkgroup.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            yes: !rest.is_empty(),
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
        Some(Command::Ingest) => match ingest::Config::from_env() {
            Ok(config) => {
                let code = match ingest::run(config, shutdown()).await {
                    Ok(()) => 0,
                    Err(error) => {
                        scannerpulse::log::error(format!("ingest cannot run: {error}"));
                        1
                    }
                };
                // Exit now: dropping the runtime would wait for every blocking
                // thread, and a probe stuck on a hung mount never ends.
                std::process::exit(code)
            }
            Err(error) => {
                eprintln!("ingest: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Lease) => lease_status().await,
        Some(Command::Read { zone, files }) => read(&zone, &files),
        Some(Command::Transcribe {
            talkgroup,
            from,
            to,
            yes,
        }) => match transcribe(&talkgroup, &from, &to, yes).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("transcribe: {error}");
                ExitCode::FAILURE
            }
        },
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

/// Completes at Ctrl-C or SIGTERM, which systemd sends at a stop.
async fn shutdown() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
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

/// Queues the past calls of a talkgroup, with a count first and progress.
async fn transcribe(
    talkgroup: &str,
    from: &str,
    to: &str,
    yes: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (system, number) = talkgroup
        .rsplit_once('/')
        .and_then(|(system, number)| Some((system, number.parse::<i64>().ok()?)))
        .filter(|(system, _)| !system.is_empty())
        .ok_or("write the talkgroup as SYSTEM/NUMBER")?;
    let day = |text: &str| {
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map_err(|_| format!("{text} is not a day such as 2031-01-15"))
    };
    let ask = Ask {
        system: system.to_string(),
        number,
        from: day(from)?,
        to: day(to)?,
    };
    let mut conn = connect().await?;
    let zone = settings::load(&mut conn).await?.time_zone;
    let transcription = settings::load_transcription(&mut conn)
        .await?
        .ok_or("transcription is off: turn it on first")?;
    let min_ms = transcription.min_duration_ms;
    let plan = backfill::plan(&mut conn, &ask, zone, min_ms).await?;
    println!(
        "{} calls of {talkgroup} from {from} to {to} were never tried and are at least {min_ms} ms long",
        plan.count
    );
    if !plan.talkgroup_on {
        println!("note: transcription is off for this talkgroup, so its new calls are not queued");
    }
    if plan.count == 0 {
        return Ok(());
    }
    if !yes && !confirm(&format!("queue {} calls? [y/N] ", plan.count))? {
        println!("nothing was queued");
        return Ok(());
    }
    let started = Instant::now();
    let total = plan.count;
    let queued = backfill::run(&mut conn, &plan.past, 1000, |n| {
        let rate = n as f64 / started.elapsed().as_secs_f64().max(0.001);
        println!("queued {n} of {total} ({rate:.0} calls/s)");
    })
    .await?;
    println!(
        "done: {queued} calls queued in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Asks a yes-or-no question on the terminal; anything but y or yes is no.
fn confirm(question: &str) -> std::io::Result<bool> {
    print!("{question}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
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
    fn ingest_and_lease_take_no_argument() {
        assert_eq!(parse(&["ingest"]), Some(Command::Ingest));
        assert_eq!(parse(&["ingest", "--now"]), None);
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
    fn transcribe_takes_a_talkgroup_two_days_and_maybe_yes() {
        let args = [
            "transcribe",
            "--talkgroup",
            "SYSA/127",
            "--from",
            "2031-01-01",
            "--to",
            "2031-01-31",
        ];
        let want = |yes| Command::Transcribe {
            talkgroup: "SYSA/127".to_string(),
            from: "2031-01-01".to_string(),
            to: "2031-01-31".to_string(),
            yes,
        };
        assert_eq!(parse(&args), Some(want(false)));
        let mut with_yes = args.to_vec();
        with_yes.push("--yes");
        assert_eq!(parse(&with_yes), Some(want(true)));
        with_yes.push("--yes");
        assert_eq!(parse(&with_yes), None);
        assert_eq!(parse(&args[..5]), None);
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
