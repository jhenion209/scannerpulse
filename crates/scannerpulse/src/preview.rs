//! A recorder's preview: a sample of the files in its folder, read as the
//! filer reads them, each with what filing it would do. It writes nothing,
//! deletes nothing and learns no names: the owner's test of a new recorder.

use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use sqlx::PgConnection;

use crate::reader::{self, FileCall, ReadError};
use crate::recorders::Refused;
use crate::store::{self, NameSeen};
use crate::{archive, watcher};

/// The most files a preview reads.
pub const SAMPLE_FILES: usize = 50;

/// How long the recorder's folder has to list itself and give the sample.
pub const PREVIEW_LIMIT: Duration = Duration::from_secs(60);

/// What filing would do with a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Verdict {
    /// Filed as a new call.
    Filed,
    /// Filed under a talkgroup added with the file's name.
    NewTalkgroup,
    /// Filed, and the talkgroup's placeholder name gives way to the file's.
    Named { current: String },
    /// Filed, and the file's other name for the talkgroup is a proposal.
    Proposal { current: String },
    /// Its system name is unknown: it waits in the staging folder.
    Staged,
    /// Rejected, and deleted from the recorder's folder.
    Rejected { reason: String },
    /// It cannot be read now: the filer tries again later.
    Unreadable { reason: String },
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Filed => f.write_str("filed"),
            Verdict::NewTalkgroup => f.write_str("filed, new talkgroup"),
            Verdict::Named { current } => write!(f, "filed, names the talkgroup (now {current:?})"),
            Verdict::Proposal { current } => write!(f, "filed, a name proposal (now {current:?})"),
            Verdict::Staged => f.write_str("staged: unknown system"),
            Verdict::Rejected { reason } => write!(f, "rejected: {reason}"),
            Verdict::Unreadable { reason } => write!(f, "cannot be read now: {reason}"),
        }
    }
}

/// How many files of the sample have each verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub filed: usize,
    pub new_talkgroup: usize,
    pub named: usize,
    pub proposal: usize,
    pub staged: usize,
    pub rejected: usize,
    pub unreadable: usize,
}

impl Counts {
    fn add(&mut self, verdict: &Verdict) {
        *match verdict {
            Verdict::Filed => &mut self.filed,
            Verdict::NewTalkgroup => &mut self.new_talkgroup,
            Verdict::Named { .. } => &mut self.named,
            Verdict::Proposal { .. } => &mut self.proposal,
            Verdict::Staged => &mut self.staged,
            Verdict::Rejected { .. } => &mut self.rejected,
            Verdict::Unreadable { .. } => &mut self.unreadable,
        } += 1;
    }
}

/// A file of the sample, as the reader reads it.
#[derive(Debug, Clone, Serialize)]
pub struct Sampled {
    /// Its path in the recorder's folder.
    pub rel_path: String,
    pub started_at: Option<DateTime<Utc>>,
    pub system: Option<String>,
    pub site: Option<String>,
    pub talkgroup: Option<i64>,
    pub talkgroup_name: Option<String>,
    pub radio_id: Option<i64>,
    pub radio_alias: Option<String>,
    pub duration_ms: Option<i32>,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub recorder: String,
    /// The audio files in its folder.
    pub files: usize,
    pub counts: Counts,
    /// In path order.
    pub sample: Vec<Sampled>,
}

/// At most [`SAMPLE_FILES`] of the paths, evenly spaced in path order, so
/// that the sample spans the folder.
fn spread(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    let n = paths.len();
    if n <= SAMPLE_FILES {
        return paths;
    }
    (0..SAMPLE_FILES)
        .map(|i| paths[i * n / SAMPLE_FILES].clone())
        .collect()
}

type Read = (usize, Vec<(String, Result<FileCall, ReadError>)>);

/// Previews the recorder `id`. The folder is listed and read in a thread of
/// its own, within [`PREVIEW_LIMIT`]: a share that stopped answering holds
/// up nothing else.
pub async fn preview(conn: &mut PgConnection, id: i16, zone: Tz) -> Result<Preview, Refused> {
    let found: Option<(String, String)> =
        sqlx::query_as("SELECT name, path FROM sources WHERE id = $1 AND removed_at IS NULL")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((name, folder)) = found else {
        return Err(Refused::NoSuch(format!("there is no recorder {id}")));
    };
    let folder = PathBuf::from(folder);
    let root = folder.clone();
    let job: watcher::Job<Read> = Box::new(move || {
        let listing = watcher::list_folder(&root)?;
        let files = listing.found.len();
        let paths = spread(
            listing
                .found
                .into_iter()
                .map(|found| found.rel_path)
                .collect(),
        );
        let read = paths
            .into_iter()
            .map(|rel| {
                let call = reader::read_path(&root.join(&rel), zone).map(|(call, _)| call);
                (rel, call)
            })
            .collect();
        Ok((files, read))
    });
    let answered = watcher::run_bounded(
        vec![(folder.clone(), job)],
        &mut watcher::Hung::default(),
        PREVIEW_LIMIT,
    )
    .await
    .pop()
    .unwrap_or_else(|| Err(std::io::Error::other("the read was not run")));
    let (files, read) = answered
        .map_err(|error| Refused::Refused(format!("the folder {}: {error}", folder.display())))?;
    let mut counts = Counts::default();
    let mut sample = Vec::with_capacity(read.len());
    for (rel_path, call) in read {
        let (call, verdict) = match call {
            Ok(call) => {
                let verdict = verdict(conn, &rel_path, &call).await?;
                (Some(call), verdict)
            }
            Err(ReadError::Rejected(reason)) => (None, Verdict::Rejected { reason }),
            Err(ReadError::Io(error)) => (
                None,
                Verdict::Unreadable {
                    reason: error.to_string(),
                },
            ),
        };
        counts.add(&verdict);
        let described = call.as_ref().map(|call| &call.described);
        let talkgroup = described.and_then(|described| described.talkgroup.as_ref());
        sample.push(Sampled {
            started_at: call.as_ref().map(|call| call.started_at),
            system: described.and_then(|described| described.system.clone()),
            site: described.and_then(|described| described.site.clone()),
            talkgroup: talkgroup.map(|talkgroup| talkgroup.number),
            talkgroup_name: talkgroup.map(|talkgroup| talkgroup.name.clone()),
            radio_id: described.and_then(|described| described.radio_id),
            radio_alias: described.and_then(|described| described.radio_alias.clone()),
            duration_ms: call.as_ref().map(|call| call.duration_ms),
            rel_path,
            verdict,
        });
    }
    Ok(Preview {
        recorder: name,
        files,
        counts,
        sample,
    })
}

/// What filing would do with the call read from the file at `rel_path`, as
/// `Filer::try_file` and `store::talkgroup_id` decide it.
async fn verdict(
    conn: &mut PgConnection,
    rel_path: &str,
    call: &FileCall,
) -> sqlx::Result<Verdict> {
    let described = &call.described;
    if let Some(system) = described.system.as_deref()
        && !store::system_known(&mut *conn, system).await?
    {
        return Ok(Verdict::Staged);
    }
    let talkgroup = described.talkgroup.as_ref();
    let folder = archive::folder(talkgroup.map(|tg| (tg.number, tg.name.as_str())));
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    if let Err(reason) = archive::rel_path(call.local_start.date(), &folder, name) {
        return Ok(Verdict::Rejected { reason });
    }
    let (Some(system), Some(talkgroup)) = (described.system.as_deref(), talkgroup) else {
        return Ok(Verdict::Filed);
    };
    // No system yet on a fresh install: its first is added with the call.
    let Some((system_id, _)) = store::find_system(&mut *conn, system).await? else {
        return Ok(Verdict::NewTalkgroup);
    };
    let stored: Option<(String, bool)> = sqlx::query_as(
        "SELECT t.name, EXISTS (SELECT 1 FROM name_proposals p \
                               WHERE p.talkgroup_id = t.id AND p.name = $3 AND p.dismissed) \
         FROM talkgroups t WHERE t.system_id = $1 AND t.number = $2",
    )
    .bind(system_id)
    .bind(talkgroup.number)
    .bind(&talkgroup.name)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((current, dismissed)) = stored else {
        return Ok(Verdict::NewTalkgroup);
    };
    Ok(match store::name_seen(&current, &talkgroup.name) {
        NameSeen::Same => Verdict::Filed,
        NameSeen::Rename => Verdict::Named { current },
        NameSeen::Propose if dismissed => Verdict::Filed,
        NameSeen::Propose => Verdict::Proposal { current },
    })
}
