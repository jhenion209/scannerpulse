//! Evaluation sets: calls held out to measure transcription against the text
//! people saved for them.
//!
//! A set is drawn from a list of talkgroups, as evenly as their traffic allows,
//! and inside each talkgroup as evenly across the months as the calls allow:
//! a month that recorded far more calls than the others would otherwise make
//! up most of the set. Only calls the machine has transcribed are drawn, so
//! every call of a set has a machine output to score.

use std::fmt;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use sqlx::postgres::PgConnection;
use sqlx::{Connection, PgExecutor};

/// What a draw asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draw {
    pub set: String,
    /// Each talkgroup as its system's name and its number, in the list's order.
    pub talkgroups: Vec<(String, i64)>,
    /// How many calls the set should hold.
    pub calls: usize,
    /// The calls of the last `days` days are drawn.
    pub days: u32,
    /// The shortest call drawn.
    pub min_ms: i32,
    /// The same seed draws the same calls from the same database.
    pub seed: i64,
}

/// How one talkgroup took part in a draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TalkgroupDraw {
    pub system: String,
    pub number: i64,
    /// The calls that could have been drawn.
    pub eligible: usize,
    pub drawn: usize,
    /// When it is off, the talkgroup's new calls get no machine output.
    pub transcribe: bool,
}

/// What a draw did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drawn {
    pub talkgroups: Vec<TalkgroupDraw>,
    /// The calls of the set's earlier draw, which this one replaced.
    pub replaced: usize,
}

impl Drawn {
    pub fn total(&self) -> usize {
        self.talkgroups.iter().map(|t| t.drawn).sum()
    }
}

#[derive(Debug)]
pub enum EvalError {
    /// No talkgroup has this system and number.
    UnknownTalkgroup(String),
    /// People have labeled calls of the set: replacing it would make every
    /// score before and after it incomparable.
    Labeled {
        set: String,
        labeled: i64,
    },
    Database(sqlx::Error),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::UnknownTalkgroup(name) => write!(f, "{name} is not a known talkgroup"),
            EvalError::Labeled { set, labeled } => write!(
                f,
                "the set {set} is not replaced: {labeled} of its calls have a person's text"
            ),
            EvalError::Database(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for EvalError {}

impl From<sqlx::Error> for EvalError {
    fn from(error: sqlx::Error) -> EvalError {
        EvalError::Database(error)
    }
}

/// The calls that may be drawn: long enough, transcribed by the machine, and
/// recent enough. `$1` is the talkgroup, `$2` the earliest start and `$3` the
/// shortest length.
const ELIGIBLE: &str = "FROM calls \
     WHERE talkgroup_id = $1 AND started_at >= $2 AND duration_ms >= $3 \
       AND transcript_state IN ('text', 'no_speech')";

/// Draws the set, replacing an earlier draw of it unless people have labeled
/// its calls. `now` ends the window; the months are those of `zone`.
pub async fn draw(
    conn: &mut PgConnection,
    draw: &Draw,
    now: DateTime<Utc>,
    zone: Tz,
) -> Result<Drawn, EvalError> {
    let from = now - chrono::Duration::days(draw.days.into());
    let mut tx = conn.begin().await?;
    let mut found = Vec::new();
    for (system, number) in &draw.talkgroups {
        let row: Option<(i32, bool)> = sqlx::query_as(
            "SELECT t.id, t.transcribe FROM talkgroups t JOIN systems s ON s.id = t.system_id \
             WHERE s.name = $1 AND t.number = $2",
        )
        .bind(system)
        .bind(number)
        .fetch_optional(&mut *tx)
        .await?;
        let (id, transcribe) =
            row.ok_or_else(|| EvalError::UnknownTalkgroup(format!("{system}/{number}")))?;
        let months: Vec<(String, i64)> = sqlx::query_as(&format!(
            "SELECT to_char(started_at AT TIME ZONE $4, 'YYYY-MM') AS month, count(*) \
             {ELIGIBLE} GROUP BY 1 ORDER BY 1"
        ))
        .bind(id)
        .bind(from)
        .bind(draw.min_ms)
        .bind(zone.name())
        .fetch_all(&mut *tx)
        .await?;
        found.push((id, transcribe, months));
    }
    let totals: Vec<usize> = found
        .iter()
        .map(|(_, _, months)| months.iter().map(|(_, n)| *n as usize).sum())
        .collect();
    let quotas = even_split(draw.calls, &totals);
    // A person's save waits until this draw commits, so a set cannot gain a label between the check and the replacement.
    sqlx::query("LOCK TABLE transcript_labels IN SHARE MODE")
        .execute(&mut *tx)
        .await?;
    let labeled: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT (e.call_id, e.started_at)) FROM eval_calls e \
         JOIN transcript_labels l ON l.call_id = e.call_id AND l.started_at = e.started_at \
         WHERE e.set_name = $1",
    )
    .bind(&draw.set)
    .fetch_one(&mut *tx)
    .await?;
    if labeled > 0 {
        return Err(EvalError::Labeled {
            set: draw.set.clone(),
            labeled,
        });
    }
    let replaced = sqlx::query("DELETE FROM eval_calls WHERE set_name = $1")
        .bind(&draw.set)
        .execute(&mut *tx)
        .await?
        .rows_affected() as usize;
    let mut talkgroups = Vec::new();
    for (((id, transcribe, months), quota), (system, number)) in
        found.iter().zip(&quotas).zip(&draw.talkgroups)
    {
        let sizes: Vec<usize> = months.iter().map(|(_, n)| *n as usize).collect();
        let mut drawn = 0;
        for ((month, _), take) in months.iter().zip(even_split(*quota, &sizes)) {
            if take == 0 {
                continue;
            }
            // In the order of a hash of the seed and the call: random enough,
            // and the same seed draws the same calls.
            drawn += sqlx::query(&format!(
                "INSERT INTO eval_calls (set_name, call_id, started_at, drawn_at) \
                 SELECT $5, id, started_at, $8 {ELIGIBLE} \
                   AND to_char(started_at AT TIME ZONE $4, 'YYYY-MM') = $6 \
                 ORDER BY md5($7::text || ':' || id::text) LIMIT $9"
            ))
            .bind(id)
            .bind(from)
            .bind(draw.min_ms)
            .bind(zone.name())
            .bind(&draw.set)
            .bind(month)
            .bind(draw.seed)
            .bind(now)
            .bind(take as i64)
            .execute(&mut *tx)
            .await?
            .rows_affected() as usize;
        }
        talkgroups.push(TalkgroupDraw {
            system: system.clone(),
            number: *number,
            eligible: sizes.iter().sum(),
            drawn,
            transcribe: *transcribe,
        });
    }
    tx.commit().await?;
    Ok(Drawn {
        talkgroups,
        replaced,
    })
}

/// Splits `n` among pools of the given sizes as evenly as the sizes allow. A
/// pool smaller than its share gives all it has, and the others share the rest.
pub fn even_split(n: usize, sizes: &[usize]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i]);
    let mut shares = vec![0; sizes.len()];
    let mut left = n;
    for (done, &i) in order.iter().enumerate() {
        let share = left.div_ceil(sizes.len() - done);
        shares[i] = share.min(sizes[i]);
        left -= shares[i];
    }
    shares
}

/// A call of a set that no person has labeled yet: a line of the labelers' list.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ToLabel {
    pub call_id: i64,
    pub started_at: DateTime<Utc>,
    pub system: String,
    pub number: i64,
    pub talkgroup: String,
    pub duration_ms: Option<i32>,
}

/// The labelers' list: the set's calls that have no person's text, oldest first.
pub async fn to_label(db: impl PgExecutor<'_>, set: &str) -> sqlx::Result<Vec<ToLabel>> {
    sqlx::query_as(
        "SELECT c.id AS call_id, c.started_at, s.name AS system, t.number, t.name AS talkgroup, \
                c.duration_ms \
         FROM eval_calls e \
         JOIN calls c ON c.id = e.call_id AND c.started_at = e.started_at \
         JOIN talkgroups t ON t.id = c.talkgroup_id JOIN systems s ON s.id = t.system_id \
         WHERE e.set_name = $1 AND NOT EXISTS \
           (SELECT 1 FROM transcript_labels l \
            WHERE l.call_id = e.call_id AND l.started_at = e.started_at) \
         ORDER BY c.started_at, c.id",
    )
    .bind(set)
    .fetch_all(db)
    .await
}

/// How many calls the set holds.
pub async fn set_size(db: impl PgExecutor<'_>, set: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar("SELECT count(*) FROM eval_calls WHERE set_name = $1")
        .bind(set)
        .fetch_one(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::even_split;

    #[test]
    fn a_small_pool_gives_all_it_has_and_the_others_share_the_rest() {
        assert_eq!(even_split(9, &[100, 100, 100]), vec![3, 3, 3]);
        assert_eq!(even_split(10, &[100, 2, 100]), vec![4, 2, 4]);
        assert_eq!(even_split(10, &[3, 2, 1]), vec![3, 2, 1]);
        assert_eq!(even_split(0, &[5]), vec![0]);
        assert_eq!(even_split(5, &[]), Vec::<usize>::new());
        assert_eq!(even_split(10, &[100, 100, 100]).iter().sum::<usize>(), 10);
    }
}
