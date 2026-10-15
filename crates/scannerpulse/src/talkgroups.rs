//! The talkgroups and their transcription switches, for the admin page's
//! Transcription tab and the `talkgroup` command.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Talkgroup {
    pub id: i32,
    pub system: String,
    pub number: i64,
    pub name: String,
    /// New calls of the talkgroup are transcribed.
    pub transcribe: bool,
}

/// Every talkgroup, by system and number.
pub async fn list(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<Talkgroup>> {
    sqlx::query_as(
        "SELECT t.id, s.name AS system, t.number, t.name, t.transcribe \
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
