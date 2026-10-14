//! What a recorder's file says about its call: the ID3 tag, the file name and
//! the audio frames, read by the rules of today's recorder and of the
//! migration, so that a call filed by ingest matches a migrated one.

use std::fmt;
use std::io::{self, Cursor};
use std::path::Path;
use std::sync::LazyLock;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use chrono_tz::Tz;
use id3::TagLike;
use regex::Regex;

use crate::localtime::{self, TimeHow};
use crate::mp3;

/// The largest file that ingest reads: a call lasts minutes and its file holds
/// a few megabytes at most.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Talkgroup {
    pub number: i64,
    pub name: String,
}

/// The ID3 values that the rules read: the first value of each frame, `None`
/// when the frame is absent or empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    /// TIT2.
    pub title: Option<String>,
    /// TPE1.
    pub artist: Option<String>,
    /// TXXX "Group Description".
    pub group_description: Option<String>,
    /// TIT1.
    pub grouping: Option<String>,
    /// COMM, language "eng", no description.
    pub comment: Option<String>,
}

/// What a file's name and tag say about its call, apart from times and length.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Described {
    pub system: Option<String>,
    pub site: Option<String>,
    pub talkgroup: Option<Talkgroup>,
    pub title: Option<String>,
    pub radio_id: Option<i64>,
    pub radio_alias: Option<String>,
}

/// A call as its file describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCall {
    /// The local time in the file name.
    pub local_start: NaiveDateTime,
    pub started_at: DateTime<Utc>,
    pub time_how: TimeHow,
    pub duration_ms: i32,
    pub file_mtime: DateTime<Utc>,
    pub described: Described,
}

#[derive(Debug)]
pub enum ReadError {
    /// The file cannot be filed; the text says why.
    Rejected(String),
    /// The file could not be read; try again later.
    Io(io::Error),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::Rejected(why) => write!(f, "rejected: {why}"),
            ReadError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ReadError {}

static TITLE_TALKGROUP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^([0-9]{1,18})"([^"]+)""#).unwrap());
static CHANNEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^([0-9]+)[-"](.+?)"?$"#).unwrap());
// The migration's P:<number>(?![0-9])[^"]*"<name>", without the look-ahead,
// which this regex engine does not have.
static PATCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^P:([0-9]{1,18})(?:[^0-9"][^"]*)?"([^"]+)""#).unwrap());
static RADIO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9]{1,18})(?:[- ](.*))?$").unwrap());
static START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{4}-[0-9]{2}-[0-9]{2}|[0-9]{8})_([0-9]{2})([0-9]{2})([0-9]{2})").unwrap()
});
static COMMENT_SYSTEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"System:([^;]+)").unwrap());
static COMMENT_SITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Site:([^;]+)").unwrap());

/// Reads the file at `path`. Its name gives the start time in `zone`.
/// Returns the call and the file's bytes.
pub fn read_path(path: &Path, zone: Tz) -> Result<(FileCall, Vec<u8>), ReadError> {
    let meta = std::fs::metadata(path).map_err(ReadError::Io)?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(ReadError::Rejected(format!(
            "the file is larger than {} MiB",
            MAX_FILE_BYTES >> 20
        )));
    }
    let data = std::fs::read(path).map_err(ReadError::Io)?;
    if data.len() as u64 != meta.len() {
        return Err(ReadError::Io(io::Error::other(
            "the file changed while it was read",
        )));
    }
    let mtime = DateTime::<Utc>::from(meta.modified().map_err(ReadError::Io)?);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let call = read_bytes(name, &data, mtime, zone).map_err(ReadError::Rejected)?;
    Ok((call, data))
}

/// The call in a file's bytes. `name` is the file's name, `file_mtime` its
/// modification time. Err gives the reason the file cannot be filed.
pub fn read_bytes(
    name: &str,
    data: &[u8],
    file_mtime: DateTime<Utc>,
    zone: Tz,
) -> Result<FileCall, String> {
    let local_start =
        start_time(name).ok_or("the file name holds no date and time (YYYYMMDD_HHMMSS)")?;
    let stream = mp3::stream(data).ok_or("the file holds no MPEG audio frame")?;
    let described = describe(name, &tags(data));
    let (started_at, time_how) =
        localtime::to_instant(local_start, Some(file_mtime.timestamp()), zone);
    Ok(FileCall {
        local_start,
        started_at,
        time_how,
        duration_ms: i32::try_from(stream.duration_ms).unwrap_or(i32::MAX),
        file_mtime,
        described,
    })
}

/// The local start time at the head of a file name: `YYYYMMDD_HHMMSS` or
/// `YYYY-MM-DD_HHMMSS`.
pub fn start_time(name: &str) -> Option<NaiveDateTime> {
    let found = START.captures(name)?;
    let day = &found[1];
    let date = if day.len() == 10 {
        NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?
    } else {
        NaiveDate::parse_from_str(day, "%Y%m%d").ok()?
    };
    let number = |i: usize| found[i].parse::<u32>().ok();
    let time = NaiveTime::from_hms_opt(number(2)?, number(3)?, number(4)?)?;
    Some(date.and_time(time))
}

/// The ID3v2 tag at the start of `data`. A tag that cannot be read gives no values, as today's recorder reads it.
pub fn tags(data: &[u8]) -> Tags {
    let len = mp3::id3v2_len(data);
    if len == 0 || len > data.len() {
        return Tags::default();
    }
    let Ok(tag) = id3::Tag::read_from2(Cursor::new(&data[..len])) else {
        return Tags::default();
    };
    Tags {
        title: tag.title().and_then(first_value),
        artist: tag.artist().and_then(first_value),
        group_description: tag
            .extended_texts()
            .find(|text| text.description.eq_ignore_ascii_case("Group Description"))
            .and_then(|text| first_value(&text.value)),
        grouping: tag
            .get("TIT1")
            .and_then(|frame| frame.content().text())
            .and_then(first_value),
        comment: tag
            .comments()
            .find(|comment| comment.lang == "eng" && comment.description.is_empty())
            .and_then(|comment| first_value(&comment.text)),
    }
}

/// The first of the values of a text frame (ID3v2.4 separates them with NUL,
/// which the database cannot store), `None` when it is empty.
fn first_value(text: &str) -> Option<String> {
    let value = text.split('\0').next().unwrap_or_default();
    (!value.is_empty()).then(|| value.to_string())
}

/// The rules: system, site, talkgroup, title and radio from the tag and the
/// file name.
pub fn describe(name: &str, tags: &Tags) -> Described {
    let comment = tags.comment.as_deref();
    let system = trimmed(tags.group_description.as_deref())
        .or_else(|| trimmed(tags.grouping.as_deref()))
        .or_else(|| comment_field(comment, &COMMENT_SYSTEM));
    let title = tags.title.as_deref();
    let (radio_id, radio_alias) = radio(tags.artist.as_deref());
    // A talkgroup and a site belong to a system: without one, the title alone
    // keeps what the recorder said.
    let Some(system) = system else {
        return Described {
            title: title.map(str::to_string),
            radio_id,
            radio_alias,
            ..Described::default()
        };
    };
    let from_title = title.and_then(title_talkgroup);
    let talkgroup = from_title.clone().or_else(|| {
        title
            .and_then(channel)
            .or_else(|| file_channel(name))
            .or_else(|| title.and_then(patch))
    });
    Described {
        system: Some(system),
        site: comment_field(comment, &COMMENT_SITE),
        talkgroup,
        // The plain <number>"<name>" title says nothing the talkgroup does not.
        title: if from_title.is_some() {
            None
        } else {
            title.map(str::to_string)
        },
        radio_id,
        radio_alias,
    }
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn comment_field(comment: Option<&str>, field: &Regex) -> Option<String> {
    trimmed(
        field
            .captures(comment?)
            .map(|found| found.get(1).unwrap().as_str()),
    )
}

/// `<number>"<name>"` at the start of the ID3 title.
fn title_talkgroup(title: &str) -> Option<Talkgroup> {
    let found = TITLE_TALKGROUP.captures(title.trim())?;
    talkgroup(&found[1], found[2].trim())
}

/// A channel token, `<number>"<name>"` or `<number>-<name>`.
fn channel(value: &str) -> Option<Talkgroup> {
    let found = CHANNEL.captures(value.trim().trim_matches('_'))?;
    talkgroup(&found[1], found[2].trim().trim_matches('"'))
}

/// The channel token in the file name: its third field, separated by `_`.
fn file_channel(name: &str) -> Option<Talkgroup> {
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    stem.split('_').nth(2).and_then(channel)
}

/// A patch title, `P:<number>..."<name>"`: the first number and the first
/// quoted name.
fn patch(title: &str) -> Option<Talkgroup> {
    let found = PATCH.captures(title)?;
    talkgroup(&found[1], found[2].trim())
}

/// A number of at most 18 digits, which a bigint holds, and a name.
fn talkgroup(number: &str, name: &str) -> Option<Talkgroup> {
    if number.len() > 18 || name.is_empty() {
        return None;
    }
    Some(Talkgroup {
        number: number.parse().ok()?,
        name: name.to_string(),
    })
}

/// The radio from the ID3 artist: `<number>`, `<number>-<alias>` or
/// `<number> <alias>`. Anything else is kept whole as the alias.
fn radio(artist: Option<&str>) -> (Option<i64>, Option<String>) {
    let Some(artist) = artist else {
        return (None, None);
    };
    match RADIO.captures(artist) {
        Some(found) => (
            found[1].parse().ok(),
            found
                .get(2)
                .map(|alias| alias.as_str())
                .filter(|alias| !alias.is_empty())
                .map(str::to_string),
        ),
        None => (None, Some(artist.to_string())),
    }
}
