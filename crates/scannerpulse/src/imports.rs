//! Importing an older archive where it is: another install's folder of
//! recordings becomes a read-only location, in which nothing is ever written.
//!
//! A review comes first and writes nothing: a job walks the folder, counts its
//! files, and reads a sample of them to tell the new calls from the copies of
//! calls already in the archive, the files of a system no one has named yet,
//! the talkgroups the files name otherwise, and the files that cannot be
//! read; it counts the copies at each whole-hour offset too, and suggests one
//! when the files are copies only once their times are moved.
//!
//! The import is a job too. Its location, its source (which its calls name,
//! and whose ingest rows are its register) and its offset are made when it
//! starts. Its first step lists every file it has not taken as a waiting row;
//! each later step files a batch: a copy of a call is skipped, never replacing
//! the archive's recording; a file of an unknown system waits in the folder
//! until its system is named and the import runs again; any other file is a
//! call of the location at its path in the folder, timed by its name and the
//! offset, its talkgroup names going through the approval, and queued for no
//! transcription. Run again, an import takes only the files it has not taken.

use std::collections::{BTreeSet, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};

use crate::data::{bounded, identities, unchanged};
use crate::jobs::{self, Job, JobError, Kind, Outcome};
use crate::reader::{self, FileCall, ReadError};
use crate::store::{self, NameSeen, NewCall, SameCall};

/// The files a review reads: the others are counted, not read.
pub const REVIEW_SAMPLE: usize = 1000;

/// The files one step of a review reads.
const REVIEW_PER_STEP: usize = 100;

/// The files one step of an import takes.
pub const FILES_PER_STEP: i64 = 200;

/// The offsets, in minutes, at which a review counts the copies.
pub const OFFSETS: [i32; 5] = [-120, -60, 0, 60, 120];

/// The files of each kind a review shows.
pub const SHOWN: usize = 5;

/// How long a listing of the folder may take: less than a step may stay idle.
const WALK_LIMIT: Duration = jobs::REQUEST_LIMIT;

/// How long the web service's look at the folder may take.
const LOOK_LIMIT: Duration = Duration::from_secs(10);

/// An offset as `±HH:MM`, or `0`, in minutes: the times the file names give
/// are moved by it. A day at most either way.
pub fn parse_offset(text: &str) -> Result<i32, String> {
    let text = text.trim();
    let wrong = || format!("{text:?} is not an offset: write it as +01:00 or -00:30");
    if text == "0" {
        return Ok(0);
    }
    let (sign, rest) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let (hours, minutes) = rest.split_once(':').ok_or_else(wrong)?;
    let (hours, minutes): (i32, i32) = (
        hours.parse().map_err(|_| wrong())?,
        minutes.parse().map_err(|_| wrong())?,
    );
    let total = hours * 60 + minutes;
    if hours < 0 || !(0..60).contains(&minutes) || total > 24 * 60 {
        return Err(wrong());
    }
    Ok(sign * total)
}

/// An offset as `±HH:MM`.
pub fn offset_text(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    format!("{sign}{:02}:{:02}", minutes.abs() / 60, minutes.abs() % 60)
}

/// When a file's call started: the local time its name gives, in the site's
/// zone, moved by the import's offset. Never by its modification time: a
/// copy of the folder changed it.
pub fn started_at(local: NaiveDateTime, offset: i32, zone: Tz) -> DateTime<Utc> {
    crate::localtime::to_instant(local, None, zone).0 + TimeDelta::minutes(offset.into())
}

/// `n` and the noun, one or many: `1 call`, `2 calls`.
fn counted(n: i64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The folder with its links resolved, once this process has listed it.
/// Nothing is written in it.
pub fn folder_here(folder: &Path) -> Result<PathBuf, String> {
    if !folder.is_absolute() {
        return Err(format!(
            "{} is not a full path: start it with /",
            folder.display()
        ));
    }
    let meta = std::fs::metadata(folder).map_err(|_| {
        format!(
            "{} does not exist on this machine: mount the disk or share there first",
            folder.display()
        )
    })?;
    if !meta.is_dir() {
        return Err(format!("{} is a file, not a folder", folder.display()));
    }
    let real =
        std::fs::canonicalize(folder).map_err(|error| format!("{}: {error}", folder.display()))?;
    std::fs::read_dir(&real)
        .map_err(|error| format!("{} cannot be listed: {error}", real.display()))?;
    Ok(real)
}

/// [`folder_here`], for [`LOOK_LIMIT`] at most: a share that does not answer
/// must not hold the request.
async fn look(folder: &Path) -> Result<PathBuf, JobError> {
    let folder = folder.to_path_buf();
    match tokio::time::timeout(
        LOOK_LIMIT,
        tokio::task::spawn_blocking(move || folder_here(&folder)),
    )
    .await
    {
        Ok(Ok(found)) => found.map_err(JobError::Refused),
        Ok(Err(error)) => Err(JobError::Refused(format!("the look failed: {error}"))),
        Err(_) => Err(JobError::Refused(format!(
            "the folder did not answer within {} s",
            LOOK_LIMIT.as_secs()
        ))),
    }
}

/// An import as its rows hold it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct Kept {
    source_id: i16,
    location_id: Option<i16>,
    folder: String,
    offset_minutes: i32,
    purged_before: Option<NaiveDate>,
}

/// The import named `name`, if any: its location is gone (None) once
/// removed, its register stays.
async fn kept(conn: &mut PgConnection, name: &str) -> sqlx::Result<Option<Kept>> {
    sqlx::query_as(
        "SELECT i.source_id, i.location_id, s.path AS folder, i.offset_minutes, l.purged_before \
         FROM imports i JOIN sources s ON s.id = i.source_id \
         LEFT JOIN locations l ON l.id = i.location_id WHERE s.name = $1",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
}

/// Why an import named `name` cannot be of `folder` (its links resolved):
/// the name is another location's or a recorder's, the import of that name
/// (`kept`) is of another folder, or the folder is one in use, by its path
/// or as the same directory reached by another path.
async fn importable(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    kept: Option<&Kept>,
) -> Result<(), JobError> {
    if let Some(problem) = crate::recorders::name_problem(name) {
        return Err(JobError::Refused(problem));
    }
    if let Some(kept) = kept {
        if Path::new(&kept.folder) != folder {
            return Err(JobError::Refused(format!(
                "{name} is the import of {}: import {} under another name",
                kept.folder,
                folder.display()
            )));
        }
        return Ok(());
    }
    let named: Option<String> = sqlx::query_scalar(
        "SELECT 'a location' FROM locations WHERE name = $1 \
         UNION ALL SELECT 'a recorder' FROM sources WHERE name = $1 LIMIT 1",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(what) = named {
        return Err(JobError::Refused(format!(
            "{what} is named {name} already: choose another name"
        )));
    }
    let taken = crate::storage::taken(&mut *conn, None).await?;
    if let Some(problem) = crate::storage::folder_problem(folder, &taken) {
        return Err(JobError::Refused(problem));
    }
    // One directory reached by two paths (a bind mount) has one device and
    // inode: nothing may be written in the folder to find it out.
    let paths: Vec<PathBuf> = std::iter::once(folder.to_path_buf())
        .chain(taken.iter().map(|(_, path)| path.clone()))
        .collect();
    let found = tokio::time::timeout(
        LOOK_LIMIT,
        tokio::task::spawn_blocking(move || {
            let paths: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
            identities(&paths)
        }),
    )
    .await;
    let Ok(Ok(found)) = found else {
        return Err(JobError::Refused(format!(
            "a folder in use did not answer within {} s: try again",
            LOOK_LIMIT.as_secs()
        )));
    };
    if let Some(i) = found[1..]
        .iter()
        .position(|other| other.is_some() && *other == found[0])
    {
        return Err(JobError::Refused(format!(
            "{} and {} are one folder reached by two paths: it is {}",
            folder.display(),
            taken[i].1.display(),
            taken[i].0
        )));
    }
    Ok(())
}

/// A job that imports into, or consolidates, the import `name`, queued or
/// running.
async fn running(conn: &mut PgConnection, name: &str) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar(
        "SELECT id FROM jobs WHERE kind IN ('import', 'consolidate') \
           AND state IN ('queued', 'running') AND params ->> 'name' = $1 ORDER BY id LIMIT 1",
    )
    .bind(name)
    .fetch_optional(&mut *conn)
    .await
}

/// What one file of an older archive is.
#[derive(Debug)]
enum Class {
    /// From before the day its import was purged before.
    BeforePurge(NaiveDate),
    /// Its system has no name in this install yet.
    UnknownSystem(String),
    /// A copy of a call already in the archive.
    Copy(SameCall),
    /// A new call; the talkgroup's name here when the file names it
    /// otherwise.
    New(Option<String>),
}

/// What the file's call `call` is, for the import `source_id` (-1 for none
/// yet) purged before `purged_before`. It writes nothing.
async fn classify(
    conn: &mut PgConnection,
    source_id: i16,
    call: &FileCall,
    purged_before: Option<NaiveDate>,
    zone: Tz,
) -> sqlx::Result<Class> {
    if let Some(day) = purged_before
        && call.started_at.with_timezone(&zone).date_naive() < day
    {
        return Ok(Class::BeforePurge(day));
    }
    let d = &call.described;
    let Some(system) = d.system.as_deref() else {
        return Ok(Class::New(None));
    };
    if !store::system_known(&mut *conn, system).await? {
        return Ok(Class::UnknownSystem(system.to_string()));
    }
    let Some(talkgroup) = &d.talkgroup else {
        return Ok(Class::New(None));
    };
    let Some(id) = store::talkgroup_of(&mut *conn, system, talkgroup.number).await? else {
        return Ok(Class::New(None));
    };
    if let Some(same) = store::same_call(&mut *conn, id, call, source_id).await? {
        return Ok(Class::Copy(same));
    }
    let stored: String = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(Class::New(
        (store::name_seen(&stored, &talkgroup.name) == NameSeen::Propose).then_some(stored),
    ))
}

/// The file's call, timed by its name and the offset; or why it cannot be
/// read as a call (`Ok(Err)`); Err when its folder did not answer.
async fn read_call(
    root: &Path,
    rel_path: &str,
    offset: i32,
    zone: Tz,
    limit: Duration,
) -> io::Result<Result<FileCall, String>> {
    let path = root.join(rel_path);
    match bounded(limit, move || Ok(reader::read_path(&path, zone))).await? {
        Ok((mut call, _)) => {
            call.started_at = started_at(call.local_start, offset, zone);
            Ok(Ok(call))
        }
        Err(ReadError::Rejected(why)) => Ok(Err(why)),
        Err(ReadError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Err("the file is gone from the folder".into()))
        }
        Err(ReadError::Io(error)) => Ok(Err(error.to_string())),
    }
}

/// Examples of each kind of file, [`SHOWN`] at most each.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Examples {
    pub no_date: Vec<String>,
    pub new: Vec<String>,
    pub copies: Vec<String>,
    pub unknown: Vec<String>,
    pub differ: Vec<String>,
    pub unreadable: Vec<String>,
}

fn shown(list: &mut Vec<String>, example: String) {
    if list.len() < SHOWN {
        list.push(example);
    }
}

/// What a review found. The files in the folder are counted exactly; the
/// kinds of call are counted among the files `read`, a sample of
/// [`REVIEW_SAMPLE`] at most of the files `left` once those taken before and
/// those with no date in their names are counted out.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Review {
    pub name: String,
    pub folder: String,
    /// Every mp3 file in the folder.
    pub files: i64,
    /// Files an earlier run of this import took.
    pub taken: i64,
    /// Files whose names hold no date and time: they are skipped.
    pub no_date: i64,
    /// Files whose names are not UTF-8: they are skipped.
    pub bad_names: i64,
    pub left: i64,
    pub read: i64,
    pub new: i64,
    pub copies: i64,
    pub unknown: i64,
    /// New calls whose files name their talkgroup otherwise.
    pub differ: i64,
    pub unreadable: i64,
    /// From before the day the import was purged before.
    pub before_purge: i64,
    pub examples: Examples,
    /// The copies found at each of [`OFFSETS`] among the files read.
    pub copies_at: Vec<i64>,
    /// The offset at which clearly more files are copies than at none.
    pub suggested: Option<i32>,
    /// The offset the import keeps, when it ran before.
    pub kept_offset: Option<i32>,
    /// The files of the sample still to read.
    #[serde(default)]
    sample: Vec<String>,
    #[serde(default)]
    walked: bool,
}

/// The offset to suggest: the one at which the most files are copies, when it
/// is not none, finds 5 at least, and twice as many as none does.
pub fn suggest(copies_at: &[i64]) -> Option<i32> {
    let at_none = copies_at.get(2).copied().unwrap_or(0);
    let (offset, most) = OFFSETS
        .iter()
        .zip(copies_at)
        .max_by_key(|(_, copies)| **copies)?;
    (*offset != 0 && *most >= 5 && *most >= 2 * at_none).then_some(*offset)
}

#[derive(Debug, Serialize, Deserialize)]
struct ReviewParams {
    name: String,
    folder: String,
}

/// Adds a job that reviews the folder for an import named `name`. Returns
/// the job's id.
pub async fn start_review(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    started_by: &str,
) -> Result<i64, JobError> {
    let real = look(folder).await?;
    let kept = kept(conn, name).await?;
    importable(conn, name, &real, kept.as_ref()).await?;
    let params = serde_json::to_value(ReviewParams {
        name: name.to_string(),
        folder: real.display().to_string(),
    })
    .expect("plain data");
    Ok(jobs::create(&mut *conn, Kind::ImportReview, &params, None, started_by).await?)
}

/// What the review `job` found, once it has walked the folder.
pub async fn review(conn: &mut PgConnection, job: i64) -> sqlx::Result<Option<Review>> {
    let found: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT work FROM jobs WHERE id = $1 AND kind = 'import_review'")
            .bind(job)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(found
        .and_then(|work| serde_json::from_value::<Review>(work).ok())
        .filter(|review| review.walked)
        .map(|review| Review {
            sample: Vec::new(),
            ..review
        }))
}

/// Of `paths`, those the import `source_id` has taken: every row it has but
/// a waiting one.
async fn taken_paths(
    conn: &mut PgConnection,
    source_id: i16,
    paths: &[String],
) -> sqlx::Result<HashSet<String>> {
    let mut taken = HashSet::new();
    for chunk in paths.chunks(10_000) {
        let found: Vec<String> = sqlx::query_scalar(
            "SELECT rel_path FROM ingest_files \
             WHERE source_id = $1 AND rel_path = ANY($2) AND state <> 'waiting'",
        )
        .bind(source_id)
        .bind(chunk)
        .fetch_all(&mut *conn)
        .await?;
        taken.extend(found);
    }
    Ok(taken)
}

/// The settings' time zone, or why there is none.
async fn zone(conn: &mut PgConnection) -> sqlx::Result<Result<Tz, String>> {
    Ok(match crate::settings::load(&mut *conn).await {
        Ok(settings) => Ok(settings.time_zone),
        Err(crate::settings::SettingsError::Database(error)) => return Err(error),
        Err(other) => Err(other.to_string()),
    })
}

/// The walk of a folder, for [`WALK_LIMIT`] at most: its mp3 files and the
/// number of names that are not UTF-8; Err with the reason it failed.
async fn walk(folder: &Path) -> Result<(Vec<crate::watcher::Found>, i64), String> {
    let root = folder.to_path_buf();
    match bounded(WALK_LIMIT, move || crate::watcher::list_folder(&root)).await {
        Ok(listing) => Ok((listing.found, listing.bad_names.len() as i64)),
        Err(error) => Err(format!("{} {error}", folder.display())),
    }
}

pub(crate) async fn review_step(
    conn: &mut PgConnection,
    job: &Job,
    limit: Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<ReviewParams>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not a review's".into(),
        ));
    };
    let mut work: Review = serde_json::from_value(job.work.clone()).unwrap_or_default();
    if work.walked && work.sample.is_empty() {
        // The step before read the last file: the work it kept is the review.
        return Ok(Outcome::Done(work.read, review_message(&work)));
    }
    let zone = match zone(conn).await? {
        Ok(zone) => zone,
        Err(why) => return Ok(Outcome::Failed(why)),
    };
    let kept = kept(conn, &params.name).await?;
    let (source_id, purged_before) = kept
        .as_ref()
        .map_or((-1, None), |k| (k.source_id, k.purged_before));
    let root = PathBuf::from(&params.folder);
    if !work.walked {
        let (found, bad_names) = match walk(&root).await {
            Ok(found) => found,
            Err(why) => return Ok(Outcome::Failed(why)),
        };
        work.name = params.name.clone();
        work.folder = params.folder.clone();
        work.kept_offset = kept.as_ref().map(|k| k.offset_minutes);
        work.files = found.len() as i64;
        work.bad_names = bad_names;
        let mut dated = Vec::new();
        for file in found {
            let name = file.rel_path.rsplit('/').next().unwrap_or_default();
            if reader::start_time(name).is_none() {
                work.no_date += 1;
                shown(&mut work.examples.no_date, file.rel_path);
            } else {
                dated.push(file.rel_path);
            }
        }
        let taken = taken_paths(conn, source_id, &dated).await?;
        work.taken = taken.len() as i64;
        let mut left: Vec<String> = dated
            .into_iter()
            .filter(|path| !taken.contains(path))
            .collect();
        left.sort();
        work.left = left.len() as i64;
        // Spread over the whole folder, which its names order by time.
        let wanted = left.len().min(REVIEW_SAMPLE);
        work.sample = (0..wanted)
            .map(|i| left[i * left.len() / wanted].clone())
            .collect();
        work.copies_at = vec![0; OFFSETS.len()];
        work.walked = true;
        return Ok(more_review(&work));
    }
    let batch: Vec<String> = work
        .sample
        .drain(..work.sample.len().min(REVIEW_PER_STEP))
        .collect();
    for rel in batch {
        let call = match read_call(&root, &rel, 0, zone, limit).await {
            Ok(Ok(call)) => call,
            Ok(Err(why)) => {
                work.read += 1;
                work.unreadable += 1;
                shown(&mut work.examples.unreadable, format!("{rel}: {why}"));
                continue;
            }
            Err(error) => {
                return Ok(Outcome::Failed(format!(
                    "{} {error}: start the review again once it answers",
                    root.display()
                )));
            }
        };
        work.read += 1;
        match classify(conn, source_id, &call, purged_before, zone).await? {
            Class::BeforePurge(_) => work.before_purge += 1,
            Class::UnknownSystem(system) => {
                work.unknown += 1;
                shown(
                    &mut work.examples.unknown,
                    format!("{rel}: the system {system}"),
                );
            }
            Class::Copy(same) => {
                work.copies += 1;
                shown(
                    &mut work.examples.copies,
                    format!("{rel}: a copy of call {} from {}", same.id, same.recorder),
                );
            }
            Class::New(differs) => {
                work.new += 1;
                if let (Some(stored), Some(talkgroup)) = (differs, &call.described.talkgroup) {
                    work.differ += 1;
                    shown(
                        &mut work.examples.differ,
                        format!(
                            "{rel}: talkgroup {} is {stored} here, {} in the file",
                            talkgroup.number, talkgroup.name
                        ),
                    );
                }
                shown(&mut work.examples.new, rel.clone());
            }
        }
        // The same file at each offset: a clock or a zone an hour or two off
        // shows as copies found only once the times are moved.
        if let (Some(system), Some(talkgroup)) =
            (call.described.system.as_deref(), &call.described.talkgroup)
            && let Some(id) = store::talkgroup_of(&mut *conn, system, talkgroup.number).await?
        {
            for (i, offset) in OFFSETS.iter().enumerate() {
                let moved = FileCall {
                    started_at: call.started_at + TimeDelta::minutes((*offset).into()),
                    ..call.clone()
                };
                if store::same_call(&mut *conn, id, &moved, source_id)
                    .await?
                    .is_some()
                {
                    work.copies_at[i] += 1;
                }
            }
        }
    }
    if work.sample.is_empty() {
        work.suggested = suggest(&work.copies_at);
    }
    Ok(more_review(&work))
}

fn more_review(work: &Review) -> Outcome {
    let total = work.read + work.sample.len() as i64;
    Outcome::More {
        done: work.read,
        total: Some(total),
        work: serde_json::to_value(work).expect("plain data"),
        message: format!(
            "{} in {}: {} read of a sample of {total}",
            counted(work.files, "file", "files"),
            work.folder,
            work.read
        ),
    }
}

fn review_message(work: &Review) -> String {
    let mut message = format!(
        "{} in {}; of {} read: {} new, {} copies, {} of unknown systems, {} unreadable",
        counted(work.files, "file", "files"),
        work.folder,
        work.read,
        work.new,
        work.copies,
        work.unknown,
        work.unreadable
    );
    if let Some(offset) = work.suggested {
        message.push_str(&format!("; suggested offset {}", offset_text(offset)));
    }
    message
}

/// A review as the command line shows it. Counts made on a sample are
/// scaled to the files left, as "about".
pub fn review_text(review: &Review) -> String {
    let sampled = review.read > 0 && review.read < review.left;
    let about = |n: i64| {
        if sampled && n > 0 {
            format!(
                "about {}",
                (n * review.left + review.read / 2) / review.read
            )
        } else {
            n.to_string()
        }
    };
    let mut text = format!(
        "{}: {}\n  {}",
        review.name,
        review.folder,
        counted(review.files, "mp3 file", "mp3 files")
    );
    if review.taken > 0 {
        text.push_str(&format!("; {} taken by an earlier run", review.taken));
    }
    if review.no_date > 0 {
        text.push_str(&format!(
            "; {} with no date and time in their names, skipped",
            review.no_date
        ));
    }
    if review.bad_names > 0 {
        text.push_str(&format!(
            "; {} whose names are not UTF-8, skipped",
            review.bad_names
        ));
    }
    text.push('\n');
    if review.read > 0 {
        text.push_str(&format!(
            "  of the {} left{}: {} new calls, {} copies of calls already in the archive (skipped), \
             {} of systems not named yet (they wait), {} unreadable (skipped)\n",
            review.left,
            if sampled {
                format!(", {} read", review.read)
            } else {
                String::new()
            },
            about(review.new),
            about(review.copies),
            about(review.unknown),
            about(review.unreadable)
        ));
        if review.differ > 0 {
            text.push_str(&format!(
                "  {} new calls name their talkgroup otherwise: the names wait for approval\n",
                about(review.differ)
            ));
        }
        if review.before_purge > 0 {
            text.push_str(&format!(
                "  {} are from before the day this import was purged before (skipped)\n",
                about(review.before_purge)
            ));
        }
        let at: Vec<String> = OFFSETS
            .iter()
            .zip(&review.copies_at)
            .map(|(offset, copies)| format!("{} {copies}", offset_text(*offset)))
            .collect();
        text.push_str(&format!("  copies at each offset: {}\n", at.join(", ")));
    }
    match (review.kept_offset, review.suggested) {
        (Some(kept), _) => text.push_str(&format!(
            "  this import keeps its offset, {}\n",
            offset_text(kept)
        )),
        (None, Some(offset)) => text.push_str(&format!(
            "  the files are copies once their times are moved by {0}: the older install's clock or zone was off; import with --offset {0}\n",
            offset_text(offset)
        )),
        (None, None) => {}
    }
    let e = &review.examples;
    for (what, list) in [
        ("no date and time in the name", &e.no_date),
        ("new", &e.new),
        ("copies", &e.copies),
        ("systems not named yet", &e.unknown),
        ("talkgroups named otherwise", &e.differ),
        ("unreadable", &e.unreadable),
    ] {
        if !list.is_empty() {
            text.push_str(&format!("  {what}:\n"));
            for line in list {
                text.push_str(&format!("    {line}\n"));
            }
        }
    }
    text
}

#[derive(Debug, Serialize, Deserialize)]
struct ImportParams {
    name: String,
    source: i16,
}

/// What starting an import did: its location, its source and its offset, and
/// whether it ran before.
#[derive(Debug, Clone, Serialize)]
pub struct Started {
    pub location: i16,
    pub source: i16,
    pub offset: i32,
    pub again: bool,
}

/// Adds a job that imports the folder as the read-only location `name`,
/// with its file names' times moved by `offset` minutes (none: the offset it
/// keeps, or none at all). A new import's location, source and offset are
/// made now; an import that ran before keeps its folder and its offset, and
/// a removed one gets its location back. Returns the job's id.
pub async fn start(
    conn: &mut PgConnection,
    name: &str,
    folder: &Path,
    offset: Option<i32>,
    started_by: &str,
) -> Result<(i64, Started), JobError> {
    let real = look(folder).await?;
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    match start_in(&mut tx, name, &real, offset, started_by).await {
        Ok(started) => {
            tx.commit().await?;
            Ok(started)
        }
        // Rolled back at once: a dropped transaction would hold its locks
        // until its connection is used again.
        Err(error) => {
            tx.rollback().await?;
            Err(error)
        }
    }
}

async fn start_in(
    tx: &mut PgConnection,
    name: &str,
    real: &Path,
    offset: Option<i32>,
    started_by: &str,
) -> Result<(i64, Started), JobError> {
    // One start at a time: two would make the same import twice.
    sqlx::query("SELECT pg_advisory_xact_lock(1397048658, 17)")
        .execute(&mut *tx)
        .await?;
    let kept = kept(tx, name).await?;
    importable(tx, name, real, kept.as_ref()).await?;
    if let Some(job) = running(tx, name).await? {
        return Err(JobError::Refused(format!(
            "job {job} works on {name} already"
        )));
    }
    let started = match kept {
        Some(kept) => {
            if let Some(offset) = offset
                && offset != kept.offset_minutes
            {
                return Err(JobError::Refused(format!(
                    "{name} was imported with the offset {}: it keeps it, so that its files are dated as before",
                    offset_text(kept.offset_minutes)
                )));
            }
            let location = match kept.location_id {
                Some(location) => location,
                None => {
                    let location = new_location(tx, name, real).await?;
                    sqlx::query("UPDATE imports SET location_id = $2 WHERE source_id = $1")
                        .bind(kept.source_id)
                        .bind(location)
                        .execute(&mut *tx)
                        .await?;
                    location
                }
            };
            Started {
                location,
                source: kept.source_id,
                offset: kept.offset_minutes,
                again: true,
            }
        }
        None => {
            let offset = offset.unwrap_or(0);
            let location = new_location(tx, name, real).await?;
            let source: i16 = sqlx::query_scalar(
                "INSERT INTO sources (name, kind, path, enabled) \
                 VALUES ($1, 'import', $2, false) RETURNING id",
            )
            .bind(name)
            .bind(real.display().to_string())
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO imports (source_id, location_id, offset_minutes) VALUES ($1, $2, $3)",
            )
            .bind(source)
            .bind(location)
            .bind(offset)
            .execute(&mut *tx)
            .await?;
            Started {
                location,
                source,
                offset,
                again: false,
            }
        }
    };
    let params = serde_json::to_value(ImportParams {
        name: name.to_string(),
        source: started.source,
    })
    .expect("plain data");
    let job = jobs::create(&mut *tx, Kind::Import, &params, None, started_by).await?;
    Ok((job, started))
}

async fn new_location(conn: &mut PgConnection, name: &str, folder: &Path) -> sqlx::Result<i16> {
    sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode) VALUES ($1, $2, 'readonly') RETURNING id",
    )
    .bind(name)
    .bind(folder.display().to_string())
    .fetch_one(&mut *conn)
    .await
}

/// An import's counts between steps.
#[derive(Debug, Default, Serialize, Deserialize)]
struct ImportWork {
    walked: bool,
    /// The last row taken.
    after: i64,
    imported: i64,
    copies: i64,
    staged: i64,
    rejected: i64,
    before_purge: i64,
    purged_before: Option<NaiveDate>,
}

impl ImportWork {
    fn message(&self, name: &str) -> String {
        let mut parts = vec![format!(
            "{} imported into {name}",
            counted(self.imported, "call", "calls")
        )];
        if self.copies > 0 {
            parts.push(format!(
                "{} already in the archive skipped",
                counted(self.copies, "copy of a call", "copies of calls")
            ));
        }
        if self.staged > 0 {
            parts.push(if self.staged == 1 {
                "1 file waits until its system is named".to_string()
            } else {
                format!("{} files wait until their systems are named", self.staged)
            });
        }
        if self.rejected > 0 {
            parts.push(if self.rejected == 1 {
                "1 file skipped: it cannot be read as a call".to_string()
            } else {
                format!(
                    "{} files skipped: they cannot be read as calls",
                    self.rejected
                )
            });
        }
        if self.before_purge > 0 {
            let day = self
                .purged_before
                .map(|day| day.to_string())
                .unwrap_or_default();
            parts.push(if self.before_purge == 1 {
                format!("1 file skipped: it is from before {day}, before which {name} was purged")
            } else {
                format!(
                    "{} files skipped: they are from before {day}, before which {name} was purged",
                    self.before_purge
                )
            });
        }
        parts.join("; ")
    }
}

/// A file one step of an import takes.
#[derive(Debug, sqlx::FromRow)]
struct Row {
    id: i64,
    rel_path: String,
    size: Option<i64>,
}

/// The import's location now, its offset, and the day it was purged before.
#[derive(Debug, sqlx::FromRow)]
struct Place {
    location_id: i16,
    root: String,
    offset_minutes: i32,
    purged_before: Option<NaiveDate>,
}

/// The import ends with its counts when its folder stops answering: run
/// again once it answers, it takes the files left.
fn hung(what: &str, work: &ImportWork, name: &str) -> Outcome {
    Outcome::Failed(format!(
        "{what}: {}; import it again once it answers",
        work.message(name)
    ))
}

/// The rows an import takes: those waiting, and those staged whose system
/// has been named since.
const TO_TAKE: &str = "(f.state = 'waiting' OR (f.state = 'staged' \
     AND (EXISTS (SELECT 1 FROM systems y WHERE y.name = f.staged_system) \
          OR EXISTS (SELECT 1 FROM system_spellings y WHERE y.name = f.staged_system))))";

pub(crate) async fn import_step(
    conn: &mut PgConnection,
    pool: &PgPool,
    job: &Job,
    limit: Duration,
) -> sqlx::Result<Outcome> {
    let Ok(params) = serde_json::from_value::<ImportParams>(job.params.clone()) else {
        return Ok(Outcome::Failed(
            "the job's parameters are not an import's".into(),
        ));
    };
    let mut work: ImportWork = serde_json::from_value(job.work.clone()).unwrap_or_default();
    let zone = match zone(conn).await? {
        Ok(zone) => zone,
        Err(why) => return Ok(Outcome::Failed(why)),
    };
    // Shared, so that the location is not removed while the step runs.
    let place: Option<Place> = sqlx::query_as(
        "SELECT l.id AS location_id, l.root, i.offset_minutes, l.purged_before \
         FROM imports i JOIN locations l ON l.id = i.location_id \
         WHERE i.source_id = $1 FOR SHARE OF l",
    )
    .bind(params.source)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(place) = place else {
        return Ok(Outcome::Failed(format!(
            "the location {} is gone: {}",
            params.name,
            work.message(&params.name)
        )));
    };
    work.purged_before = place.purged_before;
    let root = PathBuf::from(&place.root);
    // An empty mount point would make every file gone.
    let (checked, name) = (root.clone(), params.name.clone());
    let before = match bounded(limit, move || {
        if std::fs::read_dir(&checked)?.next().is_none() {
            return Err(io::Error::other(format!(
                "{name} is not mounted at {} on this machine",
                checked.display()
            )));
        }
        Ok(identities(&[checked.as_path()]))
    })
    .await
    {
        Ok(before) => before,
        Err(error) if error.kind() == io::ErrorKind::TimedOut => {
            return Ok(hung(
                &format!("{} {error}", root.display()),
                &work,
                &params.name,
            ));
        }
        Err(error) => return Ok(Outcome::Failed(error.to_string())),
    };
    if !work.walked {
        let (found, _) = match walk(&root).await {
            Ok(found) => found,
            Err(why) => return Ok(hung(&why, &work, &params.name)),
        };
        for chunk in found.chunks(5_000) {
            let paths: Vec<&str> = chunk.iter().map(|f| f.rel_path.as_str()).collect();
            let sizes: Vec<i64> = chunk.iter().map(|f| f.size as i64).collect();
            let mtimes: Vec<DateTime<Utc>> = chunk.iter().map(|f| f.mtime.into()).collect();
            sqlx::query(
                "INSERT INTO ingest_files (source_id, rel_path, size, file_mtime, first_seen, seen_at, state) \
                 SELECT $1, u.rel_path, u.size, u.file_mtime, now(), now(), 'waiting' \
                 FROM unnest($2::text[], $3::bigint[], $4::timestamptz[]) AS u (rel_path, size, file_mtime) \
                 ON CONFLICT (source_id, rel_path) DO NOTHING",
            )
            .bind(params.source)
            .bind(paths)
            .bind(sizes)
            .bind(mtimes)
            .execute(&mut *conn)
            .await?;
        }
        let total: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM ingest_files f WHERE source_id = $1 AND {TO_TAKE}"
        ))
        .bind(params.source)
        .fetch_one(&mut *conn)
        .await?;
        work.walked = true;
        return Ok(Outcome::More {
            done: 0,
            total: Some(total),
            message: format!(
                "{} in {}: {total} to take",
                counted(found.len() as i64, "file", "files"),
                root.display()
            ),
            work: serde_json::to_value(&work).expect("plain data"),
        });
    }
    let batch: Vec<Row> = sqlx::query_as(&format!(
        "SELECT id, rel_path, size FROM ingest_files f \
         WHERE source_id = $1 AND id > $2 AND {TO_TAKE} ORDER BY id LIMIT $3"
    ))
    .bind(params.source)
    .bind(work.after)
    .bind(FILES_PER_STEP)
    .fetch_all(&mut *conn)
    .await?;
    if batch.is_empty() {
        return Ok(Outcome::Done(job.done, work.message(&params.name)));
    }
    // The months the files' calls fall in, each made before this step's
    // transaction touches the calls: making one locks the tables until the
    // transaction that made it ends.
    let months: BTreeSet<(i32, u32)> = batch
        .iter()
        .filter_map(|row| reader::start_time(row.rel_path.rsplit('/').next()?))
        .map(|local| started_at(local, place.offset_minutes, zone))
        .map(|at| (at.year(), at.month()))
        .collect();
    for (year, month) in months {
        let noon = NaiveDate::from_ymd_opt(year, month, 1)
            .and_then(|day| day.and_hms_opt(12, 0, 0))
            .expect("a month's first noon");
        store::ensure_partition(pool, noon.and_utc()).await?;
    }
    for row in &batch {
        let call = match read_call(&root, &row.rel_path, place.offset_minutes, zone, limit).await {
            Ok(Ok(call)) => call,
            Ok(Err(why)) => {
                store::reject(&mut *conn, row.id, &why).await?;
                work.rejected += 1;
                work.after = row.id;
                continue;
            }
            Err(error) => {
                unchanged(std::slice::from_ref(&root), &before, limit).await?;
                return Ok(hung(
                    &format!("{} {error}", root.display()),
                    &work,
                    &params.name,
                ));
            }
        };
        match classify(conn, params.source, &call, place.purged_before, zone).await? {
            Class::BeforePurge(day) => {
                let why = format!("from before {day}, before which {} was purged", params.name);
                store::reject(&mut *conn, row.id, &why).await?;
                work.before_purge += 1;
            }
            Class::UnknownSystem(system) => {
                let talkgroup = call
                    .described
                    .talkgroup
                    .as_ref()
                    .map(|tg| format!("{} {}", tg.number, tg.name));
                store::stage(&mut *conn, row.id, &system, talkgroup.as_deref()).await?;
                work.staged += 1;
            }
            Class::Copy(same) => {
                let why = format!(
                    "a copy of call {} from {}: the call keeps its recording",
                    same.id, same.recorder
                );
                let file = (row.size.unwrap_or(0), call.file_mtime);
                store::duplicate_in(conn, row.id, &same, params.source, file, &why).await?;
                work.copies += 1;
            }
            Class::New(_) => {
                let new = NewCall {
                    call: &call,
                    source_id: params.source,
                    location_id: place.location_id,
                    rel_path: &row.rel_path,
                    transcribe_from_ms: None,
                };
                store::record_in(conn, row.id, &new).await?;
                work.imported += 1;
            }
        }
        work.after = row.id;
    }
    unchanged(std::slice::from_ref(&root), &before, limit).await?;
    let done = job.done + batch.len() as i64;
    let total = job.total.map(|total| total.max(done));
    Ok(Outcome::More {
        done,
        total,
        message: format!(
            "{} files: {}",
            jobs::progress(done, total),
            work.message(&params.name)
        ),
        work: serde_json::to_value(&work).expect("plain data"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offset_is_suggested_only_when_it_finds_clearly_more_copies() {
        assert_eq!(suggest(&[0, 0, 0, 6, 0]), Some(60));
        assert_eq!(suggest(&[0, 0, 3, 6, 0]), Some(60));
        // Not twice as many as none, too few, or none is best.
        assert_eq!(suggest(&[0, 0, 4, 7, 0]), None);
        assert_eq!(suggest(&[0, 0, 0, 4, 0]), None);
        assert_eq!(suggest(&[0, 0, 9, 6, 0]), None);
    }

    #[test]
    fn a_reviews_sampled_counts_are_scaled_to_the_files_left() {
        let review = Review {
            name: "Old".into(),
            folder: "/srv/old".into(),
            files: 4003,
            no_date: 3,
            left: 4000,
            read: 1000,
            new: 600,
            copies: 400,
            copies_at: vec![0, 0, 400, 10, 0],
            examples: Examples {
                no_date: vec!["notes/readme.mp3".into()],
                ..Examples::default()
            },
            suggested: None,
            ..Review::default()
        };
        assert_eq!(
            review_text(&review),
            "Old: /srv/old\n  4003 mp3 files; 3 with no date and time in their names, skipped\n  \
             of the 4000 left, 1000 read: about 2400 new calls, about 1600 copies of calls already in the archive (skipped), \
             0 of systems not named yet (they wait), 0 unreadable (skipped)\n  \
             copies at each offset: -02:00 0, -01:00 0, +00:00 400, +01:00 10, +02:00 0\n  \
             no date and time in the name:\n    notes/readme.mp3\n"
        );
    }
}
