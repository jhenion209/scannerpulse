//! The transcription servers: how many requests each one takes at once, whose
//! turn it is, and the cooldown of a server that failed.
//!
//! A server that refuses, times out or answers 5xx cools down for 30 s, twice
//! as long for each failed round after that, 10 min at most. The requests that
//! fail while it cools down belong to the same round. Its first success ends
//! the cooldown.

use std::time::{Duration, Instant};

use crate::settings::Server;

/// The first cooldown of a server.
pub const COOLDOWN_FIRST: Duration = Duration::from_secs(30);

/// The longest cooldown.
pub const COOLDOWN_MAX: Duration = Duration::from_secs(600);

/// The cooldown after `rounds` failed rounds in a row (1 or more).
pub fn cooldown(rounds: u32) -> Duration {
    let doublings = rounds.saturating_sub(1).min(16);
    COOLDOWN_FIRST
        .saturating_mul(1 << doublings)
        .min(COOLDOWN_MAX)
}

/// One server as the admin page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerState {
    pub url: String,
    /// How much longer it cools down; `None` when it takes requests.
    pub cooling_for: Option<Duration>,
    pub failed_rounds: u32,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    url: String,
    weight: u32,
    busy: u32,
    failed_rounds: u32,
    cooling_until: Option<Instant>,
    last_error: Option<String>,
}

impl Entry {
    fn new(server: &Server) -> Entry {
        Entry {
            url: server.url.clone(),
            weight: server.weight,
            busy: 0,
            failed_rounds: 0,
            cooling_until: None,
            last_error: None,
        }
    }

    fn cooling(&self, now: Instant) -> bool {
        self.cooling_until.is_some_and(|until| now < until)
    }
}

/// The servers and their state.
#[derive(Debug, Clone, Default)]
pub struct Servers {
    list: Vec<Entry>,
    next: usize,
}

impl Servers {
    pub fn new(servers: &[Server]) -> Servers {
        Servers {
            list: servers.iter().map(Entry::new).collect(),
            next: 0,
        }
    }

    /// Takes a new list. A server that stays keeps its state and its requests
    /// in flight; a new one starts fresh.
    pub fn update(&mut self, servers: &[Server]) {
        let mut old = std::mem::take(&mut self.list);
        self.list = servers
            .iter()
            .map(
                |server| match old.iter().position(|e| e.url == server.url) {
                    Some(at) => Entry {
                        weight: server.weight,
                        ..old.swap_remove(at)
                    },
                    None => Entry::new(server),
                },
            )
            .collect();
        self.next = 0;
    }

    /// The free places, over the servers that do not cool down at `now`.
    pub fn free(&self, now: Instant) -> u32 {
        self.list
            .iter()
            .filter(|e| !e.cooling(now))
            .map(|e| e.weight.saturating_sub(e.busy))
            .sum()
    }

    /// The next server, in turn, that does not cool down and has a free
    /// place; takes the place. `None` when there is none.
    pub fn take(&mut self, now: Instant) -> Option<String> {
        let n = self.list.len();
        for step in 0..n {
            let at = (self.next + step) % n;
            let entry = &mut self.list[at];
            if !entry.cooling(now) && entry.busy < entry.weight {
                entry.busy += 1;
                self.next = (at + 1) % n;
                return Some(entry.url.clone());
            }
        }
        None
    }

    /// Gives back the place a request held.
    pub fn release(&mut self, url: &str) {
        if let Some(entry) = self.list.iter_mut().find(|e| e.url == url) {
            entry.busy = entry.busy.saturating_sub(1);
        }
    }

    /// Gives back every place; the cooldowns stay.
    pub fn release_all(&mut self) {
        for entry in &mut self.list {
            entry.busy = 0;
        }
    }

    /// A request failed for the server's sake. Starts a cooldown, unless the
    /// server cools down already: then it is the same round. True when a new
    /// round began.
    pub fn failed(&mut self, url: &str, error: &str, now: Instant) -> bool {
        let Some(entry) = self.list.iter_mut().find(|e| e.url == url) else {
            return false;
        };
        entry.last_error = Some(error.to_string());
        if entry.cooling(now) {
            return false;
        }
        entry.failed_rounds += 1;
        entry.cooling_until = Some(now + cooldown(entry.failed_rounds));
        true
    }

    /// The server answered. Ends its cooldown; true when it had one, or had
    /// failed rounds to forget.
    pub fn succeeded(&mut self, url: &str) -> bool {
        let Some(entry) = self.list.iter_mut().find(|e| e.url == url) else {
            return false;
        };
        let changed = entry.failed_rounds > 0 || entry.cooling_until.is_some();
        entry.failed_rounds = 0;
        entry.cooling_until = None;
        entry.last_error = None;
        changed
    }

    pub fn states(&self, now: Instant) -> Vec<ServerState> {
        self.list
            .iter()
            .map(|e| ServerState {
                url: e.url.clone(),
                cooling_for: e
                    .cooling_until
                    .filter(|until| now < *until)
                    .map(|until| until - now),
                failed_rounds: e.failed_rounds,
                last_error: e.last_error.clone(),
            })
            .collect()
    }
}
