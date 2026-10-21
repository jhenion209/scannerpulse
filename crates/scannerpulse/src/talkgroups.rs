//! The talkgroups: their transcription switches (the admin page's
//! Transcription tab), their names and the talkgroups whose files still carry
//! an old name (the Data tab), and the `talkgroup` commands.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Talkgroup {
    pub id: i32,
    pub system: String,
    pub number: i64,
    pub name: String,
    /// New calls of the talkgroup are transcribed.
    pub transcribe: bool,
    /// Its archived files carry its name: false after a rename, until the
    /// correction job has run.
    pub files_correct: bool,
}

/// Every talkgroup, by system and number.
pub async fn list(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<Talkgroup>> {
    sqlx::query_as(
        "SELECT t.id, s.name AS system, t.number, t.name, t.transcribe, t.files_correct \
         FROM talkgroups t JOIN systems s ON s.id = t.system_id \
         ORDER BY s.name, t.number",
    )
    .fetch_all(db)
    .await
}

/// Turns a talkgroup's transcription on or off; false when there is no
/// such talkgroup. Only new calls follow the switch: past calls are queued
/// by a queue job.
pub async fn set_transcribe(
    db: impl sqlx::PgExecutor<'_>,
    id: i32,
    on: bool,
) -> sqlx::Result<bool> {
    let done = sqlx::query("UPDATE talkgroups SET transcribe = $2 WHERE id = $1")
        .bind(id)
        .bind(on)
        .execute(db)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// The talkgroup written as SYSTEM/NUMBER.
pub async fn find(
    db: impl sqlx::PgExecutor<'_>,
    system: &str,
    number: i64,
) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar(
        "SELECT t.id FROM talkgroups t JOIN systems s ON s.id = t.system_id \
         WHERE s.name = $1 AND t.number = $2",
    )
    .bind(system)
    .bind(number)
    .fetch_optional(db)
    .await
}

/// The longest name a talkgroup may be given.
pub const MAX_NAME: usize = 200;

/// What a rename did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Renamed {
    /// The name is changed. `files_correct` is false when the talkgroup has
    /// calls, whose files still carry the old name.
    Done {
        files_correct: bool,
    },
    /// The talkgroup has that name already.
    Same,
    /// Another talkgroup of the same system has that name: merge into it
    /// instead. Its id and its SYSTEM/NUMBER.
    Taken(i32, String),
    NoSuch,
}

/// Why a name cannot be a talkgroup's, or `None` when it can. The recorder's
/// files carry the name in a quoted title and in their folder, so a quote, a
/// control character or a name of only spaces cannot round-trip.
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
    if name.chars().any(|c| c == '"' || c.is_control()) {
        return Some("the name holds a double quote or a control character".into());
    }
    None
}

/// Renames a talkgroup. Its calls show the new name at once; their files keep
/// the old one, remembered in `files_name`, until an administrator starts the
/// correction. The rename lasts: a recorder that still sends another name
/// makes it a proposal (`names`).
pub async fn rename(conn: &mut sqlx::PgConnection, id: i32, name: &str) -> sqlx::Result<Renamed> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    let found: Option<(i32, String)> =
        sqlx::query_as("SELECT system_id, name FROM talkgroups WHERE id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((system_id, old)) = found else {
        return Ok(Renamed::NoSuch);
    };
    // Two renames in one system at once must not both take a name: they
    // queue on the system.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('scannerpulse talkgroup names'), $1)")
        .bind(system_id)
        .execute(&mut *tx)
        .await?;
    if old == name {
        return Ok(Renamed::Same);
    }
    let taken: Option<(i32, String)> = sqlx::query_as(
        "SELECT t.id, s.name || '/' || t.number FROM talkgroups t JOIN systems s ON s.id = t.system_id \
         WHERE t.system_id = $1 AND t.name = $2 AND t.id <> $3 ORDER BY t.number LIMIT 1",
    )
    .bind(system_id)
    .bind(name)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((other, written)) = taken {
        return Ok(Renamed::Taken(other, written));
    }
    let files_correct: bool = sqlx::query_scalar(
        "UPDATE talkgroups SET name = $2, \
           files_name = CASE WHEN files_correct AND EXISTS (SELECT 1 FROM calls WHERE talkgroup_id = $1) THEN $3
                             WHEN coalesce(files_name, '') = '' THEN $3 ELSE files_name END, \
           files_correct = files_correct AND NOT EXISTS (SELECT 1 FROM calls WHERE talkgroup_id = $1) \
         WHERE id = $1 RETURNING files_correct",
    )
    .bind(id)
    .bind(name)
    .bind(&old)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Renamed::Done { files_correct })
}

/// A talkgroup whose files still carry an old name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Uncorrected {
    pub id: i32,
    pub system: String,
    pub number: i64,
    pub name: String,
    /// The first old name its files carry.
    pub files_name: Option<String>,
    pub calls: i64,
}

/// The talkgroups whose files are not yet corrected, by system and number.
pub async fn uncorrected(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<Uncorrected>> {
    sqlx::query_as(
        "SELECT t.id, s.name AS system, t.number, t.name, t.files_name, \
                (SELECT count(*) FROM calls c WHERE c.talkgroup_id = t.id) AS calls \
         FROM talkgroups t JOIN systems s ON s.id = t.system_id \
         WHERE NOT t.files_correct ORDER BY s.name, t.number",
    )
    .fetch_all(db)
    .await
}
