//! The local times in the recorder's file names, as instants.

use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeDelta, TimeZone, Utc};
use chrono_tz::Tz;

/// How a local time became an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeHow {
    /// The local time names one instant.
    Plain,
    /// The local time is in the hour that the autumn change repeats, and the
    /// file's modification time chose between its two instants.
    ByMtime,
    /// The repeated hour, with the modification time unknown or too far from
    /// both instants: the earlier instant.
    EarlierDefault,
    /// The local time is in the hour that the spring change skips. It is read
    /// with the offset in force before the change, which moves it forward by
    /// the length of the gap.
    Gap,
}

/// How near the modification time must be to an instant of the repeated hour
/// to choose it: a window of 30 minutes.
pub const MTIME_WINDOW_SECONDS: i64 = 1800;

/// The instant of a local time in `zone`. `mtime` is the file's modification
/// time in whole seconds since 1970.
pub fn to_instant(local: NaiveDateTime, mtime: Option<i64>, zone: Tz) -> (DateTime<Utc>, TimeHow) {
    match zone.from_local_datetime(&local) {
        LocalResult::Single(at) => (at.with_timezone(&Utc), TimeHow::Plain),
        LocalResult::Ambiguous(a, b) => {
            let (a, b) = (a.with_timezone(&Utc), b.with_timezone(&Utc));
            let (earlier, later) = if a <= b { (a, b) } else { (b, a) };
            let Some(mtime) = mtime else {
                return (earlier, TimeHow::EarlierDefault);
            };
            let to_earlier = (mtime - earlier.timestamp()).abs();
            let to_later = (mtime - later.timestamp()).abs();
            if to_earlier.min(to_later) > MTIME_WINDOW_SECONDS {
                (earlier, TimeHow::EarlierDefault)
            } else if to_earlier <= to_later {
                (earlier, TimeHow::ByMtime)
            } else {
                (later, TimeHow::ByMtime)
            }
        }
        LocalResult::None => {
            // A day earlier is safely before the change.
            let before = zone
                .offset_from_utc_datetime(&(local - TimeDelta::days(1)))
                .fix();
            let at = local - TimeDelta::seconds(i64::from(before.local_minus_utc()));
            (at.and_utc(), TimeHow::Gap)
        }
    }
}
