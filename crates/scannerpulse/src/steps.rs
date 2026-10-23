//! A step of the conversion and the jobs that change archive files exclude
//! each other: whichever is started second is refused, naming what runs and
//! who started it. A step holds a session lock, on a connection of its own,
//! while it is carried out (a step that dies frees it), and a row saying
//! what it is. A job of those kinds is created only by a statement that can
//! take the lock shared (`jobs::create`), so neither can slip past the
//! other. Update now and a switchover are not steps.

use std::future::Future;

use sqlx::Connection;
use sqlx::postgres::{PgConnectOptions, PgConnection};

use crate::agent::Kind;
use crate::jobs;

/// The advisory lock's key, two numbers (the imports' first, then this
/// migration's), so it never meets a key made by `hashtext` of what a
/// visitor types: a sign-in locks those.
pub const LOCK: (i32, i32) = (1397048658, 20);

/// A step of the conversion in words.
pub fn words(kind: Kind) -> &'static str {
    match kind {
        Kind::ArchiveCheck => "the check of the archive's copy",
        Kind::ArchiveSwitch => "the archive's switch",
        Kind::DbMove => "the database's move",
        Kind::DbSwitch => "the database's switch",
        Kind::DbRetire => "the old database's removal",
        Kind::Second => "the second machine's addition",
        Kind::Update | Kind::Switchover => "not a step of the conversion",
    }
}

/// The step a job is refused for, in words: what, who started it, where
/// and when. No row when none is recorded.
pub(crate) const RUNNING: &str = "SELECT what || ' (started by ' || started_by || ' on ' || host \
     || ' at ' || to_char(started_at, 'YYYY-MM-DD HH24:MI:SS TZ') || ')' \
     FROM conversion_steps ORDER BY id DESC LIMIT 1";

/// A step being carried out: its lock and its row.
pub struct Step {
    conn: PgConnection,
    id: i64,
}

impl Step {
    /// Begins a step, on a connection of its own. Refused while another
    /// step is carried out, or a job that changes archive files waits or
    /// runs.
    pub async fn begin(
        options: &PgConnectOptions,
        what: &str,
        started_by: &str,
    ) -> Result<Step, String> {
        let mut conn = PgConnection::connect_with(options)
            .await
            .map_err(|error| format!("the database does not answer: {error}"))?;
        let failed = |error: sqlx::Error| format!("the database failed: {error}");
        // A job being created holds the lock shared for its statement: wait
        // for it, then see it.
        sqlx::query("SET lock_timeout = '5s'")
            .execute(&mut conn)
            .await
            .map_err(failed)?;
        let locked = sqlx::query("SELECT pg_advisory_lock($1, $2)")
            .bind(LOCK.0)
            .bind(LOCK.1)
            .execute(&mut conn)
            .await;
        if let Err(error) = locked {
            let busy = matches!(&error, sqlx::Error::Database(db) if db.code().as_deref() == Some("55P03"));
            if !busy {
                return Err(failed(error));
            }
            let running: Option<String> = sqlx::query_scalar(RUNNING)
                .fetch_optional(&mut conn)
                .await
                .map_err(failed)?;
            return Err(format!(
                "{} is being carried out: wait for it to end",
                running.unwrap_or_else(|| "another step of the conversion".into())
            ));
        }
        // Rows left by a step that died: it no longer holds the lock.
        sqlx::query("DELETE FROM conversion_steps")
            .execute(&mut conn)
            .await
            .map_err(failed)?;
        let kinds: Vec<&str> = jobs::Kind::ALL
            .into_iter()
            .filter(|kind| kind.changes_archive())
            .map(jobs::Kind::name)
            .collect();
        let busy: Option<(i64, String, String, String)> = sqlx::query_as(
            "SELECT id, kind, state, started_by FROM jobs \
             WHERE state IN ('queued', 'running') AND kind = ANY($1) ORDER BY id LIMIT 1",
        )
        .bind(&kinds)
        .fetch_optional(&mut conn)
        .await
        .map_err(failed)?;
        if let Some((id, kind, state, by)) = busy {
            let _ = conn.close().await;
            let state = if state == "queued" {
                "waiting"
            } else {
                "running"
            };
            return Err(format!(
                "job {id} ({kind}), started by {by}, is {state}: wait for it to end, or stop it, then try again"
            ));
        }
        let id = sqlx::query_scalar(
            "INSERT INTO conversion_steps (what, host, started_by) VALUES ($1, $2, $3) RETURNING id",
        )
        .bind(what)
        .bind(crate::audit::this_machine())
        .bind(started_by)
        .fetch_one(&mut conn)
        .await
        .map_err(failed)?;
        Ok(Step { conn, id })
    }

    /// Ends the step: its row goes, and its lock with its connection. A
    /// database that went away meanwhile (a switch moves it) already freed
    /// the lock; the row is gone at the next step's start.
    pub async fn end(mut self) {
        let _ = sqlx::query("DELETE FROM conversion_steps WHERE id = $1")
            .bind(self.id)
            .execute(&mut self.conn)
            .await;
        let _ = self.conn.close().await;
    }
}

/// Carries out `step` as a step of the conversion, `what` in words. A step
/// that cannot reach the database runs unguarded, and says so: no job can
/// start or go on without it either.
pub async fn carry_out<T>(
    options: Option<&PgConnectOptions>,
    what: &str,
    started_by: &str,
    said: impl Fn(&str),
    step: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let held = match options {
        Some(options) => match Step::begin(options, what, started_by).await {
            Ok(held) => Some(held),
            Err(why) if why.starts_with("the database does not answer") => {
                said(&format!(
                    "{why}; {what} goes on without checking for jobs that change archive files"
                ));
                None
            }
            Err(why) => return Err(why),
        },
        None => {
            said(&format!(
                "no database to check for jobs that change archive files; {what} goes on"
            ));
            None
        }
    };
    let result = step.await;
    if let Some(held) = held {
        held.end().await;
    }
    result
}
