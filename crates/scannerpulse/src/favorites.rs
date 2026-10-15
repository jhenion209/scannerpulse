//! The named favorite lists of talkgroups that every visitor can pick. Admins
//! set and remove them; the command line and the admin page share these.

use std::collections::BTreeSet;
use std::fmt;

use sqlx::Connection;
use sqlx::postgres::PgConnection;

/// The longest list name, in characters.
pub const MAX_NAME: usize = 100;
/// The most talkgroups one list holds.
pub const MAX_TALKGROUPS: usize = 1000;

#[derive(Debug)]
pub enum FavoriteError {
    BadName(String),
    TooMany,
    /// Talkgroup ids that do not exist.
    UnknownTalkgroups(Vec<i32>),
    NoSuch(String),
    Database(sqlx::Error),
}

impl fmt::Display for FavoriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FavoriteError::BadName(why) => f.write_str(why),
            FavoriteError::TooMany => write!(f, "a list holds at most {MAX_TALKGROUPS} talkgroups"),
            FavoriteError::UnknownTalkgroups(ids) => write!(f, "no talkgroup has the id {ids:?}"),
            FavoriteError::NoSuch(name) => write!(f, "there is no list named {name:?}"),
            FavoriteError::Database(error) => write!(f, "the database failed: {error}"),
        }
    }
}

impl std::error::Error for FavoriteError {}

impl From<sqlx::Error> for FavoriteError {
    fn from(error: sqlx::Error) -> FavoriteError {
        FavoriteError::Database(error)
    }
}

fn check_name(name: &str) -> Result<(), FavoriteError> {
    let length = name.chars().count();
    if name.trim() != name || length == 0 || length > MAX_NAME || name.chars().any(char::is_control)
    {
        return Err(FavoriteError::BadName(format!(
            "a list's name is 1 to {MAX_NAME} characters, with no control character \
             and no space at either end"
        )));
    }
    Ok(())
}

/// Creates the list `name`, or replaces its talkgroups, with `talkgroups`.
pub async fn set(
    conn: &mut PgConnection,
    name: &str,
    talkgroups: &[i32],
) -> Result<(), FavoriteError> {
    check_name(name)?;
    let talkgroups: Vec<i32> = talkgroups
        .iter()
        .copied()
        .collect::<BTreeSet<i32>>()
        .into_iter()
        .collect();
    if talkgroups.len() > MAX_TALKGROUPS {
        return Err(FavoriteError::TooMany);
    }
    let mut tx = conn.begin().await?;
    let unknown: Vec<i32> = sqlx::query_scalar(
        "SELECT u.wanted FROM unnest($1::int[]) AS u(wanted) \
         WHERE NOT EXISTS (SELECT 1 FROM talkgroups t WHERE t.id = u.wanted) ORDER BY u.wanted",
    )
    .bind(&talkgroups)
    .fetch_all(&mut *tx)
    .await?;
    if !unknown.is_empty() {
        return Err(FavoriteError::UnknownTalkgroups(unknown));
    }
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO favorites (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(name)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM favorite_talkgroups WHERE favorite_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO favorite_talkgroups (favorite_id, talkgroup_id) \
         SELECT $1, unnest($2::int[])",
    )
    .bind(id)
    .bind(&talkgroups)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn remove(db: impl sqlx::PgExecutor<'_>, name: &str) -> Result<(), FavoriteError> {
    let done = sqlx::query("DELETE FROM favorites WHERE name = $1")
        .bind(name)
        .execute(db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(FavoriteError::NoSuch(name.to_string()));
    }
    Ok(())
}
