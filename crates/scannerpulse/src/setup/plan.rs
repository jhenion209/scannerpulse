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
    /// The nodes that have joined.
    #[serde(default)]
    pub joined: Vec<String>,
}

/// Never a password.
impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plan")
            .field("hub", &self.hub)
            .field("nodes", &self.nodes)
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

    /// Writes the plan whole, root's alone.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let new = path.with_extension("new");
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&new)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .and_then(|()| std::fs::rename(&new, path))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))
    }

    /// Which node a machine is, by its own names and addresses (`mine`):
    /// a node named as one of them, or whose name `resolve`s to one.
    pub fn place(&self, mine: &[String], resolve: impl Fn(&str) -> Vec<IpAddr>) -> Option<usize> {
        let mine_lower: Vec<String> = mine.iter().map(|m| m.to_ascii_lowercase()).collect();
        let addresses: Vec<IpAddr> = mine.iter().filter_map(|m| m.parse().ok()).collect();
        self.nodes.iter().position(|node| {
            mine_lower.contains(&node.to_ascii_lowercase())
                || resolve(node).iter().any(|ip| addresses.contains(ip))
        })
    }

    /// Notes that `node` has joined.
    pub fn mark_joined(&mut self, node: &str) {
        if !self.joined.iter().any(|n| n == node) {
            self.joined.push(node.to_string());
        }
    }

    /// The nodes not yet joined, in order.
    pub fn waiting(&self) -> Vec<&str> {
        self.nodes
            .iter()
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
    use super::Plan;
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
            joined: Vec::new(),
        }
    }

    #[test]
    fn a_machine_finds_its_place_by_name_or_address() {
        let none = |_: &str| Vec::<IpAddr>::new();
        let plan = plan();
        assert_eq!(plan.place(&["DB1.example.org".into()], none), Some(0));
        assert_eq!(
            plan.place(&["db2".into(), "192.0.2.22".into()], none),
            Some(1)
        );
        // A node named by its name, the machine knowing only its address.
        let dns = |name: &str| {
            if name == "db1.example.org" {
                vec!["192.0.2.21".parse().unwrap()]
            } else {
                Vec::new()
            }
        };
        assert_eq!(plan.place(&["192.0.2.21".into()], dns), Some(0));
        assert_eq!(plan.place(&["web.example.org".into()], none), None);
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
        let shown = format!("{:?}", plan());
        assert!(!shown.contains("secret"), "{shown}");
    }
}
