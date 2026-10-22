//! Recorders: the SDRTrunk folders ingest takes calls from, each a row of
//! `sources`. A recorder is watched (its files are taken as they arrive),
//! paused, or taken in when asked (its files wait in its folder until an
//! administrator brings them in). Removing one stops taking its files; its
//! row stays, so that its calls stay credited to it.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use chrono::{DateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use sqlx::PgConnection;

/// The longest name a recorder may have, in characters.
pub const MAX_NAME: usize = 40;

/// How a recorder's files are taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Watched,
    Paused,
    Asked,
}

/// A recorder as the admin page and `scannerpulse recorder list` show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Recorder {
    pub id: i16,
    pub name: String,
    pub folder: String,
    pub state: State,
    /// Taken in when asked, asked to bring its files in, and not every file
    /// of that request filed yet.
    pub bringing_in: bool,
    /// When its newest file was first seen.
    pub last_file: Option<DateTime<Utc>>,
    /// Its files waiting to be filed.
    pub waiting: i64,
    /// Its files filed, rejected, waiting in the staging folder, and
    /// deleted as copies of calls another recorder filed, since the start of
    /// the day.
    pub filed_today: i64,
    pub rejected_today: i64,
    pub staged_today: i64,
    pub duplicates_today: i64,
    /// What each host's watcher last found in its folder.
    pub hosts: Vec<FolderHealth>,
}

/// What one host's watcher last found in a recorder's folder.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct FolderHealth {
    pub host: String,
    pub ok: bool,
    pub problem: Option<String>,
    /// The audio files in the folder, when it could be listed.
    pub files: Option<i32>,
    pub checked_at: DateTime<Utc>,
}

/// A change to a recorder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Take its files as they arrive.
    Watch,
    /// Take its files only when asked.
    Ask,
    /// Stop taking a watched recorder's files for now.
    Pause,
    /// Take the files a recorder taken in when asked holds now.
    BringIn,
    /// Stop taking its files for good; its calls stay.
    Remove,
}

impl FromStr for Change {
    type Err = String;

    fn from_str(word: &str) -> Result<Change, String> {
        match word {
            "watch" => Ok(Change::Watch),
            "ask" => Ok(Change::Ask),
            "pause" => Ok(Change::Pause),
            "bring-in" => Ok(Change::BringIn),
            "remove" => Ok(Change::Remove),
            other => Err(format!("{other:?} is not a change to a recorder")),
        }
    }
}

/// Why a recorder was not added or changed.
#[derive(Debug)]
pub enum Refused {
    /// It cannot be done as asked; the reason is for the person.
    Refused(String),
    /// There is no such recorder.
    NoSuch(String),
    Database(sqlx::Error),
}

impl From<sqlx::Error> for Refused {
    fn from(error: sqlx::Error) -> Refused {
        Refused::Database(error)
    }
}

impl std::error::Error for Refused {}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::Refused(why) | Refused::NoSuch(why) => f.write_str(why),
            Refused::Database(error) => write!(f, "the database failed: {error}"),
        }
    }
}

/// When the day of `now` began in `zone`: the start of the counts of the
/// day. A day whose midnight a change of clocks skips begins at the first
/// moment it has.
pub fn start_of_day(now: DateTime<Utc>, zone: Tz) -> DateTime<Utc> {
    let day = now.with_timezone(&zone).date_naive();
    (0..=3)
        .find_map(|hour| {
            zone.from_local_datetime(&day.and_hms_opt(hour, 0, 0)?)
                .earliest()
        })
        .map_or(now, |start| start.with_timezone(&Utc))
}

/// Why `name` cannot name a recorder, or None.
pub fn name_problem(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        return Some("the name is empty".into());
    }
    if name != name.trim() {
        return Some("the name starts or ends with a space".into());
    }
    if name.chars().count() > MAX_NAME {
        return Some(format!("the name is longer than {MAX_NAME} characters"));
    }
    if name.chars().any(|c| c == '"' || c == '/' || c.is_control()) {
        return Some("the name holds a double quote, a slash or a control character".into());
    }
    None
}

/// Why `folder` cannot be a recorder's folder beside the archive and the
/// other recorders' folders, all with links resolved, or None. Filing deletes
/// what it takes from a recorder's folder, so no folder may hold another.
pub fn folder_problem(
    folder: &Path,
    archive: &Path,
    others: &[(String, PathBuf)],
) -> Option<String> {
    let apart = |other: &Path| !folder.starts_with(other) && !other.starts_with(folder);
    if !apart(archive) {
        return Some(format!(
            "{} and the archive {} must not be the same folder or inside each other",
            folder.display(),
            archive.display()
        ));
    }
    others.iter().find(|(_, other)| !apart(other)).map(|(name, other)| {
        format!(
            "{} and the folder of the recorder {name}, {}, must not be the same folder or inside each other",
            folder.display(),
            other.display()
        )
    })
}

/// The folder with links resolved, once this process has found it to be a
/// folder it can list and write in (filing deletes the files it takes), or
/// why not.
pub fn check_here(folder: &Path) -> Result<PathBuf, String> {
    if !folder.is_absolute() {
        return Err(format!(
            "{} is not a full path: start it with /",
            folder.display()
        ));
    }
    let meta = std::fs::metadata(folder).map_err(|_| {
        format!(
            "{} does not exist on this machine: mount the recorder's share there first",
            folder.display()
        )
    })?;
    if !meta.is_dir() {
        return Err(format!("{} is a file, not a folder", folder.display()));
    }
    let real =
        std::fs::canonicalize(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    std::fs::read_dir(&real)
        .map_err(|error| format!("{} cannot be listed: {error}", real.display()))?;
    let probe = real.join(format!(".scannerpulse-reach-{}", std::process::id()));
    std::fs::write(&probe, b"")
        .and_then(|()| std::fs::remove_file(&probe))
        .map_err(|error| {
            format!(
                "{} cannot be written: {error}; ScannerPulse deletes the files it takes from a recorder's folder",
                real.display()
            )
        })?;
    Ok(real)
}

/// Run by root on the command line: whether `account` (the service account,
/// which ingest runs as) can list `folder` and write in it, or why not.
pub fn reach_as(account: &str, folder: &Path) -> Result<(), String> {
    let output = std::process::Command::new("runuser")
        .args(["-u", account, "--", "sh", "-c"])
        .arg(
            "cd -- \"$1\" && ls >/dev/null && touch .scannerpulse-reach && rm -f .scannerpulse-reach",
        )
        .arg("sh")
        .arg(folder)
        .output()
        .map_err(|error| format!("cannot run runuser: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "the {account} account cannot list or write {0}: let it in, as setup lets it into its own folders (setfacl -R -m u:{account}:rwX -m d:u:{account}:rwX {0}), or through the share's own rights, then try again",
        folder.display()
    ))
}

/// Adds a recorder, watched or taken in when asked. `check` finds the folder
/// usable and resolves its links ([`check_here`] in the web service, which
/// runs as the service account). Every running ingest is told.
pub async fn add(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    asked: bool,
    check: impl FnOnce(&Path) -> Result<PathBuf, String>,
) -> Result<i16, Refused> {
    if let Some(problem) = name_problem(name) {
        return Err(Refused::Refused(problem));
    }
    let taken: Option<bool> =
        sqlx::query_scalar("SELECT removed_at IS NOT NULL FROM sources WHERE name = $1")
            .bind(name)
            .fetch_optional(&mut *conn)
            .await?;
    match taken {
        Some(false) => {
            return Err(Refused::Refused(format!(
                "a recorder named {name} exists already"
            )));
        }
        Some(true) => {
            return Err(Refused::Refused(format!(
                "a removed recorder was named {name}, and its calls keep the name: choose another"
            )));
        }
        None => {}
    }
    let real = check(folder).map_err(Refused::Refused)?;
    let archive: Option<String> = sqlx::query_scalar("SELECT root FROM locations WHERE is_primary")
        .fetch_optional(&mut *conn)
        .await?;
    let archive = archive.ok_or_else(|| {
        Refused::Refused("there is no archive yet: run scannerpulse setup first".into())
    })?;
    let others: Vec<(String, String)> =
        sqlx::query_as("SELECT name, path FROM sources WHERE removed_at IS NULL ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    let others: Vec<(String, PathBuf)> = others
        .into_iter()
        .map(|(name, path)| (name, PathBuf::from(path)))
        .collect();
    if let Some(problem) = folder_problem(&real, Path::new(&archive), &others) {
        return Err(Refused::Refused(problem));
    }
    let id: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path, enabled, intake) \
         VALUES ($1, 'sdrtrunk', $2, $3, $4) RETURNING id",
    )
    .bind(name)
    .bind(real.display().to_string())
    .bind(!asked)
    .bind(if asked { "asked" } else { "watched" })
    .fetch_one(&mut *conn)
    .await?;
    crate::ingest::request_rescan(&mut *conn).await?;
    Ok(id)
}

/// The id of the recorder named `name`, unless it was removed.
pub async fn find(conn: &mut PgConnection, name: &str) -> Result<i16, Refused> {
    sqlx::query_scalar("SELECT id FROM sources WHERE name = $1 AND removed_at IS NULL")
        .bind(name)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| Refused::NoSuch(format!("there is no recorder named {name}")))
}

/// Makes one change to a recorder and tells every running ingest. Removing
/// one also forgets its files that were waiting: they stay in its folder.
pub async fn change(conn: &mut PgConnection, id: i16, change: Change) -> Result<(), Refused> {
    let intake: Option<String> =
        sqlx::query_scalar("SELECT intake FROM sources WHERE id = $1 AND removed_at IS NULL")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some(intake) = intake else {
        return Err(Refused::NoSuch(format!("there is no recorder {id}")));
    };
    let asked = intake == "asked";
    match change {
        Change::Watch => {
            sqlx::query("UPDATE sources SET intake = 'watched', enabled = true WHERE id = $1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
        Change::Ask => {
            sqlx::query("UPDATE sources SET intake = 'asked', enabled = false WHERE id = $1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
        Change::Pause if asked => {
            return Err(Refused::Refused(
                "a recorder taken in when asked takes nothing until it is asked: there is nothing to pause"
                    .into(),
            ));
        }
        Change::Pause => {
            sqlx::query("UPDATE sources SET enabled = false WHERE id = $1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
        Change::BringIn if !asked => {
            return Err(Refused::Refused(
                "a watched recorder's files are taken as they arrive: switch it to \"when asked\" first"
                    .into(),
            ));
        }
        Change::BringIn => {
            sqlx::query("UPDATE sources SET bring_in_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *conn)
                .await?;
        }
        Change::Remove => {
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            sqlx::query("UPDATE sources SET enabled = false, removed_at = now() WHERE id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM ingest_files WHERE source_id = $1 AND state = 'waiting'")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM source_health WHERE source_id = $1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
    }
    crate::ingest::request_rescan(&mut *conn).await?;
    Ok(())
}

type Row = (
    i16,
    String,
    String,
    bool,
    String,
    bool,
    Option<DateTime<Utc>>,
    i64,
    i64,
    i64,
    i64,
    i64,
);

type HealthRow = (
    i16,
    String,
    bool,
    Option<String>,
    Option<i32>,
    DateTime<Utc>,
);

/// Every recorder but the removed ones, with its counts since `since` (the
/// start of the day in the site's time zone) and its folder's health.
pub async fn list(conn: &mut PgConnection, since: DateTime<Utc>) -> sqlx::Result<Vec<Recorder>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT s.id, s.name, s.path, s.enabled, s.intake, \
                s.bring_in_at IS NOT NULL AND s.brought_in_at IS DISTINCT FROM s.bring_in_at, \
                (SELECT max(f.first_seen) FROM ingest_files f WHERE f.source_id = s.id), \
                (SELECT count(*) FROM ingest_files f WHERE f.source_id = s.id AND f.state = 'waiting'), \
                (SELECT count(*) FROM ingest_files f \
                 WHERE f.source_id = s.id AND f.first_seen >= $1 AND f.state = 'done'), \
                (SELECT count(*) FROM ingest_files f \
                 WHERE f.source_id = s.id AND f.first_seen >= $1 AND f.state = 'rejected'), \
                (SELECT count(*) FROM ingest_files f \
                 WHERE f.source_id = s.id AND f.first_seen >= $1 AND f.state = 'staged'), \
                (SELECT count(*) FROM ingest_files f \
                 WHERE f.source_id = s.id AND f.first_seen >= $1 AND f.state = 'duplicate') \
         FROM sources s WHERE s.removed_at IS NULL ORDER BY s.id",
    )
    .bind(since)
    .fetch_all(&mut *conn)
    .await?;
    let health: Vec<HealthRow> = sqlx::query_as(
        "SELECT source_id, host, ok, problem, files, checked_at FROM source_health ORDER BY host",
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                folder,
                enabled,
                intake,
                asked_pending,
                last_file,
                waiting,
                filed,
                rejected,
                staged,
                duplicates,
            )| {
                let state = match (intake.as_str(), enabled) {
                    ("asked", _) => State::Asked,
                    (_, true) => State::Watched,
                    (_, false) => State::Paused,
                };
                Recorder {
                    id,
                    name,
                    folder,
                    state,
                    bringing_in: state == State::Asked && (asked_pending || waiting > 0),
                    last_file,
                    waiting,
                    filed_today: filed,
                    rejected_today: rejected,
                    staged_today: staged,
                    duplicates_today: duplicates,
                    hosts: health
                        .iter()
                        .filter(|row| row.0 == id)
                        .map(|(_, host, ok, problem, files, checked_at)| FolderHealth {
                            host: host.clone(),
                            ok: *ok,
                            problem: problem.clone(),
                            files: *files,
                            checked_at: *checked_at,
                        })
                        .collect(),
                }
            },
        )
        .collect())
}
