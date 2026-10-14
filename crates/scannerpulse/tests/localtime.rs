use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use scannerpulse::localtime::{TimeHow, to_instant};

const CHICAGO: Tz = chrono_tz::America::Chicago;

fn local(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S").unwrap()
}

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn secs(text: &str) -> Option<i64> {
    Some(utc(text).timestamp())
}

#[test]
fn an_ordinary_time_has_one_instant() {
    assert_eq!(
        to_instant(local("2026-07-01 12:00:00"), None, CHICAGO),
        (utc("2026-07-01T17:00:00Z"), TimeHow::Plain)
    );
}

// 2026-11-01 01:30 happens twice in Chicago: at 06:30 UTC (CDT) and at 07:30 UTC (CST).
#[test]
fn in_the_repeated_hour_the_modification_time_chooses() {
    let at = local("2026-11-01 01:30:00");
    let earlier = utc("2026-11-01T06:30:00Z");
    let later = utc("2026-11-01T07:30:00Z");
    assert_eq!(
        to_instant(at, secs("2026-11-01T06:31:00Z"), CHICAGO),
        (earlier, TimeHow::ByMtime)
    );
    assert_eq!(
        to_instant(at, secs("2026-11-01T07:30:05Z"), CHICAGO),
        (later, TimeHow::ByMtime)
    );
    // Half way between: the earlier instant.
    assert_eq!(
        to_instant(at, secs("2026-11-01T07:00:00Z"), CHICAGO),
        (earlier, TimeHow::ByMtime)
    );
}

#[test]
fn in_the_repeated_hour_a_far_or_unknown_modification_time_gives_the_earlier_instant() {
    let at = local("2026-11-01 01:30:00");
    let earlier = utc("2026-11-01T06:30:00Z");
    assert_eq!(
        to_instant(at, secs("2026-11-01T09:00:00Z"), CHICAGO),
        (earlier, TimeHow::EarlierDefault)
    );
    assert_eq!(
        to_instant(at, None, CHICAGO),
        (earlier, TimeHow::EarlierDefault)
    );
}

// 2026-03-08 02:30 does not exist in Chicago: the clocks go from 02:00 CST to 03:00 CDT.
#[test]
fn a_time_in_the_spring_gap_moves_forward_by_the_gap() {
    assert_eq!(
        to_instant(local("2026-03-08 02:30:00"), None, CHICAGO),
        (utc("2026-03-08T08:30:00Z"), TimeHow::Gap)
    );
}

// Lord Howe Island moves its clocks by 30 minutes: 02:00 +10:30 becomes 02:30 +11:00.
#[test]
fn a_half_hour_gap_moves_a_time_by_half_an_hour() {
    let (at, how) = to_instant(
        local("2026-10-04 02:15:00"),
        None,
        chrono_tz::Australia::Lord_Howe,
    );
    assert_eq!(how, TimeHow::Gap);
    assert_eq!(at, utc("2026-10-03T15:45:00Z"));
    assert_eq!(
        at.with_timezone(&chrono_tz::Australia::Lord_Howe)
            .format("%H:%M")
            .to_string(),
        "02:45"
    );
}
