//! Joining an install as a database machine: setup on the new machine
//! reaches the install's first ScannerPulse machine (the hub) over SSH,
//! with a login the person gives there, reads its place in the plan, sets
//! itself up, and says it has joined. SSH is the only channel between the
//! machines, and nothing on the new machine keeps that login.

use std::path::Path;
use std::process::{Command, Stdio};

use super::plan::{PLAN, Place, Plan, SSH, resolve};

/// What the hub tells a joining machine: its place, and the plan.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Joining {
    pub place: Place,
    pub plan: Plan,
    /// For the second ScannerPulse machine, the first machine's files.
    #[serde(default)]
    pub files: Option<super::second::HubFiles>,
}

/// A host name or an address, nothing a shell would read more into.
pub(super) fn plain(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
}

/// `scannerpulse join info NAME...`, as root on the hub: the place of the
/// machine with these names and addresses, and the plan (its passwords
/// travel over SSH, to root on the joining machine).
pub fn info(names: &[String]) -> Result<String, String> {
    info_from(Path::new(PLAN), names, resolve)
}

/// `info`, from the plan at `path`, the names resolved by `resolve`.
pub fn info_from(
    path: &Path,
    names: &[String],
    resolve: impl Fn(&str) -> Vec<std::net::IpAddr>,
) -> Result<String, String> {
    let plan = Plan::read(path)?;
    let place = plan.place(names, resolve).ok_or_else(|| {
        format!(
            "this machine ({}) is not one of the install's machines: they are {}",
            names.join(", "),
            plan.referee
                .iter()
                .chain(&plan.nodes)
                .chain(&plan.second)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let files = match place {
        Place::Second => Some(super::second::HubFiles::read()?),
        _ => None,
    };
    serde_json::to_string(&Joining { place, plan, files }).map_err(|error| error.to_string())
}

/// The hub's known_hosts, `kept`, with `node`'s lines replaced by `lines`.
pub fn known_hosts_with(kept: &str, node: &str, lines: &[String]) -> String {
    let mut text: String = kept
        .lines()
        .filter(|line| line.split_whitespace().next() != Some(node))
        .map(|line| format!("{line}\n"))
        .collect();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    text
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
    let plan = Plan::read(Path::new(PLAN))?;
    if !plan
        .nodes
        .iter()
        .chain(&plan.referee)
        .chain(&plan.second)
        .any(|n| n == node)
    {
        return Err(format!("{node} is not one of the install's machines"));
    }
    let lines = known_lines(node, host_keys);
    if lines.is_empty() {
        return Err("no SSH host key came with it".into());
    }
    // Under the plan's lock, so that two machines joining at once each keep
    // their host keys and their mark. A machine set up again has new host
    // keys: its old ones go.
    Plan::replace(Path::new(PLAN), |kept_plan| {
        let mut plan = kept_plan.ok_or_else(|| Plan::read(Path::new(PLAN)).unwrap_err())?;
        let path = Path::new(SSH).join("known_hosts");
        let kept = std::fs::read_to_string(&path).unwrap_or_default();
        let new = path.with_extension("new");
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&new)
            .and_then(|mut file| file.write_all(known_hosts_with(&kept, node, &lines).as_bytes()))
            .and_then(|()| std::fs::rename(&new, &path))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        plan.mark_joined(node);
        Ok(plan)
    })?;
    Ok(format!("{node} has joined"))
}

/// Where the connection to the hub is kept for the join's commands.
const CONTROL_PATH: &str = "/run/scannerpulse-join-%C";

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
    /// Closes the connection `command` keeps open.
    pub fn close_command(&self) -> Vec<String> {
        vec![
            "ssh".into(),
            "-o".into(),
            format!("ControlPath={CONTROL_PATH}"),
            "-O".into(),
            "exit".into(),
            format!("{}@{}", self.login, self.address),
        ]
    }

    pub fn close(&self) {
        let command = self.close_command();
        let _ = Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

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
        let control = format!("ControlPath={CONTROL_PATH}");
        let mut command: Vec<String> = [
            "ssh",
            "-o",
            "ControlMaster=auto",
            "-o",
            &control,
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
            Err(ssh_failed(&self.address, &output))
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

/// What a failed command to the first machine says: ssh's and the
/// command's errors only. Its answer, which holds the plan's passwords and
/// the key, may have arrived before it failed.
fn ssh_failed(address: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let said = lines[lines.len().saturating_sub(10)..].join("\n");
    if said.is_empty() {
        format!("on {address}: ssh ended with {}", output.status)
    } else {
        format!("on {address}: {said}")
    }
}

/// Sets this machine up as the machine the hub's plan names it (the
/// referee, or a database machine), once the machines before it have
/// joined, and says so to the hub. `yes` asks the person to go ahead (an
/// answers file says yes).
pub async fn run(hub: Hub, dry_run: bool, yes: impl Fn(&str) -> bool) -> Result<(), String> {
    // The kept connection to the hub, the person's login, closes however
    // the join ends: done, failed, or stopped by Ctrl-C, a hang-up or a stop.
    let stop = stopped();
    let joined = tokio::select! {
        biased;
        () = stop => Err("the join was stopped".into()),
        joined = join(&hub, dry_run, yes) => joined,
    };
    hub.close();
    joined
}

/// Completes at Ctrl-C, a hang-up (the terminal gone) or SIGTERM. Its
/// handlers exist from this call on, so a stop during the join's first steps
/// is caught too; it takes effect between steps.
fn stopped() -> impl std::future::Future<Output = ()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut interrupt = signal(SignalKind::interrupt()).ok();
    let mut hangup = signal(SignalKind::hangup()).ok();
    let mut term = signal(SignalKind::terminate()).ok();
    async move {
        tokio::select! {
            () = received(&mut interrupt) => {}
            () = received(&mut hangup) => {}
            () = received(&mut term) => {}
        }
    }
}

/// Completes when `stream` receives its signal; never without one.
async fn received(stream: &mut Option<tokio::signal::unix::Signal>) {
    match stream {
        Some(stream) => {
            stream.recv().await;
        }
        None => std::future::pending().await,
    }
}

async fn join(hub: &Hub, dry_run: bool, yes: impl Fn(&str) -> bool) -> Result<(), String> {
    let names = my_names(&hub.address);
    let mut joining = hub.info(&names)?;
    let mut said = None;
    while let Some(first) = joining.plan.awaited_before(joining.place) {
        if said.as_deref() != Some(first) {
            println!("Waiting for {first} to join first.");
            said = Some(first.to_string());
        }
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        joining = hub.info(&names)?;
    }
    let Joining { place, plan, files } = joining;
    // The ScannerPulse machines: the database's access rules let both in.
    let mut hub_ips = resolve(&plan.hub);
    if hub_ips.is_empty() {
        return Err(format!("{} does not resolve to an address here", plan.hub));
    }
    if let Some(second) = &plan.second {
        let found = resolve(second);
        if found.is_empty() {
            return Err(format!("{second} does not resolve to an address here"));
        }
        hub_ips.extend(found);
    }
    let mut node_ips = Vec::new();
    for node in plan.nodes.iter().chain(&plan.referee) {
        let found = resolve(node);
        if found.is_empty() {
            return Err(format!("{node} does not resolve to an address here"));
        }
        if plan.nodes.contains(node) {
            node_ips.extend(found);
        }
    }
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    let system = super::system::detect(&os_release, std::env::consts::ARCH)?;
    // Another PostgreSQL here would take port 5432 from the one setup makes.
    if place != Place::Second {
        super::system::other_postgres_joining(&system.seen())?;
    }
    let binary = Path::new(super::BINARY);
    let (steps, what) = match place {
        Place::Second => (
            super::second::steps(
                &plan,
                files.as_ref().ok_or("the first machine sent no settings")?,
                &system,
                binary,
                plan.name(place),
                Path::new(super::steps::WEB_UNIT)
                    .exists()
                    .then(|| std::fs::read_to_string(super::steps::env_path()).ok())
                    .flatten()
                    .as_deref(),
            )?,
            "the second ScannerPulse machine".to_string(),
        ),
        Place::Referee => (
            super::referee::steps(&plan, &system, binary, &hub_ips, &node_ips),
            "the referee, which decides which database machine is the primary".to_string(),
        ),
        Place::Node(index) => (
            super::node::steps(&plan, index, &system, binary, &hub_ips, &node_ips),
            format!(
                "database machine {}: {}",
                index + 1,
                match (index, plan.referee.is_some()) {
                    (0, _) => "it starts as the primary",
                    (_, true) => "the standby of the first, promoted by the referee",
                    (_, false) => "the standby of the first, promoted by hand",
                }
            ),
        ),
    };
    let name = plan.name(place).to_string();
    println!(
        "\nThis machine is {name}, {what}, of the install at {}.",
        plan.hub
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
        nodes: Vec::new(),
    };
    super::apply::apply(&steps, &context, true).await?;
    hub.done(&name, &host_keys())?;
    println!(
        "\nJoined: {name} is {what}, of the install at {}.",
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
    fn the_kept_connection_is_closed_at_the_end() {
        let hub = Hub {
            address: "192.0.2.10".into(),
            login: "chief".into(),
            sudo_password: None,
            batch: true,
        };
        let close = hub.close_command();
        assert_eq!(
            close,
            [
                "ssh",
                "-o",
                "ControlPath=/run/scannerpulse-join-%C",
                "-O",
                "exit",
                "chief@192.0.2.10"
            ]
        );
        assert!(
            hub.command(&["info"])
                .contains(&"ControlPath=/run/scannerpulse-join-%C".to_string())
        );
    }

    #[test]
    fn a_machine_not_in_the_plan_is_told_the_plans_machines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plan.json");
        let plan = Plan {
            hub: "192.0.2.10".into(),
            hub_key: "ssh-ed25519 AAAA hub".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            database: "scannerpulse".into(),
            role: "scannerpulse".into(),
            password: "app-secret-1".into(),
            replication_password: "copy-secret-2".into(),
            referee: Some("ref.example.org".into()),
            monitor_password: "referee-secret-3".into(),
            second: Some("web2.example.org".into()),
            shared: None,
            install_id: String::new(),
            archive: String::new(),
            recordings: String::new(),
            ready: false,
            joined: Vec::new(),
        };
        plan.write(&path).unwrap();
        let none = |_: &str| Vec::new();
        let error = info_from(&path, &["web.example.org".into()], none).unwrap_err();
        assert!(
            error.contains("web.example.org")
                && error.contains(
                    "ref.example.org, db1.example.org, db2.example.org, web2.example.org"
                ),
            "{error}"
        );
        let told: Joining =
            serde_json::from_str(&info_from(&path, &["db2.example.org".into()], none).unwrap())
                .unwrap();
        assert_eq!(told.place, Place::Node(1));
    }

    #[test]
    fn a_nodes_new_host_keys_replace_its_old_ones() {
        let kept = "db1.example.org ssh-ed25519 AAAAold\ndb2.example.org ssh-ed25519 AAAAtwo\n";
        let lines = vec!["db1.example.org ssh-ed25519 AAAAnew".to_string()];
        assert_eq!(
            known_hosts_with(kept, "db1.example.org", &lines),
            "db2.example.org ssh-ed25519 AAAAtwo\ndb1.example.org ssh-ed25519 AAAAnew\n"
        );
    }

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
        // Long enough to outlast the steps between the hub's questions; a
        // join cut short closes it (`run`).
        assert!(command.contains(&"ControlPersist=300".to_string()));
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

    #[test]
    fn a_failed_answer_from_the_first_machine_is_never_shown() {
        use std::os::unix::process::ExitStatusExt;
        // ssh can fail (a dropped connection gives 255) after the answer,
        // which holds the plan's passwords and the key, has arrived.
        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(255 << 8),
            stdout: br#"{"plan":{"password":"app-secret-1"},"files":{"key":"key-secret"}}"#
                .to_vec(),
            stderr: b"Connection to 192.0.2.10 closed by remote host.\n".to_vec(),
        };
        let said = super::ssh_failed("192.0.2.10", &output);
        assert!(said.contains("closed by remote host"), "{said}");
        assert!(!said.contains("secret"), "{said}");
    }

    #[tokio::test]
    async fn a_stop_is_caught_from_the_moment_the_join_starts() {
        // The handlers exist once `stopped` is called, before the join's
        // first wait: a hang-up then ends the join, never the process.
        let stop = super::stopped();
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::HUP)
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), stop)
            .await
            .expect("the hang-up ends the wait");
    }
}
