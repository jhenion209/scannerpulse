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
