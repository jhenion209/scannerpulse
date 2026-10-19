//! The install's plan for its database nodes: made by setup on the first
//! ScannerPulse machine (the hub, which the others join through) and read
//! by each node as it joins. It holds the database's passwords, so it is
//! root's alone.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub const PLAN: &str = "/etc/scannerpulse/plan.json";

/// Where the hub keeps its key for the nodes' commands, and their host keys.
pub const SSH: &str = "/etc/scannerpulse/ssh";

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Plan {
    /// The hub's address, as the nodes reach it.
    pub hub: String,
    /// The hub's public key, which each node lets run its node commands.
    pub hub_key: String,
    /// The database nodes, by the names or addresses the person gave; the
    /// first starts as the primary.
    pub nodes: Vec<String>,
    pub database: String,
    pub role: String,
    pub password: String,
    pub replication_password: String,
    /// The referee, when the standby is promoted by itself (pg_auto_failover's
    /// monitor); none when it is promoted by hand.
    #[serde(default)]
    pub referee: Option<String>,
    /// The password the database machines reach the referee with.
    #[serde(default)]
    pub monitor_password: String,
    /// The machines that have joined.
    #[serde(default)]
    pub joined: Vec<String>,
}

/// A machine's place in the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    /// Database machine N (0 starts as the primary).
    Node(usize),
    Referee,
}

/// Never a password.
impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("hub", &self.hub)
            .field("nodes", &self.nodes)
            .field("referee", &self.referee)
            .field("joined", &self.joined)
            .finish_non_exhaustive()
    }
}

impl Plan {
    pub fn read(path: &Path) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|error| {
            format!(
                "{}: {error} (is this the install's first machine?)",
                path.display()
            )
        })?;
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Writes the plan whole, root's alone, and on disk before it is
    /// named.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let fail = |error: std::io::Error| format!("cannot write {}: {error}", path.display());
        let new = path.with_extension("new");
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        // A file left by an earlier write could have another mode.
        match std::fs::remove_file(&new) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(fail(error)),
            _ => {}
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&new)
            .map_err(fail)?;
        file.write_all(text.as_bytes()).map_err(fail)?;
        file.sync_all().map_err(fail)?;
        std::fs::rename(&new, path).map_err(fail)?;
        if let Some(folder) = path.parent() {
            std::fs::File::open(folder)
                .and_then(|f| f.sync_all())
                .map_err(fail)?;
        }
        Ok(())
    }

    /// Changes the plan at `path` with `change`, under its lock, so that
    /// two machines joining at once each keep their mark.
    pub fn update(path: &Path, change: impl FnOnce(&mut Plan)) -> Result<(), String> {
        Plan::replace(path, |kept| {
            let mut plan = kept.ok_or_else(|| Plan::read(path).unwrap_err())?;
            change(&mut plan);
            Ok(plan)
        })
    }

    /// Writes the plan `make` gives from the plan at `path`, if any, under
    /// its lock.
    pub fn replace(
        path: &Path,
        make: impl FnOnce(Option<Plan>) -> Result<Plan, String>,
    ) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        let lock_path = path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(|error| format!("{}: {error}", lock_path.display()))?;
        rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
            .map_err(|error| format!("{}: {error}", lock_path.display()))?;
        let kept = path.exists().then(|| Plan::read(path)).transpose()?;
        make(kept)?.write(path)
    }

    /// Which machine of the plan a machine is, by its own names and
    /// addresses (`mine`): one named as one of them, or whose name
    /// `resolve`s to one.
    pub fn place(&self, mine: &[String], resolve: impl Fn(&str) -> Vec<IpAddr>) -> Option<Place> {
        let mine_lower: Vec<String> = mine.iter().map(|m| m.to_ascii_lowercase()).collect();
        let addresses: Vec<IpAddr> = mine.iter().filter_map(|m| m.parse().ok()).collect();
        let is = |name: &String| {
            mine_lower.contains(&name.to_ascii_lowercase())
                || resolve(name).iter().any(|ip| addresses.contains(ip))
        };
        if let Some(i) = self.nodes.iter().position(is) {
            return Some(Place::Node(i));
        }
        self.referee
            .as_ref()
            .filter(|r| is(r))
            .map(|_| Place::Referee)
    }

    /// The machine at `place`.
    pub fn name(&self, place: Place) -> &str {
        match place {
            Place::Node(i) => &self.nodes[i],
            Place::Referee => self.referee.as_deref().unwrap_or_default(),
        }
    }

    /// Notes that `node` has joined.
    pub fn mark_joined(&mut self, node: &str) {
        if !self.joined.iter().any(|n| n == node) {
            self.joined.push(node.to_string());
        }
    }

    /// The first machine that joins before the one at `place` and has not
    /// yet: the referee comes first, then database machine 1, then 2.
    pub fn awaited_before(&self, place: Place) -> Option<&str> {
        let mine = self.name(place);
        self.referee
            .iter()
            .chain(&self.nodes)
            .take_while(|n| n.as_str() != mine)
            .find(|n| !self.joined.contains(n))
            .map(String::as_str)
    }

    /// The machines not yet joined, in the order they join: the referee,
    /// then the database machines.
    pub fn waiting(&self) -> Vec<&str> {
        self.referee
            .iter()
            .chain(&self.nodes)
            .filter(|n| !self.joined.contains(n))
            .map(String::as_str)
            .collect()
    }
}

/// The database's password in a plan kept for the same machines: setup
/// run again on the first machine keeps it, since the first database
/// machine already holds it.
pub fn kept_password(kept: Option<Plan>, nodes: &[String]) -> Option<String> {
    kept.filter(|plan| plan.nodes == nodes)
        .map(|plan| plan.password)
}

/// The addresses `name` resolves to (an address resolves to itself).
pub fn resolve(name: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    if let Ok(ip) = name.parse::<IpAddr>() {
        return vec![ip];
    }
    (name, 0)
        .to_socket_addrs()
        .map(|found| found.map(|a| a.ip()).collect())
        .unwrap_or_default()
}

pub fn plan_path() -> PathBuf {
    PathBuf::from(PLAN)
}

#[cfg(test)]
mod tests {
    use super::{Place, Plan};
    use std::net::IpAddr;

    fn plan() -> Plan {
        Plan {
            hub: "192.0.2.10".into(),
            hub_key: "ssh-ed25519 AAAA hub".into(),
            nodes: vec!["db1.example.org".into(), "192.0.2.22".into()],
            database: "scannerpulse".into(),
            role: "scannerpulse".into(),
            password: "app-secret-1".into(),
            replication_password: "copy-secret-2".into(),
            referee: None,
            monitor_password: String::new(),
            joined: Vec::new(),
        }
    }

    /// The same machines, the standby promoted by a referee.
    fn refereed() -> Plan {
        Plan {
            referee: Some("ref.example.org".into()),
            monitor_password: "referee-secret-3".into(),
            ..plan()
        }
    }

    #[test]
    fn a_machine_finds_its_place_by_name_or_address() {
        let none = |_: &str| Vec::<IpAddr>::new();
        let plan = plan();
        assert_eq!(
            plan.place(&["DB1.example.org".into()], none),
            Some(Place::Node(0))
        );
        assert_eq!(
            plan.place(&["db2".into(), "192.0.2.22".into()], none),
            Some(Place::Node(1))
        );
        // A node named by its name, the machine knowing only its address.
        let dns = |name: &str| {
            if name == "db1.example.org" {
                vec!["192.0.2.21".parse().unwrap()]
            } else {
                Vec::new()
            }
        };
        assert_eq!(
            plan.place(&["192.0.2.21".into()], dns),
            Some(Place::Node(0))
        );
        assert_eq!(plan.place(&["web.example.org".into()], none), None);
        // The referee, only in a plan that has one.
        assert_eq!(plan.place(&["ref.example.org".into()], none), None);
        assert_eq!(
            refereed().place(&["REF.example.org".into()], none),
            Some(Place::Referee)
        );
    }

    #[test]
    fn setup_run_again_for_the_same_machines_keeps_the_databases_password() {
        // The first machine holds the password the plan gave it.
        let nodes = plan().nodes;
        assert_eq!(
            super::kept_password(Some(plan()), &nodes).as_deref(),
            Some("app-secret-1")
        );
        let other = vec!["db1.example.org".to_string(), "db3.example.org".to_string()];
        assert_eq!(super::kept_password(Some(plan()), &other), None);
        assert_eq!(super::kept_password(None, &nodes), None);
    }

    #[test]
    fn the_plan_keeps_who_joined_and_who_is_awaited() {
        let mut plan = plan();
        assert_eq!(plan.waiting(), ["db1.example.org", "192.0.2.22"]);
        plan.mark_joined("db1.example.org");
        plan.mark_joined("db1.example.org");
        assert_eq!(plan.joined, ["db1.example.org"]);
        assert_eq!(plan.waiting(), ["192.0.2.22"]);
        // The referee comes first, then the database machines.
        let mut plan = refereed();
        assert_eq!(
            plan.waiting(),
            ["ref.example.org", "db1.example.org", "192.0.2.22"]
        );
        plan.mark_joined("ref.example.org");
        assert_eq!(plan.waiting(), ["db1.example.org", "192.0.2.22"]);
    }

    #[test]
    fn a_machine_waits_for_the_ones_that_join_before_it() {
        let mut plan = refereed();
        assert_eq!(plan.awaited_before(Place::Referee), None);
        assert_eq!(plan.awaited_before(Place::Node(0)), Some("ref.example.org"));
        assert_eq!(plan.awaited_before(Place::Node(1)), Some("ref.example.org"));
        plan.mark_joined("ref.example.org");
        assert_eq!(plan.awaited_before(Place::Node(0)), None);
        assert_eq!(plan.awaited_before(Place::Node(1)), Some("db1.example.org"));
        // Promoted by hand: the second waits for the first only.
        assert_eq!(super::tests::plan().awaited_before(Place::Node(0)), None);
        assert_eq!(
            super::tests::plan().awaited_before(Place::Node(1)),
            Some("db1.example.org")
        );
    }

    #[test]
    fn a_plan_is_changed_under_a_lock() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        plan().write(&path).unwrap();
        // A stale file of another mode where the new one is written.
        std::fs::write(path.with_extension("new"), "stale").unwrap();
        std::fs::set_permissions(
            path.with_extension("new"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        Plan::update(&path, |plan| plan.mark_joined("db1.example.org")).unwrap();
        assert_eq!(Plan::read(&path).unwrap().joined, ["db1.example.org"]);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(path.with_extension("lock").exists());
    }

    #[test]
    fn the_plan_is_root_s_alone_and_never_shows_a_password() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        plan().write(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(Plan::read(&path).unwrap(), plan());
        let shown = format!("{:?}", refereed());
        assert!(!shown.contains("secret"), "{shown}");
        // A plan written before the referee reads as one promoted by hand.
        let old = r#"{"hub":"h","hub_key":"k","nodes":["a","b"],"database":"d","role":"r","password":"p","replication_password":"q"}"#;
        let old: Plan = serde_json::from_str(old).unwrap();
        assert_eq!((old.referee, old.monitor_password.as_str()), (None, ""));
    }
}
