use std::process::ExitCode;
use std::time::Duration;

use sqlx::postgres::PgConnection;
use sqlx::{Connection, Row};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// How long `db-update` waits for the database server to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
usage: scannerpulse db-update [--quiet]
       scannerpulse --version
       scannerpulse --help

db-update   create or update the database schema (reads DATABASE_URL)";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    DbUpdate { quiet: bool },
    Version,
    Help,
}

/// What the command line asks for; `None` when it is not understood. The words
/// must match exactly: a mistyped flag must not run an update.
fn parse(args: &[&str]) -> Option<Command> {
    match args {
        ["db-update"] => Some(Command::DbUpdate { quiet: false }),
        ["db-update", "--quiet"] => Some(Command::DbUpdate { quiet: true }),
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

async fn db_update(quiet: bool) -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    // One connection, not a pool: a refused connection fails at once with the real
    // error. An address that never answers would wait for the operating system's own
    // limit, which is minutes, so give up after CONNECT_TIMEOUT.
    let mut conn = tokio::time::timeout(CONNECT_TIMEOUT, PgConnection::connect(&url))
        .await
        .map_err(|_| {
            format!(
                "the database server did not answer within {} seconds",
                CONNECT_TIMEOUT.as_secs()
            )
        })??;
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
