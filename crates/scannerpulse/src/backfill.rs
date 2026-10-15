//! `scannerpulse transcribe`: queues the past calls of one talkgroup that were
//! never tried. The admin page's "also transcribe past calls" uses the same
//! functions.

use chrono::{NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use sqlx::postgres::PgConnection;

use crate::queue::{self, Past};

/// What the command line asks for: a talkgroup by its system's name and its
/// number, and the local days from `from` to `to`, both included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub system: String,
    pub number: i64,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

/// The work that `run` would do.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub past: Past,
    /// The talkgroup's own transcription switch.
    pub talkgroup_on: bool,
    /// The calls to queue.
    pub count: i64,
}

/// Finds the talkgroup and counts its calls to queue: never tried, not
/// queued, at least `min_ms` long.
pub async fn plan(
    conn: &mut PgConnection,
    ask: &Ask,
    zone: Tz,
    min_ms: i32,
) -> Result<Plan, String> {
    if ask.to < ask.from {
        return Err(format!("{} is before {}", ask.to, ask.from));
    }
    let found: Option<(i32, bool)> = sqlx::query_as(
        "SELECT t.id, t.transcribe FROM talkgroups t JOIN systems s ON s.id = t.system_id \
         WHERE s.name = $1 AND t.number = $2",
    )
    .bind(&ask.system)
    .bind(ask.number)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| error.to_string())?;
    let Some((talkgroup_id, talkgroup_on)) = found else {
        return Err(format!(
            "there is no talkgroup {}/{}",
            ask.system, ask.number
        ));
    };
    let past = Past {
        talkgroup_id: Some(talkgroup_id),
        from: Some(start_of(ask.from, zone)?),
        until: Some(start_of(
            ask.to.succ_opt().ok_or("the last day is too far")?,
            zone,
        )?),
        min_ms,
    };
    let count = queue::count_past(&mut *conn, &past)
        .await
        .map_err(|error| error.to_string())?;
    Ok(Plan {
        past,
        talkgroup_on,
        count,
    })
}

/// Queues the calls in batches of `batch`, newest first; tells `progress` the
/// total after each batch. Returns how many were queued.
pub async fn run(
    conn: &mut PgConnection,
    past: &Past,
    batch: i64,
    mut progress: impl FnMut(u64),
) -> sqlx::Result<u64> {
    let mut queued = 0;
    let mut before = None;
    loop {
        let (n, oldest) = queue::queue_past(&mut *conn, past, batch, before).await?;
        if n == 0 {
            return Ok(queued);
        }
        before = oldest;
        queued += n;
        progress(queued);
    }
}

/// The first instant of a local day. A day that starts in a spring gap starts
/// at the gap's end.
pub fn start_of(day: NaiveDate, zone: Tz) -> Result<chrono::DateTime<Utc>, String> {
    let midnight = day.and_time(NaiveTime::MIN);
    zone.from_local_datetime(&midnight)
        .earliest()
        .or_else(|| {
            (1..=4)
                .map(|hours| midnight + chrono::Duration::hours(hours))
                .find_map(|later| zone.from_local_datetime(&later).earliest())
        })
        .map(|at| at.with_timezone(&Utc))
        .ok_or_else(|| format!("{day} has no start in {zone}"))
}
