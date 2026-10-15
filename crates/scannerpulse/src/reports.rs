//! The admin page's Reports tab: the traffic report, which says when the
//! site is quiet enough for maintenance, and the totals of its period.

use chrono::{DateTime, Datelike, Duration, NaiveDate, Timelike, Utc, Weekday};
use chrono_tz::Tz;
use serde::Serialize;

/// The days the report looks back.
pub const DAYS: i32 = 120;
/// Five-minute slots in a day.
pub const SLOTS: usize = 288;
/// A window is an hour: twelve slots.
pub const WINDOW: usize = 12;
/// The quietest windows the report names, for weekdays and for weekends.
pub const QUIETEST: usize = 3;
/// The longest the report's query may take.
pub const TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

/// A quiet hour: its local start and end, and the calls it had per day.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Window {
    pub start: String,
    pub end: String,
    pub calls_per_day: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Traffic {
    pub days: i32,
    pub from: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub time_zone: String,
    pub weekdays: i64,
    pub weekend_days: i64,
    /// Calls per day in each five-minute slot, from local midnight.
    pub weekday: Vec<f64>,
    pub weekend: Vec<f64>,
    pub quietest_weekday: Vec<Window>,
    pub quietest_weekend: Vec<Window>,
    pub calls: i64,
    pub transcribed: i64,
    pub no_speech: i64,
}

/// The `n` quietest windows of `width` slots in a day's `profile`, the
/// quietest first, none overlapping another; a window may run past
/// midnight into the next morning. Each is its first slot and its sum.
pub fn quietest(profile: &[f64], width: usize, n: usize) -> Vec<(usize, f64)> {
    let slots = profile.len();
    if slots == 0 || width == 0 || width > slots {
        return Vec::new();
    }
    let mut windows: Vec<(usize, f64)> = (0..slots)
        .map(|start| {
            (
                start,
                (0..width).map(|i| profile[(start + i) % slots]).sum(),
            )
        })
        .collect();
    windows.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut chosen: Vec<(usize, f64)> = Vec::new();
    for window in windows {
        let overlaps = chosen.iter().any(|kept| {
            let distance = (window.0 + slots - kept.0) % slots;
            distance < width || slots - distance < width
        });
        if !overlaps {
            chosen.push(window);
            if chosen.len() == n {
                break;
            }
        }
    }
    chosen
}

fn clock(slot: usize) -> String {
    let minutes = (slot % SLOTS) * 5;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// The local days from `from` through `to`, as (weekdays, weekend days).
pub fn day_counts(from: NaiveDate, to: NaiveDate) -> (i64, i64) {
    let mut counts = (0, 0);
    let mut day = from;
    while day <= to {
        if matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
            counts.1 += 1;
        } else {
            counts.0 += 1;
        }
        let Some(next) = day.succ_opt() else { break };
        day = next;
    }
    counts
}

/// The traffic of the `DAYS` full local days before today.
pub async fn traffic(
    conn: &mut sqlx::PgConnection,
    zone: Tz,
    now: DateTime<Utc>,
) -> sqlx::Result<Traffic> {
    use sqlx::Connection;
    let today = now.with_timezone(&zone).date_naive();
    let first = today - Duration::days(i64::from(DAYS));
    let last = today - Duration::days(1);
    let start = |day: NaiveDate| {
        crate::backfill::start_of(day, zone)
            .unwrap_or_else(|_| day.and_hms_opt(0, 0, 0).unwrap().and_utc())
    };
    let (from, until) = (start(first), start(today));
    let mut tx = conn.begin().await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{}ms", TIME_LIMIT.as_millis()))
        .execute(&mut *tx)
        .await?;
    // Five-minute buckets in UTC, placed on the local day here: converting
    // every call to local time in the query takes several times longer.
    // Every zone's offset is a whole number of five minutes.
    let buckets: Vec<(DateTime<Utc>, i64, i64, i64)> = sqlx::query_as(
        "SELECT date_bin('5 minutes', started_at, timestamptz '2000-01-01 00:00:00+00'), \
                count(*), count(*) FILTER (WHERE transcript_state = 'text'), \
                count(*) FILTER (WHERE transcript_state = 'no_speech') \
         FROM calls WHERE started_at >= $1 AND started_at < $2 GROUP BY 1",
    )
    .bind(from)
    .bind(until)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    let (weekdays, weekend_days) = day_counts(first, last);
    let mut weekday = vec![0.0; SLOTS];
    let mut weekend = vec![0.0; SLOTS];
    let (mut calls, mut transcribed, mut no_speech) = (0, 0, 0);
    for (bucket, count, text, silent) in buckets {
        calls += count;
        transcribed += text;
        no_speech += silent;
        let local = bucket.with_timezone(&zone);
        let slot = (local.hour() * 12 + local.minute() / 5) as usize;
        let (profile, days) = if matches!(local.weekday(), Weekday::Sat | Weekday::Sun) {
            (&mut weekend, weekend_days)
        } else {
            (&mut weekday, weekdays)
        };
        if days > 0 && slot < SLOTS {
            profile[slot] += count as f64 / days as f64;
        }
    }
    let windows = |profile: &[f64]| {
        quietest(profile, WINDOW, QUIETEST)
            .into_iter()
            .map(|(slot, sum)| Window {
                start: clock(slot),
                end: clock(slot + WINDOW),
                calls_per_day: (sum * 10.0).round() / 10.0,
            })
            .collect()
    };
    Ok(Traffic {
        days: DAYS,
        from,
        until,
        time_zone: zone.name().to_string(),
        weekdays,
        weekend_days,
        quietest_weekday: windows(&weekday),
        quietest_weekend: windows(&weekend),
        weekday,
        weekend,
        calls,
        transcribed,
        no_speech,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_quietest_windows_do_not_overlap_and_may_cross_midnight() {
        // A day of 8 slots: busy except slots 7, 0 (across midnight) and 3.
        let profile = [0.0, 5.0, 5.0, 1.0, 5.0, 5.0, 5.0, 0.0];
        assert_eq!(quietest(&profile, 2, 2), [(7, 0.0), (2, 6.0)]);
        assert_eq!(quietest(&profile, 1, 3), [(0, 0.0), (7, 0.0), (3, 1.0)]);
        assert!(quietest(&profile, 9, 1).is_empty());
        assert!(quietest(&[], 1, 1).is_empty());
    }

    #[test]
    fn days_are_counted_as_weekdays_and_weekend_days() {
        let monday = NaiveDate::from_ymd_opt(2031, 1, 13).unwrap();
        let sunday = NaiveDate::from_ymd_opt(2031, 1, 19).unwrap();
        assert_eq!(day_counts(monday, sunday), (5, 2));
        assert_eq!(day_counts(sunday, monday), (0, 0));
    }

    #[test]
    fn a_slot_is_written_as_a_local_clock_time() {
        assert_eq!(clock(0), "00:00");
        assert_eq!(clock(13), "01:05");
        assert_eq!(clock(SLOTS + 2), "00:10");
    }
}
