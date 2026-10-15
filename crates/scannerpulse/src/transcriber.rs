//! The transcriber, on the lease holder only: it sends the queued calls that
//! are due, newest first, to the transcription servers, as many at once as
//! their weights allow, and writes each result.
//!
//! A server's failure (refused, timed out, 5xx) puts the server on a cooldown
//! and leaves the call queued with no failure counted, so an outage never
//! fails calls. From the second server fault on one call, the call's next
//! try moves along `queue::RETRY_AFTER`, so it stops recurring. A refusal of
//! the call itself (4xx), or audio that cannot be read, counts against the
//! call. After a lost lease nothing is written.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use sqlx::postgres::PgPool;
use tokio::task::JoinSet;

use crate::asr::{AsrError, Client, Request};
use crate::filer::Fence;
use crate::log;
use crate::queue::{self, Queued, Written};
use crate::reader::MAX_FILE_BYTES;
use crate::servers::{ServerState, Servers};
use crate::settings::Transcription;
use crate::store::{self, RecordError};

const DEFAULT_DB_LIMIT: Duration = Duration::from_secs(30);

/// How long a call whose result could not be written is kept out of `fill`.
pub const RETRY_HOLD: Duration = Duration::from_secs(5);

/// What became of one queued call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Its result is written: text, no speech, or a person's text kept.
    Written(i64, Written),
    /// The call failed and is tried again later.
    Retry { call: i64, error: String },
    /// The call failed for the fifth time: it is marked failed.
    Failed { call: i64, error: String },
    /// The server failed; the call stays queued with nothing counted.
    /// `deferred` when the servers failed on it before, so its next try is
    /// moved along `queue::RETRY_AFTER`.
    ServerDown {
        call: i64,
        server: String,
        error: String,
        deferred: bool,
    },
    /// The lease was lost before the result could be written.
    LeaseLost(i64),
}

/// A request that has ended, waiting to be written.
#[derive(Debug)]
pub struct Done {
    queued: Queued,
    server: String,
    model: String,
    result: Result<String, AsrError>,
}

pub struct Transcriber {
    pool: PgPool,
    fence: Arc<dyn Fence>,
    client: Client,
    settings: Option<Arc<Transcription>>,
    servers: Servers,
    jobs: JoinSet<Done>,
    in_flight: HashSet<i64>,
    /// Server faults per call, since its last other outcome.
    faults: HashMap<i64, u16>,
    /// Calls whose result could not be written, held out until then.
    held: HashMap<i64, Instant>,
    db_limit: Duration,
    servers_changed: bool,
}

impl Transcriber {
    pub fn new(pool: PgPool, fence: Arc<dyn Fence>, client: Client) -> Transcriber {
        Transcriber {
            pool,
            fence,
            client,
            settings: None,
            servers: Servers::default(),
            jobs: JoinSet::new(),
            in_flight: HashSet::new(),
            faults: HashMap::new(),
            held: HashMap::new(),
            db_limit: DEFAULT_DB_LIMIT,
            servers_changed: false,
        }
    }

    /// Sets the limit on each database call (default 30 s).
    pub fn with_db_limit(mut self, limit: Duration) -> Transcriber {
        self.db_limit = limit;
        self
    }

    /// Takes new settings; `None` turns transcription off. Requests in flight
    /// finish and are written.
    pub fn set(&mut self, settings: Option<Transcription>) {
        let servers = settings
            .as_ref()
            .map(|s| s.servers.clone())
            .unwrap_or_default();
        self.servers.update(&servers);
        self.settings = settings.map(Arc::new);
        self.servers_changed = true;
    }

    pub fn settings(&self) -> Option<&Transcription> {
        self.settings.as_deref()
    }

    /// The requests in flight.
    pub fn busy(&self) -> usize {
        self.jobs.len()
    }

    pub fn server_states(&self) -> Vec<ServerState> {
        self.servers.states(Instant::now())
    }

    /// The servers' state when it changed since the last call.
    pub fn take_server_change(&mut self) -> Option<Vec<ServerState>> {
        if std::mem::take(&mut self.servers_changed) {
            Some(self.server_states())
        } else {
            None
        }
    }

    /// Sends due calls while the servers have free places; returns how many
    /// requests it started. Sends nothing while transcription is off or the
    /// lease is not held.
    pub async fn fill(&mut self) -> sqlx::Result<usize> {
        let Some(settings) = self.settings.clone() else {
            return Ok(0);
        };
        if !self.fence.held() {
            return Ok(0);
        }
        let now = Instant::now();
        let free = self.servers.free(now);
        if free == 0 {
            return Ok(0);
        }
        self.held.retain(|_, until| now < *until);
        let skip: Vec<i64> = self
            .in_flight
            .iter()
            .chain(self.held.keys())
            .copied()
            .collect();
        let due = store::limited(self.db_limit, queue::due(&self.pool, free.into(), &skip)).await?;
        let mut started = 0;
        for queued in due {
            let Some(server) = self.servers.take(now) else {
                break;
            };
            self.in_flight.insert(queued.call_id);
            self.jobs.spawn(request(
                self.client.clone(),
                settings.clone(),
                server,
                queued,
            ));
            started += 1;
        }
        Ok(started)
    }

    /// Waits for the next request to end; `None` when none is in flight.
    /// Cancel safe: nothing is lost when the wait is dropped.
    pub async fn next_done(&mut self) -> Option<Done> {
        loop {
            match self.jobs.join_next().await? {
                Ok(done) => return Some(done),
                Err(error) => {
                    log::error(format!("a transcription request ended badly: {error}"));
                    if self.jobs.is_empty() {
                        // No request is in flight: every place is free again.
                        self.free_all();
                    }
                }
            }
        }
    }

    /// Writes the result of a request that has ended. After a database error
    /// the call is held out of `fill` for `RETRY_HOLD`.
    pub async fn settle(&mut self, done: Done) -> sqlx::Result<Outcome> {
        let call = done.queued.call_id;
        let settled = self.write(done).await;
        if settled.is_err() {
            self.held.insert(call, Instant::now() + RETRY_HOLD);
        }
        settled
    }

    async fn write(&mut self, done: Done) -> sqlx::Result<Outcome> {
        let Done {
            queued,
            server,
            model,
            result,
        } = done;
        self.servers.release(&server);
        self.in_flight.remove(&queued.call_id);
        let call = queued.call_id;
        let error = match result {
            Err(AsrError::Server(error)) => {
                if self.servers.failed(&server, &error, Instant::now()) {
                    self.servers_changed = true;
                }
                let faults = self.faults.entry(call).or_insert(0);
                *faults = faults.saturating_add(1);
                let faults = *faults;
                if faults < 2 {
                    return Ok(Outcome::ServerDown {
                        call,
                        server,
                        error,
                        deferred: false,
                    });
                }
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost(call));
                }
                let mut conn = store::limited(self.db_limit, self.pool.acquire()).await?;
                let holder = self.fence.holder().to_string();
                let defer = queue::defer(&mut conn, &holder, &queued, faults);
                return match tokio::time::timeout(self.db_limit, defer).await {
                    Ok(Ok(())) => Ok(Outcome::ServerDown {
                        call,
                        server,
                        error,
                        deferred: true,
                    }),
                    Ok(Err(RecordError::LeaseLost)) => Ok(Outcome::LeaseLost(call)),
                    Ok(Err(RecordError::Database(error))) => Err(error),
                    Err(_) => Err(store::timed_out(self.db_limit)),
                };
            }
            Ok(text) => {
                self.faults.remove(&call);
                if self.servers.succeeded(&server) {
                    self.servers_changed = true;
                }
                if !self.fence.held() {
                    return Ok(Outcome::LeaseLost(call));
                }
                let mut conn = store::limited(self.db_limit, self.pool.acquire()).await?;
                let holder = self.fence.holder().to_string();
                let write =
                    queue::write(&mut conn, &holder, (call, queued.started_at), &text, &model);
                return match tokio::time::timeout(self.db_limit, write).await {
                    Ok(Ok(written)) => Ok(Outcome::Written(call, written)),
                    Ok(Err(RecordError::LeaseLost)) => Ok(Outcome::LeaseLost(call)),
                    Ok(Err(RecordError::Database(error))) => Err(error),
                    Err(_) => Err(store::timed_out(self.db_limit)),
                };
            }
            Err(AsrError::Call(error)) => {
                self.faults.remove(&call);
                if self.servers.succeeded(&server) {
                    self.servers_changed = true;
                }
                error
            }
            Err(AsrError::Audio(error)) => {
                self.faults.remove(&call);
                error
            }
        };
        if !self.fence.held() {
            return Ok(Outcome::LeaseLost(call));
        }
        let mut conn = store::limited(self.db_limit, self.pool.acquire()).await?;
        let holder = self.fence.holder().to_string();
        let count = queue::count_failure(&mut conn, &holder, &queued, &error);
        match tokio::time::timeout(self.db_limit, count).await {
            Ok(Ok(true)) => Ok(Outcome::Failed { call, error }),
            Ok(Ok(false)) => Ok(Outcome::Retry { call, error }),
            Ok(Err(RecordError::LeaseLost)) => Ok(Outcome::LeaseLost(call)),
            Ok(Err(RecordError::Database(error))) => Err(error),
            Err(_) => Err(store::timed_out(self.db_limit)),
        }
    }

    /// Ends every request in flight; their calls stay queued.
    pub fn abort(&mut self) {
        self.jobs.abort_all();
        self.jobs.detach_all();
        self.free_all();
    }

    /// Forgets the places of the requests in flight; the cooldowns stay.
    fn free_all(&mut self) {
        self.in_flight.clear();
        self.servers.release_all();
    }
}

/// One request: reads the call's file and sends it.
async fn request(
    client: Client,
    settings: Arc<Transcription>,
    server: String,
    queued: Queued,
) -> Done {
    let path = queued.path();
    let result = match read_audio(&path).await {
        Err(why) => Err(AsrError::Audio(why)),
        Ok(audio) => {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("call.mp3")
                .to_string();
            let request = Request {
                url: &server,
                model: &settings.model,
                language: settings.language.as_deref(),
                prompt: settings.prompt.as_deref(),
                timeout: settings.request_timeout,
            };
            client.transcribe(&request, &name, audio).await
        }
    };
    Done {
        queued,
        server,
        model: settings.model.clone(),
        result,
    }
}

/// The bytes of a call's file, read off the async threads; a file larger
/// than the reader's limit is not sent.
async fn read_audio(path: &Path) -> Result<Vec<u8>, String> {
    let path = path.to_path_buf();
    let shown = path.display().to_string();
    let read = tokio::task::spawn_blocking(move || {
        let size = std::fs::metadata(&path)?.len();
        if size > MAX_FILE_BYTES {
            return Err(std::io::Error::other(format!(
                "the file is larger than {MAX_FILE_BYTES} bytes"
            )));
        }
        std::fs::read(&path)
    })
    .await
    .map_err(|error| format!("{shown} could not be read: {error}"))?;
    read.map_err(|error| format!("{shown} could not be read: {error}"))
}
