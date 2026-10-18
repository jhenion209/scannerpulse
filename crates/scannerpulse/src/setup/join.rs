//! Joining an install as a database machine: setup on the new machine
//! reaches the install's first ScannerPulse machine (the hub) over SSH,
//! with a login the person gives there, reads its place in the plan, sets
//! itself up, and says it has joined. SSH is the only channel between the
//! machines, and nothing on the new machine keeps that login.

use std::path::Path;
use std::process::{Command, Stdio};

use super::plan::{PLAN, Plan, SSH, resolve};

/// What the hub tells a joining machine: its place, and the plan.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Joining {
    pub index: usize,
    pub plan: Plan,
}

/// A host name or an address, nothing a shell would read more into.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

/// `scannerpulse join info NAME...`, as root on the hub: the place of the
/// machine with these names and addresses, and the plan (its passwords
/// travel over SSH, to root on the joining machine).
pub fn info(names: &[String]) -> Result<String, String> {
    let plan = Plan::read(Path::new(PLAN))?;
    let index = plan.place(names, resolve).ok_or_else(|| {
        format!(
            "this machine ({}) is not one of the install's database machines: they are {} and {}",
            names.join(", "),
            plan.nodes[0],
            plan.nodes[1]
        )
    })?;
    serde_json::to_string(&Joining { index, plan }).map_err(|error| error.to_string())
}

/// The known_hosts lines for `node` from its public host keys (one
/// `type key [comment]` a line).
pub fn known_lines(node: &str, host_keys: &str) -> Vec<String> {
    host_keys
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let kind = words.next()?;
            let key = words.next()?;
            (kind.starts_with("ssh-") || kind.starts_with("ecdsa-"))
                .then(|| format!("{node} {kind} {key}"))
        })
        .collect()
}

/// `scannerpulse join done NODE`, as root on the hub: NODE has joined, and
/// its host keys (standard input) are the ones the hub's commands trust.
pub fn done(node: &str, host_keys: &str) -> Result<String, String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut plan = Plan::read(Path::new(PLAN))?;
    if !plan.nodes.iter().any(|n| n == node) {
        return Err(format!(
            "{node} is not one of the install's database machines"
        ));
    }
    let lines = known_lines(node, host_keys);
    if lines.is_empty() {
        return Err("no SSH host key came with it".into());
    }
    let path = Path::new(SSH).join("known_hosts");
    let kept = std::fs::read_to_string(&path).unwrap_or_default();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    for line in lines
        .iter()
        .filter(|l| !kept.lines().any(|k| k == l.as_str()))
    {
        writeln!(file, "{line}").map_err(|error| format!("{}: {error}", path.display()))?;
    }
    plan.mark_joined(node);
    plan.write(Path::new(PLAN))?;
    Ok(format!("{node} has joined"))
}

/// How setup on a database machine reaches the hub.
pub struct Hub {
    pub address: String,
    pub login: String,
    /// The login's sudo password, when it is not root.
    pub sudo_password: Option<String>,
    /// No password prompt for SSH itself (an answers file).
    pub batch: bool,
}

impl Hub {
    /// The ssh command line that runs `scannerpulse join ARGS` on the hub
    /// as root, over one connection kept for the session.
    pub fn command(&self, args: &[&str]) -> Vec<String> {
        let mut remote = String::new();
        if self.login != "root" {
            remote.push_str("sudo -S -p '' ");
        }
        remote.push_str(super::BINARY);
        remote.push_str(" join");
        for arg in args {
            remote.push(' ');
            remote.push_str(arg);
        }
        let mut command: Vec<String> = [
            "ssh",
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=/run/scannerpulse-join-%C",
            "-o",
            "ControlPersist=300",
            "-o",
            "StrictHostKeyChecking=accept-new",
        ]
        .map(String::from)
        .to_vec();
        if self.batch {
            command.extend(["-o".into(), "BatchMode=yes".into()]);
        }
        command.push(format!("{}@{}", self.login, self.address));
        command.push("--".into());
        command.push(remote);
        command
    }

    fn run(&self, args: &[&str], input: &str) -> Result<String, String> {
        if let Some(bad) = args.iter().find(|a| !plain(a)) {
            return Err(format!("{bad:?} is not a name or an address"));
        }
        let command = self.command(args);
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot run ssh: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let mut text = String::new();
            if let Some(password) = &self.sudo_password {
                text.push_str(password);
                text.push('\n');
            }
            text.push_str(input);
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| format!("cannot talk to ssh: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("ssh: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(format!(
                "on {}: {}",
                self.address,
                super::apply::said(&output)
            ))
        }
    }

    pub fn info(&self, names: &[String]) -> Result<Joining, String> {
        let args: Vec<&str> = std::iter::once("info")
            .chain(names.iter().map(String::as_str))
            .collect();
        let text = self.run(&args, "")?;
        serde_json::from_str(&text).map_err(|error| format!("the hub's answer: {error}"))
    }

    pub fn done(&self, node: &str, host_keys: &str) -> Result<(), String> {
        self.run(&["done", node], host_keys).map(drop)
    }
}

/// Sets this machine up as the database machine the hub's plan names it,
/// once the first one has joined (for the second), and says so to the hub.
/// `yes` asks the person to go ahead (an answers file says yes).
pub async fn run(hub: Hub, dry_run: bool, yes: impl Fn(&str) -> bool) -> Result<(), String> {
    let names = my_names(&hub.address);
    let mut joining = hub.info(&names)?;
    if joining.index == 1 {
        let first = joining.plan.nodes[0].clone();
        let mut said = false;
        while !joining.plan.joined.contains(&first) {
            if !said {
                println!("Waiting for {first}, the first database machine, to join first.");
                said = true;
            }
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            joining = hub.info(&names)?;
        }
    }
    let Joining { index, plan } = joining;
    let hub_ips = resolve(&plan.hub);
    if hub_ips.is_empty() {
        return Err(format!("{} does not resolve to an address here", plan.hub));
    }
    let mut node_ips = Vec::new();
    for node in &plan.nodes {
        let found = resolve(node);
        if found.is_empty() {
            return Err(format!("{node} does not resolve to an address here"));
        }
        node_ips.extend(found);
    }
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    let system = super::system::detect(&os_release, std::env::consts::ARCH)?;
    let steps = super::node::steps(
        &plan,
        index,
        &system,
        Path::new(super::BINARY),
        &hub_ips,
        &node_ips,
    );
    let role = if index == 0 {
        "it starts as the primary"
    } else {
        "the standby of the first, promoted by hand"
    };
    println!(
        "\nThis machine is database machine {} of the install at {}: {} ({role}).",
        index + 1,
        plan.hub,
        plan.nodes[index]
    );
    println!("Setup will:");
    for (n, step) in steps.iter().enumerate() {
        println!("  {:>2}. {}", n + 1, step.title);
    }
    if dry_run {
        println!("\nA dry run: nothing was changed.");
        return Ok(());
    }
    if !yes("Set this machine up?") {
        println!("Setup left; nothing was changed.");
        return Ok(());
    }
    let context = super::apply::Context {
        database_url: String::new(),
        db_password: String::new(),
    };
    super::apply::apply(&steps, &context, true).await?;
    hub.done(&plan.nodes[index], &host_keys())?;
    println!(
        "\nJoined: {} is database machine {} of the install at {}.",
        plan.nodes[index],
        index + 1,
        plan.hub
    );
    Ok(())
}

/// This machine's names, as the plan may give them: its host name, and the
/// address it reaches `hub` from.
pub fn my_names(hub: &str) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(name) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let name = name.trim().to_ascii_lowercase();
        if !name.is_empty() {
            names.push(name);
        }
    }
    let target = resolve(hub).into_iter().next();
    if let Some(ip) = target {
        let any = if ip.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" };
        if let Ok(socket) = std::net::UdpSocket::bind(any)
            && socket.connect((ip, 22)).is_ok()
            && let Ok(local) = socket.local_addr()
        {
            names.push(local.ip().to_string());
        }
    }
    names
}

/// This machine's public SSH host keys.
pub fn host_keys() -> String {
    let mut keys = String::new();
    if let Ok(entries) = std::fs::read_dir("/etc/ssh") {
        let mut paths: Vec<_> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("ssh_host_") && n.ends_with("_key.pub"))
            })
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(&path) {
                keys.push_str(text.trim());
                keys.push('\n');
            }
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hub_is_asked_as_root_over_one_kept_connection() {
        let hub = Hub {
            address: "192.0.2.10".into(),
            login: "chief".into(),
            sudo_password: Some("sudo-secret".into()),
            batch: false,
        };
        let command = hub.command(&["info", "db2", "192.0.2.22"]);
        assert_eq!(command[0], "ssh");
        assert!(command.contains(&"ControlMaster=auto".to_string()));
        assert!(!command.contains(&"BatchMode=yes".to_string()));
        assert_eq!(command[command.len() - 3], "chief@192.0.2.10");
        assert_eq!(
            command.last().unwrap(),
            "sudo -S -p '' /usr/local/bin/scannerpulse join info db2 192.0.2.22"
        );
        assert!(
            !command.join(" ").contains("sudo-secret"),
            "the password goes on standard input"
        );
        let root = Hub {
            login: "root".into(),
            sudo_password: None,
            batch: true,
            ..hub
        };
        let command = root.command(&["done", "db2"]);
        assert!(command.contains(&"BatchMode=yes".to_string()));
        assert_eq!(
            command.last().unwrap(),
            "/usr/local/bin/scannerpulse join done db2"
        );
    }

    #[test]
    fn a_name_with_shell_in_it_never_reaches_the_hub() {
        let hub = Hub {
            address: "192.0.2.10".into(),
            login: "root".into(),
            sudo_password: None,
            batch: true,
        };
        let error = hub.run(&["info", "db2;id"], "").unwrap_err();
        assert!(error.contains("not a name or an address"), "{error}");
    }

    #[test]
    fn a_nodes_host_keys_become_known_hosts_lines_for_its_name() {
        assert_eq!(
            known_lines(
                "db2.example.org",
                "ssh-ed25519 AAAAkey1 root@db2\necdsa-sha2-nistp256 AAAAkey2\n\nnot a key\n"
            ),
            [
                "db2.example.org ssh-ed25519 AAAAkey1",
                "db2.example.org ecdsa-sha2-nistp256 AAAAkey2"
            ]
        );
    }
}
