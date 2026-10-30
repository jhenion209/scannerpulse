//! A listener's star rating of a call's transcript, one per browser. The
//! rating copies the transcript's model and origin as they are when it is
//! given, so a later edit or new transcription does not change what was
//! judged. `rater` is the SHA-256 of the browser's token, never the token.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;

/// The hash a browser's token is stored under.
pub fn rater(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Saves `stars` (1 to 5) for `call` from `rater`, replacing that browser's
/// earlier rating. False when the call has no transcript to rate.
pub async fn set(
    conn: &mut PgConnection,
    call: (i64, DateTime<Utc>),
    rater: &str,
    stars: i16,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "INSERT INTO transcript_ratings (call_id, started_at, rater, stars, model, origin) \
         SELECT call_id, started_at, $3, $4, model, origin FROM transcripts \
         WHERE call_id = $1 AND started_at = $2 \
         ON CONFLICT (call_id, started_at, rater) DO UPDATE \
         SET stars = EXCLUDED.stars, model = EXCLUDED.model, origin = EXCLUDED.origin, \
             updated_at = now()",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(rater)
    .bind(stars)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Removes that browser's rating of `call`; false when it had none.
pub async fn clear(
    conn: &mut PgConnection,
    call: (i64, DateTime<Utc>),
    rater: &str,
) -> sqlx::Result<bool> {
    let done = sqlx::query(
        "DELETE FROM transcript_ratings WHERE call_id = $1 AND started_at = $2 AND rater = $3",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(rater)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The ratings of one model's transcripts of one origin.
#[derive(Debug, PartialEq, sqlx::FromRow)]
pub struct Row {
    /// The model that wrote the text; none for a call no model transcribed.
    pub model: Option<String>,
    pub origin: String,
    pub ratings: i64,
    pub calls: i64,
    pub mean: f64,
    /// Ratings of one star, two stars, and so on up to five.
    pub one: i64,
    pub two: i64,
    pub three: i64,
    pub four: i64,
    pub five: i64,
}

/// The ratings given since `since` (all of them when none), by model and
/// origin: models by name, none last.
pub async fn report(
    conn: &mut PgConnection,
    since: Option<DateTime<Utc>>,
) -> sqlx::Result<Vec<Row>> {
    sqlx::query_as(
        "SELECT model, origin, count(*) AS ratings, \
                count(DISTINCT (call_id, started_at)) AS calls, \
                avg(stars)::float8 AS mean, \
                count(*) FILTER (WHERE stars = 1) AS one, \
                count(*) FILTER (WHERE stars = 2) AS two, \
                count(*) FILTER (WHERE stars = 3) AS three, \
                count(*) FILTER (WHERE stars = 4) AS four, \
                count(*) FILTER (WHERE stars = 5) AS five \
         FROM transcript_ratings \
         WHERE $1::timestamptz IS NULL OR created_at >= $1 \
         GROUP BY model, origin ORDER BY model NULLS LAST, origin",
    )
    .bind(since)
    .fetch_all(conn)
    .await
}
