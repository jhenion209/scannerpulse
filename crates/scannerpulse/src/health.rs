//! Path health. Every 10 s the host reads the archive root and each watched
//! recorder's folder, with a 5 s limit, each in a thread of its own, so that
//! a hung mount cannot block the process. When the archive, or every watched
//! folder, has failed for 60 s, the host is unhealthy: it gives up the lease
//! and takes none until its paths answer again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthTimings {
    pub every: Duration,
    pub timeout: Duration,
    pub fail_after: Duration,
}

impl Default for HealthTimings {
    fn default() -> HealthTimings {
        HealthTimings {
            every: Duration::from_secs(10),
            timeout: Duration::from_secs(5),
            fail_after: Duration::from_secs(60),
        }
    }
}

/// How a path is read. Blocking: it runs in a thread of its own.
pub type Probe = fn(&Path) -> io::Result<()>;

/// Opens the folder and reads its first entry.
pub fn read_dir_probe(path: &Path) -> io::Result<()> {
    fs::read_dir(path)?.next().transpose()?;
    Ok(())
}

struct Check {
    path: PathBuf,
    failing_since: Option<Instant>,
    /// A probe still running from an earlier round: a hung path gets no second one.
    running: Option<JoinHandle<io::Result<()>>>,
}

impl Check {
    fn new(path: &Path) -> Check {
        Check {
            path: path.to_path_buf(),
            failing_since: None,
            running: None,
        }
    }

    fn failed(&self, now: Instant, fail_after: Duration) -> bool {
        self.failing_since
            .is_some_and(|since| now.duration_since(since) >= fail_after)
    }
}

pub struct Health {
    archive: Check,
    sources: Vec<Check>,
    timings: HealthTimings,
    probe: Probe,
}

impl Health {
    pub fn new(archive: &Path, sources: &[&Path], timings: HealthTimings, probe: Probe) -> Health {
        Health {
            archive: Check::new(archive),
            sources: sources.iter().map(|path| Check::new(path)).collect(),
            timings,
            probe,
        }
    }

    pub fn timings(&self) -> HealthTimings {
        self.timings
    }

    /// Checks these sources from now on; a source checked before keeps its
    /// state, so that a change of recorders does not hide a failing one.
    pub fn set_sources(&mut self, sources: &[&Path]) {
        let mut old = std::mem::take(&mut self.sources);
        self.sources = sources
            .iter()
            .map(
                |path| match old.iter().position(|check| check.path == *path) {
                    Some(at) => old.swap_remove(at),
                    None => Check::new(path),
                },
            )
            .collect();
    }

    /// One round: reads every path, and returns when each has answered or
    /// its time is up.
    pub async fn check(&mut self) {
        let started = Instant::now();
        let probe = self.probe;
        let checks = std::iter::once(&mut self.archive).chain(self.sources.iter_mut());
        let mut checks: Vec<&mut Check> = checks.collect();
        for check in checks.iter_mut() {
            if check
                .running
                .as_ref()
                .is_some_and(|probe| !probe.is_finished())
            {
                continue;
            }
            let path = check.path.clone();
            check.running = Some(tokio::task::spawn_blocking(move || probe(&path)));
        }
        let deadline = tokio::time::Instant::from_std(started + self.timings.timeout);
        for check in checks {
            let Some(running) = check.running.as_mut() else {
                continue;
            };
            let answered = match tokio::time::timeout_at(deadline, running).await {
                Ok(result) => matches!(result, Ok(Ok(()))),
                Err(_) => {
                    // Still running: kept, so that the path gets no second probe.
                    check.failing_since.get_or_insert(started);
                    continue;
                }
            };
            check.running = None;
            if answered {
                check.failing_since = None;
            } else {
                check.failing_since.get_or_insert(started);
            }
        }
    }

    /// False once the archive, or every source, has failed for `fail_after`.
    pub fn healthy(&self) -> bool {
        let now = Instant::now();
        let fail_after = self.timings.fail_after;
        let sources_failed = !self.sources.is_empty()
            && self
                .sources
                .iter()
                .all(|check| check.failed(now, fail_after));
        !self.archive.failed(now, fail_after) && !sources_failed
    }
}
