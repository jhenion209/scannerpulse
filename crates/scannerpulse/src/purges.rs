//! Purging older than a date: every call before a day goes, its record with
//! its transcripts, labels and queue row, and its file for good (nothing is
//! kept in a holding folder). The record goes first and its file becomes a
//! leftover, which the filer's sweep deletes, so a stop never leaves a call
//! whose file is gone. A read-only location's files stay: only their records
//! go, and the location keeps the day, so that its import skips them.

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::jobs::{self, Job, JobError, Kind, Outcome};
use crate::storage;

/// The calls one step of a purge deletes: records only, no file is read.
pub const CALLS_PER_STEP: i64 = 500;

/// How long measuring the calls' files may take: a hung share must not hold
/// the request.
const MEASURE_LIMIT: Duration = Duration::from_secs(10);

/// The calls a purge takes from one location.
#[derive(Debug, Clone, Serialize)]
pub struct Held {
    pub location: String,
    /// Its files stay where they are: only the records go.
    pub read_only: bool,
    pub calls: i64,
    /// The space their files use, about (none for a read-only location's).
    pub bytes_about: Option<i64>,
}

/// What purging the calls before the day `before` would do: the calls of
/// each location, and the space of the files that go, about; or why it
/// cannot start.
#[derive(Debug, Clone, Serialize)]
pub struct PurgePreview {
    pub before: NaiveDate,
    pub calls: i64,
    pub held: Vec<Held>,
    pub bytes_about: Option<i64>,
    pub refused: Option<String>,
}

impl PurgePreview {
    /// Nothing counted: the purge cannot start, for `why`.
    fn refused(before: NaiveDate, why: String) -> PurgePreview {
        PurgePreview {
            before,
            calls: 0,
            held: Vec::new(),
            bytes_about: None,
            refused: Some(why),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Purge {
    until: DateTime<Utc>,
    before: NaiveDate,
}

/// A purge's state between steps.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PurgeWork {
    /// The last call taken.
    after: Option<(DateTime<Utc>, i64)>,
    purged: i64,
    /// Calls in a read-only location: their files stay.
    records_only: i64,
}

/// The instant the day `before` of `zone` begins.
fn cutoff(before: NaiveDate, zone: Tz) -> Result<DateTime<Utc>, JobError> {
    use chrono::Datelike;
    if !(1970..=9999).contains(&before.year()) {
        return Err(JobError::Refused(
            "the day must be between 1970 and 9999".into(),
        ));
    }
    crate::backfill::start_of(before, zone).map_err(JobError::Refused)
}

/// What purging the calls before the day `before` (in `zone`) would do. It
/// changes nothing, and refuses a day that has not begun.
pub async fn preview(
    conn: &mut PgConnection,
    before: NaiveDate,
    zone: Tz,
) -> Result<PurgePreview, JobError> {
    let until = cutoff(before, zone)?;
    // A mistyped year would purge every call, those filed meanwhile too.
    if until > Utc::now() {
        return Ok(PurgePreview::refused(
            before,
            format!("{before} is in the future: choose today or an earlier day"),
        ));
    }
    let rows: Vec<(String, String, bool, i64, Vec<String>)> = sqlx::query_as(&format!(
        "SELECT l.name, l.root, l.mode <> 'managed', count(*), \
                coalesce((array_agg(c.rel_path) FILTER (WHERE c.id % 97 = 0))[1:{}], \
                         ARRAY[min(c.rel_path), max(c.rel_path)]) \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.started_at < $1 \
         GROUP BY l.id, l.name, l.root, l.mode ORDER BY l.id",
        storage::SAMPLED
    ))
    .bind(until)
    .fetch_all(&mut *conn)
    .await?;
    let calls = rows.iter().map(|row| row.3).sum();
    if calls == 0 {
        return Ok(PurgePreview::refused(
            before,
            format!("no call is before {before}"),
        ));
    }
    // Measured in a thread of its own, for a limit: a share that does not
    // answer leaves its size unknown, which a purge does not need.
    let measuring = rows.clone();
    let measured = tokio::time::timeout(
        MEASURE_LIMIT,
        tokio::task::spawn_blocking(move || {
            measuring
                .iter()
                .map(|(_, root, read_only, calls, sample)| {
                    (!read_only)
                        .then(|| storage::bytes_about(Path::new(root), sample, *calls))
                        .flatten()
                })
                .collect::<Vec<Option<i64>>>()
        }),
    )
    .await;
    let sizes = match measured {
        Ok(Ok(sizes)) => sizes,
        _ => vec![None; rows.len()],
    };
    let held: Vec<Held> = rows
        .into_iter()
        .zip(sizes)
        .map(|((location, _, read_only, calls, _), bytes_about)| Held {
            location,
            read_only,
            calls,
            bytes_about,
        })
        .collect();
    let bytes_about = held
        .iter()
        .filter(|h| !h.read_only)
        .map(|h| h.bytes_about)
        .sum::<Option<i64>>();
    Ok(PurgePreview {
        before,
        calls,
        held,
        bytes_about,
        refused: None,
    })
}

/// Adds a job that purges the calls before the day `before`, once
/// [`preview`] finds calls and the day is typed as `confirm`. Every
/// read-only location keeps the latest day it was purged before. Returns the
/// job's id and what was counted.
pub async fn start(
    conn: &mut PgConnection,
    before: NaiveDate,
    zone: Tz,
    confirm: &str,
    started_by: &str,
) -> Result<(i64, PurgePreview), JobError> {
    let preview = preview(conn, before, zone).await?;
    if let Some(why) = &preview.refused {
        return Err(JobError::Refused(why.clone()));
    }
    if confirm.trim() != before.to_string() {
        return Err(JobError::Refused(format!("type {before} to confirm")));
    }
    let until = cutoff(before, zone)?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    sqlx::query(
        "UPDATE locations SET purged_before = GREATEST(purged_before, $1) WHERE mode = 'readonly'",
    )
    .bind(before)
    .execute(&mut *tx)
    .await?;
    let params = serde_json::to_value(Purge { until, before }).expect("plain data");
    let job = jobs::create(
        &mut *tx,
        Kind::PurgeBefore,
        &params,
        Some(preview.calls),
        started_by,
    )
    .await?;
    tx.commit().await?;
    Ok((job, preview))
}

pub(crate) async fn step(conn: &mut PgConnection, job: &Job) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<Purge>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a purge's".into(),
        ));
    };
    let mut work: PurgeWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let (after_at, after_id) = work.after.unzip();
    // One statement: the records go, the managed files are listed for the
    // sweep, and the last call taken is where the next step starts.
    let taken: Option<(i64, i64, i64, DateTime<Utc>, i64)> = sqlx::query_as(
        "WITH batch AS ( \
             SELECT id, started_at FROM calls \
             WHERE started_at < $1 \
               AND ($2::timestamptz IS NULL OR (started_at, id) > ($2, $3)) \
             ORDER BY started_at, id LIMIT $4 \
         ), gone AS ( \
             DELETE FROM calls c USING batch b, locations l \
             WHERE c.id = b.id AND c.started_at = b.started_at AND l.id = c.location_id \
             RETURNING c.location_id, c.rel_path, l.mode = 'managed' AS managed \
         ), listed AS ( \
             INSERT INTO archive_leftovers (location_id, rel_path) \
             SELECT location_id, rel_path FROM gone WHERE managed \
             ON CONFLICT DO NOTHING \
         ) \
         SELECT (SELECT count(*) FROM batch), \
                (SELECT count(*) FROM gone WHERE managed), \
                (SELECT count(*) FROM gone WHERE NOT managed), \
                last.started_at, last.id \
         FROM (SELECT started_at, id FROM batch ORDER BY started_at DESC, id DESC LIMIT 1) last",
    )
    .bind(params.until)
    .bind(after_at)
    .bind(after_id)
    .bind(CALLS_PER_STEP)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((taken, listed, records_only, last_at, last_id)) = taken else {
        let files = work.purged - work.records_only;
        let mut message = format!(
            "{} calls before {} purged: {files} files are deleted by the sweep",
            work.purged, params.before
        );
        if work.records_only > 0 {
            message.push_str(&format!(
                "; {} in a read-only location lost only {}",
                work.records_only,
                if work.records_only == 1 {
                    "its record"
                } else {
                    "their records"
                }
            ));
        }
        return Ok(Outcome::Done(job.done, message));
    };
    work.purged += listed + records_only;
    work.records_only += records_only;
    work.after = Some((last_at, last_id));
    let done = job.done + taken;
    let total = job.total.map(|total| total.max(done));
    Ok(Outcome::More {
        done,
        total,
        message: format!(
            "{} calls: {} purged",
            jobs::progress(done, total),
            work.purged
        ),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}
