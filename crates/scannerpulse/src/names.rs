//! Names change only with the administrator's approval. A talkgroup name a
//! recorder sends that differs from the stored one is a proposal, approved
//! (the talkgroup is renamed) or dismissed (that spelling is ignored from
//! then on); see `store::talkgroup_id`.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgConnection;

use crate::recorders::Refused;
use crate::talkgroups::{self, Renamed};

/// An open proposal, as the admin page and `scannerpulse names list` show it.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Proposal {
    pub talkgroup_id: i32,
    pub system: String,
    pub number: i64,
    pub current: String,
    pub proposed: String,
    /// The recorders that sent it, by name.
    pub recorders: Vec<String>,
    pub calls: i32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// The open proposals, oldest first. A proposal that names the talkgroup's
/// own name now (renamed by hand meanwhile) is left out.
pub async fn proposals(conn: &mut PgConnection) -> sqlx::Result<Vec<Proposal>> {
    sqlx::query_as(
        "SELECT p.talkgroup_id, y.name AS system, t.number, t.name AS current, p.name AS proposed, \
                ARRAY(SELECT s.name FROM sources s WHERE s.id = ANY (p.source_ids) ORDER BY s.id) AS recorders, \
                p.calls, p.first_seen, p.last_seen \
         FROM name_proposals p \
         JOIN talkgroups t ON t.id = p.talkgroup_id \
         JOIN systems y ON y.id = t.system_id \
         WHERE NOT p.dismissed AND p.name <> t.name \
         ORDER BY p.first_seen, y.name, t.number, p.name",
    )
    .fetch_all(&mut *conn)
    .await
}

/// Approves a proposal: the talkgroup is renamed as a rename by hand does
/// (its files then wait for the correction), the proposal goes, and the name
/// it replaced is dismissed, so that a recorder not yet updated does not
/// propose it back.
pub async fn approve(
    conn: &mut PgConnection,
    talkgroup_id: i32,
    name: &str,
) -> Result<(), Refused> {
    let open: Option<String> = sqlx::query_scalar(
        "SELECT t.name FROM name_proposals p JOIN talkgroups t ON t.id = p.talkgroup_id \
         WHERE p.talkgroup_id = $1 AND p.name = $2 AND NOT p.dismissed",
    )
    .bind(talkgroup_id)
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(old) = open else {
        return Err(Refused::NoSuch(format!(
            "talkgroup {talkgroup_id} has no open proposal {name:?}"
        )));
    };
    if let Some(problem) = talkgroups::name_problem(name) {
        return Err(Refused::Refused(problem));
    }
    match talkgroups::rename(conn, talkgroup_id, name).await? {
        Renamed::Done { .. } | Renamed::Same => {}
        Renamed::Taken(_, written) => {
            return Err(Refused::Refused(format!(
                "talkgroup {written} has that name already: merge into it instead"
            )));
        }
        Renamed::NoSuch => {
            return Err(Refused::NoSuch(format!(
                "there is no talkgroup {talkgroup_id}"
            )));
        }
    }
    sqlx::query("DELETE FROM name_proposals WHERE talkgroup_id = $1 AND name = $2")
        .bind(talkgroup_id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "INSERT INTO name_proposals (talkgroup_id, name, dismissed) VALUES ($1, $2, true) \
         ON CONFLICT (talkgroup_id, name) DO UPDATE SET dismissed = true",
    )
    .bind(talkgroup_id)
    .bind(&old)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Dismisses a proposal: that spelling, for that talkgroup, is ignored from
/// then on.
pub async fn dismiss(
    conn: &mut PgConnection,
    talkgroup_id: i32,
    name: &str,
) -> Result<(), Refused> {
    let done = sqlx::query(
        "UPDATE name_proposals SET dismissed = true \
         WHERE talkgroup_id = $1 AND name = $2 AND NOT dismissed",
    )
    .bind(talkgroup_id)
    .bind(name)
    .execute(&mut *conn)
    .await?;
    if done.rows_affected() == 0 {
        return Err(Refused::NoSuch(format!(
            "talkgroup {talkgroup_id} has no open proposal {name:?}"
        )));
    }
    Ok(())
}

/// A system name files carry that no system has and no spelling maps: its
/// files wait in the archive's staging folder.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Unknown {
    pub system: String,
    pub files: i64,
    /// The recorders that sent its files, by name.
    pub recorders: Vec<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    /// Some of its talkgroups, `<number> <name>`.
    pub talkgroups: Vec<String>,
}

/// The most talkgroups listed for an unknown system.
pub const SAMPLE_TALKGROUPS: i64 = 5;

/// The unknown system names, oldest first.
pub async fn unknown_systems(conn: &mut PgConnection) -> sqlx::Result<Vec<Unknown>> {
    sqlx::query_as(
        "SELECT f.staged_system AS system, count(*) AS files, \
                ARRAY(SELECT DISTINCT s.name FROM ingest_files g JOIN sources s ON s.id = g.source_id \
                      WHERE g.state = 'staged' AND g.staged_system = f.staged_system ORDER BY s.name) AS recorders, \
                min(f.first_seen) AS first_seen, max(f.seen_at) AS last_seen, \
                ARRAY(SELECT DISTINCT g.staged_talkgroup FROM ingest_files g \
                      WHERE g.state = 'staged' AND g.staged_system = f.staged_system \
                        AND g.staged_talkgroup IS NOT NULL ORDER BY g.staged_talkgroup LIMIT $1) AS talkgroups \
         FROM ingest_files f \
         WHERE f.state = 'staged' \
           AND NOT EXISTS (SELECT 1 FROM systems y WHERE y.name = f.staged_system) \
           AND NOT EXISTS (SELECT 1 FROM system_spellings y WHERE y.name = f.staged_system) \
         GROUP BY f.staged_system ORDER BY min(f.first_seen)",
    )
    .bind(SAMPLE_TALKGROUPS)
    .fetch_all(&mut *conn)
    .await
}

/// How many decisions wait: unknown system names and open proposals.
pub async fn waiting_count(conn: &mut PgConnection) -> sqlx::Result<i64> {
    Ok(unknown_systems(conn).await?.len() as i64 + proposals(conn).await?.len() as i64)
}

/// Refused unless `name` is an unknown system name that files wait for.
async fn staged_name(conn: &mut PgConnection, name: &str) -> Result<(), Refused> {
    if unknown_systems(conn)
        .await?
        .iter()
        .any(|u| u.system == name)
    {
        Ok(())
    } else {
        Err(Refused::NoSuch(format!(
            "no file waits for a system named {name:?}"
        )))
    }
}

/// Names a new system after the unknown name: its files are filed at the
/// filer's next pass.
pub async fn new_system(conn: &mut PgConnection, name: &str) -> Result<(), Refused> {
    staged_name(conn, name).await?;
    sqlx::query("INSERT INTO systems (name) VALUES ($1) ON CONFLICT (name) DO NOTHING")
        .bind(name)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Maps the unknown name to the existing system `to` for good: its files,
/// and every later file that names it, are filed under `to`.
pub async fn map_system(conn: &mut PgConnection, name: &str, to: &str) -> Result<(), Refused> {
    staged_name(conn, name).await?;
    let target: Option<i32> = sqlx::query_scalar("SELECT id FROM systems WHERE name = $1")
        .bind(to)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(target) = target else {
        return Err(Refused::Refused(format!("there is no system named {to:?}")));
    };
    sqlx::query("INSERT INTO system_spellings (name, system_id) VALUES ($1, $2)")
        .bind(name)
        .bind(target)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Deletes the staged files of the unknown name, once the name is typed
/// exactly as `confirm`; their rows say they were discarded. Returns how many
/// were. Files already gone from the staging folder count too.
pub async fn discard(
    conn: &mut PgConnection,
    name: &str,
    confirm: &str,
    archive: &std::path::Path,
) -> Result<i64, Refused> {
    staged_name(conn, name).await?;
    if confirm != name {
        return Err(Refused::Refused(format!(
            "type the system's name, {name:?}, exactly to discard its files"
        )));
    }
    let files: Vec<(i64, i16, String)> = sqlx::query_as(
        "SELECT id, source_id, rel_path FROM ingest_files WHERE state = 'staged' AND staged_system = $1",
    )
    .bind(name)
    .fetch_all(&mut *conn)
    .await?;
    let mut discarded = 0;
    for (id, source_id, rel_path) in files {
        let path = crate::archive::staging_path(archive, source_id, &rel_path);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(Refused::Refused(format!(
                    "{} cannot be deleted ({error}); {discarded} were",
                    path.display()
                )));
            }
        }
        sqlx::query(
            "UPDATE ingest_files SET state = 'rejected', \
             reason = 'discarded: its system ' || $2 || ' was not wanted' \
             WHERE id = $1 AND state = 'staged'",
        )
        .bind(id)
        .bind(name)
        .execute(&mut *conn)
        .await?;
        discarded += 1;
    }
    Ok(discarded)
}
