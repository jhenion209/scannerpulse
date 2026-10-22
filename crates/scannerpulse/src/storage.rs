//! Storage locations: the main archive, where new calls are filed, and the
//! folders an administrator adds beside it to hold calls moved out of it.
//! Each is a row of `locations`; a call's file is its location's root joined
//! with its `rel_path`. A location an administrator adds holds a marker file,
//! so that a share that is not mounted (an empty mount point) is never taken
//! for an empty location. Each host's ingest checks every location once a
//! minute, and a move into one waits until every running host can use it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, PgExecutor};

pub use crate::recorders::Refused;

/// The file that marks a folder as the location it was added as.
pub const MARKER: &str = ".scannerpulse-location";

/// How often each host's ingest checks every location.
pub const CHECK_EVERY: Duration = Duration::from_secs(60);

/// A host's word on a location older than this, in seconds, is no word: its
/// ingest has stopped checking.
pub const STALE_SECONDS: f64 = 300.0;

/// The files whose sizes are read to tell the space a location's calls use.
pub const SAMPLED: usize = 40;

/// A location as the admin page and `scannerpulse storage list` show it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Location {
    pub id: i16,
    pub name: String,
    pub root: String,
    /// `managed`, or `readonly` for an older archive imported where it is.
    pub mode: String,
    /// The main archive: new calls are filed here.
    pub primary: bool,
    /// What each host's ingest last found there.
    pub hosts: Vec<Seen>,
}

/// What one host last found in a location.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Seen {
    pub host: String,
    pub ok: bool,
    pub problem: Option<String>,
    pub free_bytes: Option<i64>,
    pub total_bytes: Option<i64>,
    pub checked_at: DateTime<Utc>,
}

/// A location's calls: how many, the first and the last, and a sample of
/// their files to tell the space they use.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Counted {
    pub location_id: i16,
    pub calls: i64,
    pub first_call: Option<DateTime<Utc>>,
    pub last_call: Option<DateTime<Utc>>,
    pub sample: Vec<String>,
}

/// Why `folder` cannot be a location beside the folders already `taken`
/// (each with what it is, as a person says it), or None. A location holds
/// files that are moved and deleted, so it shares no folder with another use.
pub fn folder_problem(folder: &Path, taken: &[(String, PathBuf)]) -> Option<String> {
    taken
        .iter()
        .find(|(_, other)| folder.starts_with(other) || other.starts_with(folder))
        .map(|(what, other)| {
            format!(
                "{} and {what}, {}, must not be the same folder or inside each other",
                folder.display(),
                other.display()
            )
        })
}

/// The folder with links resolved, once this process has found that it can
/// list it and write in it, with the marker written there; or why not.
pub fn check_here(folder: &Path, name: &str) -> Result<PathBuf, String> {
    if !folder.is_absolute() {
        return Err(format!(
            "{} is not a full path: start it with /",
            folder.display()
        ));
    }
    let meta = std::fs::metadata(folder).map_err(|_| {
        format!(
            "{} does not exist on this machine: mount the disk or share there first",
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
    std::fs::write(
        real.join(MARKER),
        format!("The ScannerPulse storage location {name}. Keep this file: without it the location counts as not mounted.\n"),
    )
    .map_err(|error| {
        format!(
            "{} cannot be written: {error}; calls are moved into it",
            real.display()
        )
    })?;
    Ok(real)
}

/// The free and the total bytes of a location's disk, or why a host cannot
/// use it.
pub type Found = Result<(u64, u64), String>;

/// What `host` finds in a location now: the free and the total bytes of its
/// disk, once it has listed the folder, found its marker (`marked`: every
/// location but the main archive has one) and written and removed a file
/// there; or why it cannot use it.
pub fn probe(root: &Path, marked: bool, host: &str) -> Found {
    std::fs::read_dir(root)
        .map_err(|error| format!("{} cannot be listed: {error}", root.display()))?;
    if marked && !root.join(MARKER).is_file() {
        return Err(format!(
            "{} has no {MARKER} file: the disk or share is not mounted there",
            root.display()
        ));
    }
    let reach = root.join(format!(".scannerpulse-reach-{host}"));
    std::fs::write(&reach, b"")
        .and_then(|()| std::fs::remove_file(&reach))
        .map_err(|error| format!("{} cannot be written: {error}", root.display()))?;
    let stats =
        rustix::fs::statvfs(root).map_err(|error| format!("{}: {error}", root.display()))?;
    Ok((
        stats.f_bavail.saturating_mul(stats.f_frsize),
        stats.f_blocks.saturating_mul(stats.f_frsize),
    ))
}

/// What `host` finds in each location now ([`probe`]), each in a thread of
/// its own for `limit` at most: a hung share holds up no other location, and
/// is not read again until its read has ended.
pub async fn check_all(
    locations: Vec<(i16, PathBuf, bool)>,
    host: &str,
    hung: &mut crate::watcher::Hung,
    limit: Duration,
) -> Vec<(i16, Found)> {
    let ids: Vec<i16> = locations.iter().map(|(id, ..)| *id).collect();
    let jobs = locations
        .into_iter()
        .map(|(_, root, marked)| {
            let (path, host) = (root.clone(), host.to_string());
            let work: crate::watcher::Job<Found> =
                Box::new(move || Ok(probe(&path, marked, &host)));
            (root, work)
        })
        .collect();
    ids.into_iter()
        .zip(crate::watcher::run_bounded(jobs, hung, limit).await)
        .map(|(id, found)| (id, found.unwrap_or_else(|error| Err(error.to_string()))))
        .collect()
}

/// The space the `calls` of a location use, about: the mean size of the
/// files of `sample` that can be read, times the calls. None when none can.
pub fn bytes_about(root: &Path, sample: &[String], calls: i64) -> Option<i64> {
    let sizes: Vec<u64> = sample
        .iter()
        .filter_map(|rel| std::fs::metadata(root.join(rel)).ok())
        .map(|meta| meta.len())
        .collect();
    if sizes.is_empty() {
        return None;
    }
    let mean = sizes.iter().sum::<u64>() as f64 / sizes.len() as f64;
    Some((mean * calls as f64).round() as i64)
}

/// Every folder a new location must stay apart from: the locations, the
/// recorders' folders, the purge's holding folder and `backups`.
async fn taken(
    conn: &mut PgConnection,
    backups: Option<&Path>,
) -> sqlx::Result<Vec<(String, PathBuf)>> {
    let locations: Vec<(String, String, bool)> =
        sqlx::query_as("SELECT name, root, is_primary FROM locations ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    let recorders: Vec<(String, String)> =
        sqlx::query_as("SELECT name, path FROM sources WHERE removed_at IS NULL ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    let holding: Option<String> =
        sqlx::query_scalar("SELECT value #>> '{}' FROM settings WHERE key = 'purge.holding_path'")
            .fetch_optional(&mut *conn)
            .await?;
    let mut taken: Vec<(String, PathBuf)> = locations
        .into_iter()
        .map(|(name, root, primary)| {
            let what = if primary {
                "the main archive".to_string()
            } else {
                format!("the location {name}")
            };
            (what, PathBuf::from(root))
        })
        .collect();
    taken.extend(
        recorders
            .into_iter()
            .map(|(name, path)| (format!("the folder of the recorder {name}"), path.into())),
    );
    taken.extend(holding.map(|path| ("the purge's holding folder".to_string(), path.into())));
    taken.extend(backups.map(|path| ("the backups' folder".to_string(), path.to_path_buf())));
    Ok(taken)
}

/// Why a location named `name` cannot be added at `folder`, as it is
/// written, before the folder is touched: [`add`] checks it again once its
/// links are resolved.
pub async fn addable(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    backups: Option<&Path>,
) -> Result<(), Refused> {
    if let Some(problem) = crate::recorders::name_problem(name) {
        return Err(Refused::Refused(problem));
    }
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM locations WHERE name = $1)")
            .bind(name)
            .fetch_one(&mut *conn)
            .await?;
    if exists {
        return Err(Refused::Refused(format!(
            "a location named {name} exists already"
        )));
    }
    let primary: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM locations WHERE is_primary)")
            .fetch_one(&mut *conn)
            .await?;
    if !primary {
        return Err(Refused::Refused(
            "there is no archive yet: run scannerpulse setup first".into(),
        ));
    }
    if folder.is_absolute()
        && let Some(problem) = folder_problem(folder, &taken(conn, backups).await?)
    {
        return Err(Refused::Refused(problem));
    }
    Ok(())
}

/// Adds a location calls can be moved into. `check` finds the folder usable,
/// resolves its links and writes its marker ([`check_here`] in the web
/// service, which runs as the service account). `backups` is this machine's
/// backups folder, when it has one.
pub async fn add(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    backups: Option<&Path>,
    check: impl FnOnce(&Path) -> Result<PathBuf, String>,
) -> Result<i16, Refused> {
    addable(conn, name, folder, backups).await?;
    let taken = taken(conn, backups).await?;
    // Checked again once its links are resolved.
    let real = check(folder).map_err(Refused::Refused)?;
    if let Some(problem) = folder_problem(&real, &taken) {
        return Err(Refused::Refused(problem));
    }
    let id = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode) VALUES ($1, $2, 'managed') RETURNING id",
    )
    .bind(name)
    .bind(real.display().to_string())
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// The id of the location named `name`.
pub async fn find(conn: &mut PgConnection, name: &str) -> Result<i16, Refused> {
    sqlx::query_scalar("SELECT id FROM locations WHERE name = $1")
        .bind(name)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(|| Refused::NoSuch(format!("there is no location named {name}")))
}

/// Removes a location that holds no call: its files, if any are left, stay
/// where they are. Returns its name.
pub async fn remove(conn: &mut PgConnection, id: i16) -> Result<String, Refused> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    let name = match removable(&mut tx, id).await? {
        Ok(name) => name,
        // Rolled back at once: a dropped transaction would hold the
        // location's row locked until its connection is used again.
        Err(why) => {
            tx.rollback().await?;
            return Err(why);
        }
    };
    sqlx::query("DELETE FROM location_health WHERE location_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM locations WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(name)
}

/// The location's name, locked for its removal, or why it cannot go.
async fn removable(conn: &mut PgConnection, id: i16) -> sqlx::Result<Result<String, Refused>> {
    let found: Option<(String, bool)> =
        sqlx::query_as("SELECT name, is_primary FROM locations WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((name, primary)) = found else {
        return Ok(Err(Refused::NoSuch(format!("there is no location {id}"))));
    };
    if primary {
        return Ok(Err(Refused::Refused(format!(
            "{name} is the main archive, where new calls are filed: it cannot be removed"
        ))));
    }
    let job: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind = 'move' AND state IN ('queued', 'running') \
           AND (params ->> 'to')::smallint = $1 ORDER BY id LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(job) = job {
        return Ok(Err(Refused::Refused(format!(
            "job {job} moves calls into {name}: stop it first"
        ))));
    }
    let holds: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM calls WHERE location_id = $1)")
            .bind(id)
            .fetch_one(&mut *conn)
            .await?;
    if holds {
        return Ok(Err(Refused::Refused(format!(
            "{name} still holds calls: move them to another location first"
        ))));
    }
    let leftovers: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM archive_leftovers WHERE location_id = $1)",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    if leftovers {
        return Ok(Err(Refused::Refused(format!(
            "the files moved out of {name} are still being deleted: try again in a minute"
        ))));
    }
    Ok(Ok(name))
}

/// Every location, the main archive first, with what each host last found
/// there.
pub async fn list(conn: &mut PgConnection) -> sqlx::Result<Vec<Location>> {
    let rows: Vec<(i16, String, String, String, bool)> = sqlx::query_as(
        "SELECT id, name, root, mode, is_primary FROM locations ORDER BY is_primary DESC, id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let seen: Vec<(i16, Seen)> = sqlx::query_as::<
        _,
        (
            i16,
            String,
            bool,
            Option<String>,
            Option<i64>,
            Option<i64>,
            DateTime<Utc>,
        ),
    >(
        "SELECT location_id, host, ok, problem, free_bytes, total_bytes, checked_at \
         FROM location_health ORDER BY host",
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(
        |(id, host, ok, problem, free_bytes, total_bytes, checked_at)| {
            (
                id,
                Seen {
                    host,
                    ok,
                    problem,
                    free_bytes,
                    total_bytes,
                    checked_at,
                },
            )
        },
    )
    .collect();
    Ok(rows
        .into_iter()
        .map(|(id, name, root, mode, primary)| Location {
            id,
            name,
            root,
            mode,
            primary,
            hosts: seen
                .iter()
                .filter(|(of, _)| *of == id)
                .map(|(_, seen)| seen.clone())
                .collect(),
        })
        .collect())
}

/// Every location's calls, counted: this reads every call, seconds on a
/// large archive, so the admin page keeps the answer a while.
pub async fn counts(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<Counted>> {
    sqlx::query_as(&format!(
        "SELECT location_id, count(*) AS calls, min(started_at) AS first_call, \
                max(started_at) AS last_call, \
                coalesce((array_agg(rel_path) FILTER (WHERE id % 499 = 0))[1:{SAMPLED}], \
                         ARRAY[min(rel_path), max(rel_path)]) AS sample \
         FROM calls GROUP BY location_id"
    ))
    .fetch_all(db)
    .await
}

/// What `host` found in each location: its disk's free and total bytes, or
/// why it cannot use it.
pub async fn record_health(
    db: impl PgExecutor<'_>,
    host: &str,
    found: &[(i16, Found)],
) -> sqlx::Result<()> {
    let ids: Vec<i16> = found.iter().map(|(id, _)| *id).collect();
    let problems: Vec<Option<String>> = found
        .iter()
        .map(|(_, f)| f.as_ref().err().cloned())
        .collect();
    let bytes = |pick: fn(&(u64, u64)) -> u64| -> Vec<Option<i64>> {
        found
            .iter()
            .map(|(_, f)| {
                f.as_ref()
                    .ok()
                    .map(|b| i64::try_from(pick(b)).unwrap_or(i64::MAX))
            })
            .collect()
    };
    sqlx::query(
        "INSERT INTO location_health (host, location_id, ok, problem, free_bytes, total_bytes, checked_at) \
         SELECT $1, u.id, u.problem IS NULL, u.problem, u.free, u.total, now() \
         FROM unnest($2::smallint[], $3::text[], $4::bigint[], $5::bigint[]) AS u (id, problem, free, total) \
         WHERE EXISTS (SELECT 1 FROM locations l WHERE l.id = u.id) \
         ON CONFLICT (host, location_id) DO UPDATE SET ok = EXCLUDED.ok, problem = EXCLUDED.problem, \
             free_bytes = EXCLUDED.free_bytes, total_bytes = EXCLUDED.total_bytes, \
             checked_at = EXCLUDED.checked_at",
    )
    .bind(host)
    .bind(ids)
    .bind(problems)
    .bind(bytes(|b| b.0))
    .bind(bytes(|b| b.1))
    .execute(db)
    .await?;
    Ok(())
}

/// The locations each host's ingest checks: id, root and whether it holds a
/// marker.
pub async fn to_check(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<(i16, PathBuf, bool)>> {
    let rows: Vec<(i16, String, bool)> =
        sqlx::query_as("SELECT id, root, NOT is_primary FROM locations ORDER BY id")
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(id, root, marked)| (id, root.into(), marked))
        .collect())
}

/// Why calls cannot be moved into the location `name` (`id`) now: a running
/// host cannot use it, has not checked it, or stopped checking it. Hosts
/// whose ingest runs are those whose heartbeat has not expired.
pub async fn unusable(
    conn: &mut PgConnection,
    id: i16,
    name: &str,
) -> sqlx::Result<Option<String>> {
    let running: Vec<String> = crate::lease::status(&mut *conn)
        .await?
        .heartbeats
        .into_iter()
        .filter(|beat| beat.expires_in > 0.0)
        .map(|beat| beat.host)
        .collect();
    if running.is_empty() {
        return Ok(Some(
            "no ingest runs: the location is checked by every running host first".into(),
        ));
    }
    let seen: Vec<(String, bool, Option<String>, f64)> = sqlx::query_as(
        "SELECT host, ok, problem, EXTRACT(EPOCH FROM now() - checked_at)::float8 \
         FROM location_health WHERE location_id = $1",
    )
    .bind(id)
    .fetch_all(&mut *conn)
    .await?;
    for host in running {
        match seen.iter().find(|(h, ..)| *h == host) {
            None => {
                return Ok(Some(format!(
                    "{host} has not checked {name} yet: wait a minute"
                )));
            }
            Some((_, false, problem, _)) => {
                return Ok(Some(format!(
                    "{host} cannot use {name}: {}",
                    problem.as_deref().unwrap_or("it cannot be read")
                )));
            }
            Some((_, true, _, age)) if *age > STALE_SECONDS => {
                return Ok(Some(format!(
                    "{host} last checked {name} {} minutes ago",
                    (*age / 60.0).round()
                )));
            }
            Some(_) => {}
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_location_stays_apart_from_every_folder_in_use() {
        let taken = vec![
            (
                "the main archive".to_string(),
                PathBuf::from("/srv/archive"),
            ),
            (
                "the folder of the recorder east".to_string(),
                PathBuf::from("/srv/in/east"),
            ),
        ];
        assert_eq!(folder_problem(Path::new("/srv/cold"), &taken), None);
        assert_eq!(folder_problem(Path::new("/srv/archive-2"), &taken), None);
        for folder in [
            "/srv/archive",
            "/srv/archive/cold",
            "/srv",
            "/srv/in/east/x",
            "/srv/in",
        ] {
            let why = folder_problem(Path::new(folder), &taken).unwrap();
            assert!(
                why.contains("must not be the same folder"),
                "{folder}: {why}"
            );
        }
    }

    #[test]
    fn a_location_is_checked_written_and_marked() {
        let dir = tempfile::tempdir().unwrap();
        let real = check_here(dir.path(), "Cold").unwrap();
        assert!(real.join(MARKER).is_file());
        let (free, total) = probe(&real, true, "host-a").unwrap();
        assert!(free > 0 && total >= free);
        assert!(!real.join(".scannerpulse-reach-host-a").exists());
        assert!(
            check_here(Path::new("cold"), "Cold")
                .unwrap_err()
                .contains("full path")
        );
        assert!(
            check_here(&dir.path().join("none"), "Cold")
                .unwrap_err()
                .contains("does not exist")
        );
    }

    #[test]
    fn a_location_without_its_marker_is_not_mounted() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            probe(dir.path(), true, "host-a")
                .unwrap_err()
                .contains("not mounted")
        );
        // The main archive has no marker.
        assert!(probe(dir.path(), false, "host-a").is_ok());
    }

    #[test]
    fn the_space_used_is_the_mean_file_times_the_calls() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.mp3"), vec![0; 100]).unwrap();
        std::fs::write(dir.path().join("b.mp3"), vec![0; 300]).unwrap();
        let sample = [
            "a.mp3".to_string(),
            "b.mp3".to_string(),
            "gone.mp3".to_string(),
        ];
        assert_eq!(bytes_about(dir.path(), &sample, 1000), Some(200_000));
        assert_eq!(bytes_about(dir.path(), &["gone.mp3".to_string()], 10), None);
    }
}
