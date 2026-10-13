use std::process::ExitCode;

use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const USAGE: &str = "usage: scannerpulse db-update [--quiet]\n\n\
    db-update   create or update the database schema (reads DATABASE_URL)";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("db-update") => {
            let quiet = args.iter().any(|a| a == "--quiet");
            match db_update(quiet).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("db-update failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

async fn db_update(quiet: bool) -> Result<(), Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await?;
    let before = applied(&pool).await?;
    MIGRATOR.run(&pool).await?;
    if !quiet {
        let rows =
            sqlx::query("SELECT version, description FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
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
async fn applied(pool: &PgPool) -> sqlx::Result<Vec<i64>> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !exists {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(pool)
        .await
}
