//! A database machine: PostgreSQL 17 on a machine of its own. The first
//! starts as the primary and the second follows it as its standby; the
//! ScannerPulse machine (the hub) reaches both, and its key may run this
//! machine's `node` commands, and nothing else.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::Plan;
use super::steps::{Action, Step, run};
use super::system::{Family, System};

/// This machine's place among the database machines; root's alone.
pub const NODE: &str = "/etc/scannerpulse/node.json";

/// The nightly dump's folder on a database machine.
pub const DUMPS: &str = "/var/backups/scannerpulse";

/// The role the standby copies the primary with.
pub const REPLICATOR: &str = "replicator";

/// The marker of setup's lines in PostgreSQL's access rules.
const MARK: &str = "# scannerpulse: the ScannerPulse machine, and both database machines";

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NodeFile {
    /// 0 for the machine that started as the primary, 1 for its standby.
    pub index: usize,
    pub nodes: Vec<String>,
    pub hub: String,
    pub replication_password: String,
}

/// Never the password.
impl std::fmt::Debug for NodeFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeFile")
            .field("index", &self.index)
            .field("nodes", &self.nodes)
            .field("hub", &self.hub)
            .finish_non_exhaustive()
    }
}

impl NodeFile {
    pub fn read() -> Result<NodeFile, String> {
        let text = std::fs::read_to_string(NODE)
            .map_err(|error| format!("{NODE}: {error} (is this a database machine?)"))?;
        serde_json::from_str(&text).map_err(|error| format!("{NODE}: {error}"))
    }

    /// The other database machine.
    pub fn other(&self) -> &str {
        &self.nodes[1 - self.index]
    }
}

fn cidr(ip: &IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => format!("{v4}/32"),
        IpAddr::V6(v6) => format!("{v6}/128"),
    }
}

/// The lines setup adds to PostgreSQL's access rules: the hub may use the
/// database as its role; either database machine may copy from this one
/// and ask it (the `postgres` database: status, rewind). TLS and a password
/// for each.
pub fn hba(plan: &Plan, hub: &[IpAddr], nodes: &[IpAddr]) -> String {
    let mut lines = vec![MARK.to_string()];
    for ip in hub {
        lines.push(format!(
            "hostssl {} {} {} scram-sha-256",
            plan.database,
            plan.role,
            cidr(ip)
        ));
    }
    for ip in nodes {
        for database in ["replication", "postgres"] {
            lines.push(format!(
                "hostssl {database} {REPLICATOR} {} scram-sha-256",
                cidr(ip)
            ));
        }
    }
    lines.join("\n") + "\n"
}

/// Root's authorized_keys line for the hub's key: from the hub's addresses
/// only, no shell, no forwarding, and only `scannerpulse node serve`, which
/// reads the command asked for and runs it if it is one of its own.
pub fn authorized_line(hub_key: &str, hub: &[IpAddr], binary: &Path) -> String {
    let from: Vec<String> = hub.iter().map(IpAddr::to_string).collect();
    format!(
        "restrict,from=\"{}\",command=\"{} node serve\" {}\n",
        from.join(","),
        binary.display(),
        hub_key.trim()
    )
}

/// The SQL that makes the copying role, which the standby (and a rewind)
/// signs in as.
fn replicator_sql(password: &str) -> String {
    let quoted = password.replace('\'', "''");
    format!(
        "SET log_min_error_statement = panic;\n\
         SELECT format('CREATE ROLE {REPLICATOR} REPLICATION LOGIN PASSWORD %L', '{quoted}') \
         WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{REPLICATOR}') \\gexec\n\
         ALTER ROLE {REPLICATOR} PASSWORD '{quoted}';\n\
         GRANT EXECUTE ON FUNCTION pg_ls_dir(text, boolean, boolean) TO {REPLICATOR};\n\
         GRANT EXECUTE ON FUNCTION pg_stat_file(text, boolean) TO {REPLICATOR};\n\
         GRANT EXECUTE ON FUNCTION pg_read_binary_file(text) TO {REPLICATOR};\n\
         GRANT EXECUTE ON FUNCTION pg_read_binary_file(text, bigint, bigint, boolean) TO {REPLICATOR};\n"
    )
}

/// A psql step that has nothing to change while this machine has no
/// cluster (a standby's copy cut short leaves none; the copy makes one).
fn psql_if_cluster(title: &str, data: &str, sql: String) -> Step {
    script(
        title,
        "[ -f \"$DATA/PG_VERSION\" ] || exit 0\nrunuser -u postgres -- psql -q -v ON_ERROR_STOP=1",
        &[("DATA", data)],
        Some(sql),
    )
}

/// A shell script run as a step, its values as named variables and
/// `input` on its standard input.
fn script(title: &str, body: &str, vars: &[(&str, &str)], input: Option<String>) -> Step {
    Step {
        title: title.into(),
        action: Action::Run {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), body.into()],
            input,
            env: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            unless: None,
        },
    }
}

/// The steps that make this machine database machine `index` of `plan`.
/// `hub` and `nodes` are the addresses the hub's and both machines' names
/// give.
pub fn steps(
    plan: &Plan,
    index: usize,
    system: &System,
    binary: &Path,
    hub: &[IpAddr],
    nodes: &[IpAddr],
) -> Vec<Step> {
    let mut steps = Vec::new();
    let data = system.postgres_data();
    let service = system.postgres_service();
    match system.family {
        Family::Apt => {
            let wait = "DPkg::Lock::Timeout=300";
            steps.push(run(
                "Read the package lists",
                "apt-get",
                &["-o", wait, "update"],
            ));
            steps.push(run(
                "Install the PostgreSQL project's repository tool",
                "apt-get",
                &[
                    "-o",
                    wait,
                    "install",
                    "-y",
                    "postgresql-common",
                    "ca-certificates",
                ],
            ));
            steps.push(run(
                "Add the PostgreSQL project's repository",
                "/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh",
                &["-y"],
            ));
            steps.push(run(
                "Install PostgreSQL 17 and SSH",
                "apt-get",
                &[
                    "-o",
                    wait,
                    "install",
                    "-y",
                    "postgresql-17",
                    "openssh-server",
                ],
            ));
        }
        Family::Dnf => {
            let rpm = system
                .pgdg_rpm()
                .expect("the Red Hat family has the repository's package");
            steps.push(run(
                "Add the PostgreSQL project's repository",
                "dnf",
                &["install", "-y", &rpm],
            ));
            if system.id != "fedora" && matches!(system.version.split('.').next(), Some("8" | "9"))
            {
                steps.push(run(
                    "Turn off the system's own PostgreSQL module",
                    "dnf",
                    &["-qy", "module", "disable", "postgresql"],
                ));
            }
            steps.push(run(
                "Install PostgreSQL 17 and SSH",
                "dnf",
                &["install", "-y", "postgresql17-server", "openssh-server"],
            ));
            steps.push(Step {
                title: "Make PostgreSQL 17's data directory".into(),
                action: Action::Run {
                    program: "/usr/pgsql-17/bin/postgresql-17-setup".into(),
                    args: vec!["initdb".into()],
                    input: None,
                    env: vec![(
                        "PGSETUP_INITDB_OPTIONS".into(),
                        "--auth-host=scram-sha-256 --auth-local=peer".into(),
                    )],
                    unless: Some(Path::new(data).join("PG_VERSION")),
                },
            });
        }
    }
    steps.push(run(
        "Start PostgreSQL 17, now and at boot",
        "systemctl",
        &["enable", "--now", service],
    ));
    steps.push(super::steps::firewall(true, 5432));
    if system.family == Family::Dnf {
        // Debian's packages give PostgreSQL a certificate; these do not.
        steps.push(script(
            "Give PostgreSQL a certificate, for TLS",
            "[ -e \"$DATA/server.key\" ] || { \"$BINARY\" cert make --cert \"$DATA/server.crt\" --key \"$DATA/server.key\" \"$NAME\" && chown postgres:postgres \"$DATA/server.crt\" \"$DATA/server.key\"; }",
            &[
                ("DATA", data),
                ("BINARY", &binary.display().to_string()),
                ("NAME", &plan.nodes[index]),
            ],
            None,
        ));
        steps.push(psql_if_cluster(
            "Turn on TLS",
            data,
            "ALTER SYSTEM SET ssl = on;\n".into(),
        ));
    }
    steps.push(psql_if_cluster(
        "Let PostgreSQL take connections from the network, and keep what a rewind needs",
        data,
        "ALTER SYSTEM SET listen_addresses = '*';\nALTER SYSTEM SET wal_log_hints = on;\n".into(),
    ));
    steps.push(script(
        "Let the ScannerPulse machine and both database machines in",
        "grep -qxF \"$MARK\" \"$HBA\" || cat >> \"$HBA\"",
        &[("MARK", MARK), ("HBA", system.postgres_hba())],
        Some(hba(plan, hub, nodes)),
    ));
    steps.push(script(
        "Restart PostgreSQL 17",
        "[ -f \"$DATA/PG_VERSION\" ] || exit 0\nsystemctl restart \"$SERVICE\"",
        &[("DATA", data), ("SERVICE", system.postgres_cluster())],
        None,
    ));
    let pgpass = Path::new(system.postgres_home()).join(".pgpass");
    steps.push(Step {
        title: "Write the copying role's password file, for the postgres account".into(),
        action: Action::Write {
            path: pgpass,
            contents: format!("*:*:*:{REPLICATOR}:{}\n", plan.replication_password),
            mode: 0o600,
            owner: "postgres:postgres".into(),
        },
    });
    // The copy's values: it copies the other machine.
    let other = &plan.nodes[1 - index];
    let copy_vars = [
        ("DATA", data),
        ("BIN", system.postgres_bin()),
        ("SERVICE", system.postgres_cluster()),
        ("PRIMARY", other.as_str()),
        ("NODE", NODE),
        ("DATABASE", super::steps::DATABASE),
    ];
    if index == 0 {
        // A standby since a change of roles whose copy was cut short has no
        // cluster: it copies the other machine, the primary, again.
        steps.push(script(
            "Copy the other machine's data, if this machine has none",
            &format!("[ -f \"$DATA/PG_VERSION\" ] && exit 0\n{COPY}"),
            &copy_vars,
            None,
        ));
        // A standby since a change of roles has them from the primary.
        steps.push(script(
            "Make the database, its role, and the copying role",
            "if runuser -u postgres -- psql -tAXc 'SELECT pg_is_in_recovery()' | grep -qx t; then\n  echo \"a standby: its roles come from the primary\"; exit 0\nfi\nrunuser -u postgres -- psql -q -v ON_ERROR_STOP=1",
            &[],
            Some(
                super::steps::database_sql(&plan.password)
                    + &replicator_sql(&plan.replication_password),
            ),
        ));
    } else {
        steps.push(script(
            "Copy the primary's data, and follow it",
            COPY,
            &copy_vars,
            None,
        ));
    }
    steps.push(script(
        "Let the ScannerPulse machine run this machine's database commands",
        "install -d -m 0700 /root/.ssh && touch /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys && { grep -qF \"$KEY\" /root/.ssh/authorized_keys || cat >> /root/.ssh/authorized_keys; }",
        &[("KEY", plan.hub_key.trim())],
        Some(authorized_line(&plan.hub_key, hub, binary)),
    ));
    // Open to all, as the nightly dump (postgres) looks in it for a settings
    // file; this machine's file in it is root's alone.
    steps.push(run(
        "Make the settings folder",
        "install",
        &[
            "-d",
            "-m",
            "0755",
            "-o",
            "root",
            "-g",
            "root",
            super::steps::ETC,
        ],
    ));
    let file = NodeFile {
        index,
        nodes: plan.nodes.clone(),
        hub: plan.hub.clone(),
        replication_password: plan.replication_password.clone(),
    };
    steps.push(Step {
        title: "Write this machine's place among the database machines".into(),
        action: Action::Write {
            path: PathBuf::from(NODE),
            contents: serde_json::to_string_pretty(&file).expect("plain values") + "\n",
            mode: 0o600,
            owner: "root:root".into(),
        },
    });
    steps.push(run(
        "Make the folder of the nightly dumps",
        "install",
        &[
            "-d", "-m", "0700", "-o", "postgres", "-g", "postgres", DUMPS,
        ],
    ));
    let (service_unit, timer) = dump_units(binary, system);
    for (name, contents) in [
        ("scannerpulse-node-backup.service", service_unit),
        ("scannerpulse-node-backup.timer", timer),
    ] {
        steps.push(Step {
            title: format!("Write {name}"),
            action: Action::Write {
                path: Path::new("/etc/systemd/system").join(name),
                contents,
                mode: 0o644,
                owner: "root:root".into(),
            },
        });
    }
    steps.push(run("Read the new units", "systemctl", &["daemon-reload"]));
    steps.push(run(
        "Start the nightly dump, now and at boot",
        "systemctl",
        &["enable", "--now", "scannerpulse-node-backup.timer"],
    ));
    steps
}

/// The standby's copy: refused over a cluster that holds databases of its
/// own; a machine already following its primary, or already the primary of
/// this install (after a change of roles), is left as it is; a copy cut
/// short (no cluster) is made again.
const COPY: &str = r#"set -e
if [ -f "$DATA/PG_VERSION" ]; then
  if runuser -u postgres -- psql -tAXc 'SELECT pg_is_in_recovery()' | grep -qx t; then
    echo "already a standby"; exit 0
  fi
  n=$(runuser -u postgres -- psql -tAXc "SELECT count(*) FROM pg_database WHERE datname NOT IN ('postgres', 'template0', 'template1')")
  if [ "$n" != 0 ]; then
    if [ -f "$NODE" ] && runuser -u postgres -- psql -tAXc "SELECT 1 FROM pg_database WHERE datname = '$DATABASE'" | grep -qx 1; then
      echo "already the primary"; exit 0
    fi
    echo "this machine's PostgreSQL holds databases of its own: setup will not replace them" >&2; exit 1
  fi
fi
systemctl stop "$SERVICE"
find "$DATA" -mindepth 1 -delete
runuser -u postgres -- "$BIN/pg_basebackup" -d "host=$PRIMARY port=5432 user=replicator sslmode=require" -D "$DATA" -R -X stream -c fast
systemctl start "$SERVICE"
"#;

/// The nightly dump on a database machine: it runs on both, and dumps on
/// the one that is the primary.
pub fn dump_units(binary: &Path, system: &System) -> (String, String) {
    (
        format!(
            "# Written by `scannerpulse setup`.\n\
             [Unit]\n\
             Description=ScannerPulse nightly dump, on the primary\n\
             After={pg}.service\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             User=postgres\n\
             ExecStart={binary} node backup\n",
            pg = system.postgres_service(),
            binary = binary.display(),
        ),
        "# Written by `scannerpulse setup`.\n\
         [Unit]\n\
         Description=ScannerPulse nightly dump\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* 03:17:00\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
            .to_string(),
    )
}

/// Runs `sql` in this machine's PostgreSQL as the postgres account (through
/// runuser when root), and gives its one-line answer.
fn psql(sql: &str) -> Result<String, String> {
    let root = rustix::process::geteuid().is_root();
    let mut command = if root {
        let mut c = Command::new("runuser");
        c.args(["-u", "postgres", "--", "psql"]);
        c
    } else {
        Command::new("psql")
    };
    command.args(["-tAX", "-v", "ON_ERROR_STOP=1", "-c", sql]);
    let output = command
        .output()
        .map_err(|error| format!("cannot run psql: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(super::apply::said(&output))
    }
}

/// Asks the other database machine, over the network as the copying role,
/// whether it is a standby: `Some(true)` a standby, `Some(false)` the
/// primary, `None` no answer.
fn other_is_standby(other: &str) -> Option<bool> {
    match ask_other(other, "SELECT pg_is_in_recovery()")?.as_str() {
        "t" => Some(true),
        "f" => Some(false),
        _ => None,
    }
}

/// The other database machine's timeline, if it answers as a primary.
fn other_timeline(other: &str) -> Option<u32> {
    crate::primary::timeline_of(&ask_other(other, crate::primary::CURRENT_WAL)?)
}

/// Asks the other database machine `sql` over the network as the copying
/// role, and gives its one-line answer; `None` when it does not answer.
fn ask_other(other: &str, sql: &str) -> Option<String> {
    let root = rustix::process::geteuid().is_root();
    let conninfo = format!(
        "host={other} port=5432 user={REPLICATOR} dbname=postgres sslmode=require connect_timeout=3"
    );
    let mut command = if root {
        let mut c = Command::new("runuser");
        c.args(["-u", "postgres", "--", "psql"]);
        c
    } else {
        Command::new("psql")
    };
    let output = command
        .args([conninfo.as_str(), "-tAXc", sql])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// This machine's timeline: from PostgreSQL if it is a primary that
/// answers, else from its control file (a stopped machine).
fn my_timeline(system: &System) -> Option<u32> {
    match psql(crate::primary::CURRENT_WAL) {
        Ok(wal) => crate::primary::timeline_of(&wal),
        Err(_) => control_timeline(&controldata(system).ok()?),
    }
}

/// Whether this machine may be rejoined to `other`: only when `other` is a
/// primary on a newer timeline than this one's (`mine`, `None` for a
/// standby). Rejoining the newest machine would throw away what was written
/// to it since the failover.
fn may_rejoin(mine: Option<u32>, theirs: Option<u32>, other: &str) -> Result<(), String> {
    let Some(theirs) = theirs else {
        return Err(format!(
            "{other} must be the primary first: `sudo scannerpulse db promote` there"
        ));
    };
    match mine {
        Some(mine) if mine >= theirs => Err(format!(
            "this machine is the newest primary (timeline {mine}; {other} is on {theirs}): run `sudo scannerpulse db rejoin` on {other} instead"
        )),
        _ => Ok(()),
    }
}

/// pg_controldata's report of this machine's data directory.
fn controldata(system: &System) -> Result<String, String> {
    let output = Command::new("runuser")
        .args([
            "-u",
            "postgres",
            "--",
            &format!("{}/pg_controldata", system.postgres_bin()),
            "-D",
            system.postgres_data(),
        ])
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("cannot run pg_controldata: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("pg_controldata: {}", super::apply::said(&output)))
    }
}

/// A field of pg_controldata's report.
fn report_field(report: &str, name: &str) -> Option<String> {
    report.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

/// The timeline of the last checkpoint, from pg_controldata's report.
fn control_timeline(report: &str) -> Option<u32> {
    report_field(report, "Latest checkpoint's TimeLineID")?
        .parse()
        .ok()
}

/// What a database machine says of itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Status {
    /// Its PostgreSQL answers.
    pub answers: bool,
    pub standby: bool,
    /// How many standbys stream from it, and how far behind the furthest
    /// one has replayed, in bytes (a primary only).
    pub streaming: u32,
    pub behind: u64,
}

pub fn status() -> Status {
    let Ok(standby) = psql("SELECT pg_is_in_recovery()") else {
        return Status {
            answers: false,
            standby: false,
            streaming: 0,
            behind: 0,
        };
    };
    let standby = standby == "t";
    let (streaming, behind) = if standby {
        (0, 0)
    } else {
        psql(
            "SELECT count(*) || ' ' || coalesce(max(pg_wal_lsn_diff(pg_current_wal_lsn(), replay_lsn)), 0)::bigint \
             FROM pg_stat_replication WHERE state = 'streaming'",
        )
        .ok()
        .and_then(|line| {
            let (n, b) = line.split_once(' ')?;
            Some((n.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, 0))
    };
    Status {
        answers: true,
        standby,
        streaming,
        behind,
    }
}

fn system() -> Result<System, String> {
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    super::system::detect(&os_release, std::env::consts::ARCH)
}

fn systemctl(verb: &str, unit: &str) -> Result<(), String> {
    let output = Command::new("systemctl")
        .args([verb, unit])
        .output()
        .map_err(|error| format!("cannot run systemctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl {verb} {unit}: {}",
            super::apply::said(&output)
        ))
    }
}

/// Stops this machine's PostgreSQL, the primary, cleanly, and gives where
/// its last checkpoint starts: a standby that has replayed past it has
/// everything the primary wrote.
pub fn stop() -> Result<String, String> {
    let system = system()?;
    systemctl("stop", system.postgres_cluster())?;
    shut_down_at(&controldata(&system)?)
}

/// The location of the last checkpoint, from pg_controldata's report of a
/// primary that stopped cleanly.
fn shut_down_at(report: &str) -> Result<String, String> {
    let field = |name: &str| report_field(report, name);
    match field("Database cluster state").as_deref() {
        Some("shut down") => {}
        state => {
            return Err(format!(
                "PostgreSQL did not stop cleanly (its state: {})",
                state.unwrap_or("unknown")
            ));
        }
    }
    field("Latest checkpoint location")
        .ok_or_else(|| "pg_controldata did not give the last checkpoint".into())
}

/// Starts this machine's PostgreSQL.
pub fn start() -> Result<(), String> {
    systemctl("start", system()?.postgres_cluster())
}

/// Makes this standby the primary once it has replayed past `lsn`, the
/// stopped primary's last checkpoint; refused if it has not within 30 s.
pub fn promote_after(lsn: &str) -> Result<(), String> {
    for _ in 0..30 {
        if psql(&format!(
            "SELECT pg_last_wal_replay_lsn() > '{lsn}'::pg_lsn"
        ))? == "t"
        {
            return promote();
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    Err(format!(
        "this machine has not received everything the primary wrote (up to {lsn}), so it was not made the primary"
    ))
}

/// Makes this standby the primary.
pub fn promote() -> Result<(), String> {
    let answer = psql("SELECT pg_promote(true, 60)")?;
    if answer == "t" {
        Ok(())
    } else {
        Err("PostgreSQL did not finish the promotion within 60 s".into())
    }
}

/// Makes this machine, stopped cleanly while it was the primary, a standby
/// of `primary`, and starts it.
pub fn follow(primary: &str) -> Result<(), String> {
    let system = system()?;
    let signal = Path::new(system.postgres_data()).join("standby.signal");
    let made = Command::new("runuser")
        .args([
            "-u",
            "postgres",
            "--",
            "touch",
            &signal.display().to_string(),
        ])
        .status()
        .map_err(|error| format!("cannot run runuser: {error}"))?;
    if !made.success() {
        return Err(format!("cannot make {}", signal.display()));
    }
    systemctl("start", system.postgres_cluster())?;
    let conninfo = conninfo(primary, &system);
    psql(&format!(
        "ALTER SYSTEM SET primary_conninfo = '{}'",
        conninfo.replace('\'', "''")
    ))?;
    psql("SELECT pg_reload_conf()").map(drop)
}

fn conninfo(primary: &str, system: &System) -> String {
    format!(
        "host={primary} port=5432 user={REPLICATOR} passfile={}/.pgpass sslmode=require",
        system.postgres_home()
    )
}

/// What the hub's key may ask for, through `node serve`.
#[derive(Debug, PartialEq, Eq)]
pub enum Asked {
    Status,
    Stop,
    Start,
    Promote,
    /// Promote once replayed past this location (`hex/hex`).
    PromoteAfter(String),
    Follow(String),
}

/// The command the hub asked for (SSH_ORIGINAL_COMMAND), if it is one of
/// the node's own: a host for `follow` is a name or an address, and a
/// location for `promote after` two hexadecimal numbers, nothing more.
pub fn asked(command: &str) -> Option<Asked> {
    let words: Vec<&str> = command.split_whitespace().collect();
    match words.as_slice() {
        ["status"] => Some(Asked::Status),
        ["stop"] => Some(Asked::Stop),
        ["start"] => Some(Asked::Start),
        ["promote"] => Some(Asked::Promote),
        ["promote", "after", lsn]
            if lsn.split_once('/').is_some_and(|(hi, lo)| {
                [hi, lo]
                    .iter()
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_hexdigit()))
            }) =>
        {
            Some(Asked::PromoteAfter(lsn.to_string()))
        }
        ["follow", host]
            if host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':')) =>
        {
            Some(Asked::Follow(host.to_string()))
        }
        _ => None,
    }
}

/// `scannerpulse node serve`: the forced command of the hub's key.
pub fn serve() -> Result<String, String> {
    let command = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    match asked(&command) {
        None => Err(format!(
            "{command:?} is not a database machine's command: status, stop, start, promote [after LSN] or follow HOST"
        )),
        Some(Asked::Status) => serde_json::to_string(&status()).map_err(|e| e.to_string()),
        Some(Asked::Stop) => stop(),
        Some(Asked::Start) => start().map(|()| "started".into()),
        Some(Asked::Promote) => promote().map(|()| "promoted".into()),
        Some(Asked::PromoteAfter(lsn)) => promote_after(&lsn).map(|()| "promoted".into()),
        Some(Asked::Follow(host)) => follow(&host).map(|()| format!("following {host}")),
    }
}

/// `scannerpulse node backup`, as the postgres account from its timer: a
/// dump of the database on the primary; a standby has nothing to do.
pub fn backup() -> Result<String, String> {
    if psql("SELECT pg_is_in_recovery()")? == "t" {
        return Ok("a standby: the primary takes the dump".into());
    }
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    crate::backup::dump(
        Path::new(DUMPS),
        &format!(
            "postgresql:///{}?host=/var/run/postgresql",
            super::steps::DATABASE
        ),
        &stamp,
    )
    .map(|name| format!("dump: {name} written"))
}

/// `scannerpulse db promote`, as root on the standby, when the primary is
/// gone: refused while the other machine still answers as the primary, so
/// two primaries never write at once.
pub fn db_promote() -> Result<String, String> {
    let node = NodeFile::read()?;
    if psql("SELECT pg_is_in_recovery()")? != "t" {
        return Err("this machine is the primary already".into());
    }
    if other_is_standby(node.other()) == Some(false) {
        return Err(format!(
            "{} still answers as the primary: for a planned change of roles, run `scannerpulse db switchover` on the ScannerPulse machine",
            node.other()
        ));
    }
    promote()?;
    Ok(format!(
        "This machine is the primary now; the ScannerPulse machines move to it within seconds. When {} is back, run `sudo scannerpulse db rejoin` there.",
        node.other()
    ))
}

/// `scannerpulse db rejoin`, as root on a former primary (or a standby that
/// no longer receives): it becomes the standby of the other machine, rewound to where their histories split,
/// or (after the person agrees) copied whole.
pub fn db_rejoin(confirm: impl Fn(&str) -> bool) -> Result<String, String> {
    let node = NodeFile::read()?;
    let other = node.other().to_string();
    let system = system()?;
    may_rejoin(my_timeline(&system), other_timeline(&other), &other)?;
    // A standby that receives from the other has nothing to rejoin; one
    // that does not (it fell behind, or their histories split) has.
    if psql("SELECT count(*) FROM pg_stat_wal_receiver WHERE status = 'streaming'").as_deref()
        == Ok("1")
    {
        return Err(format!(
            "this machine is a standby already, receiving from {other}"
        ));
    }
    systemctl("stop", system.postgres_cluster())?;
    let rewind = Command::new("runuser")
        .args([
            "-u",
            "postgres",
            "--",
            &format!("{}/pg_rewind", system.postgres_bin()),
            &format!("--target-pgdata={}", system.postgres_data()),
            &format!(
                "--source-server=host={other} port=5432 user={REPLICATOR} dbname=postgres sslmode=require"
            ),
            "--write-recovery-conf",
        ])
        .output()
        .map_err(|error| format!("cannot run pg_rewind: {error}"))?;
    if !rewind.status.success() {
        let why = super::apply::said(&rewind);
        if !confirm(&format!(
            "pg_rewind could not rewind this machine ({why}). Copy all of {other}'s data over this machine's instead?"
        )) {
            return Err(format!("not rejoined: pg_rewind said: {why}"));
        }
        let copy = Command::new("/bin/sh")
            .args(["-c", COPY_OVER])
            .envs([
                ("DATA", system.postgres_data()),
                ("BIN", system.postgres_bin()),
                ("PRIMARY", other.as_str()),
            ])
            .output()
            .map_err(|error| format!("cannot copy: {error}"))?;
        if !copy.status.success() {
            return Err(format!("the copy failed: {}", super::apply::said(&copy)));
        }
    }
    systemctl("start", system.postgres_cluster())?;
    Ok(format!("This machine follows {other} as its standby now."))
}

/// A former primary's data, replaced by a copy of the new one's.
const COPY_OVER: &str = r#"set -e
find "$DATA" -mindepth 1 -delete
runuser -u postgres -- "$BIN/pg_basebackup" -d "host=$PRIMARY port=5432 user=replicator sslmode=require" -D "$DATA" -R -X stream -c fast
"#;

/// The most a standby may be behind for a planned switchover, in bytes.
pub const SWITCH_BEHIND: u64 = 16 * 1024 * 1024;

/// Which of two machines' statuses is the primary and which its standby,
/// ready for a switchover: both answer, the primary streams to a standby
/// that is caught up within `SWITCH_BEHIND`.
pub fn ready_to_switch(first: &Status, second: &Status) -> Result<(usize, usize), String> {
    let (primary, standby) = match (first, second) {
        (a, b) if a.answers && b.answers && !a.standby && b.standby => (0, 1),
        (a, b) if a.answers && b.answers && a.standby && !b.standby => (1, 0),
        _ => {
            return Err(
                "both database machines must answer, one the primary and the other its standby"
                    .into(),
            );
        }
    };
    let p = [first, second][primary];
    if p.streaming == 0 {
        return Err("the standby is not streaming from the primary".into());
    }
    if p.behind > SWITCH_BEHIND {
        return Err(format!(
            "the standby is {} MiB behind: try again when it has caught up",
            p.behind / (1024 * 1024)
        ));
    }
    Ok((primary, standby))
}

/// Runs a node command on `node` with the hub's key.
fn on_node(node: &str, command: &str) -> Result<String, String> {
    let ssh = super::plan::SSH;
    let output = Command::new("ssh")
        .args([
            "-i",
            &format!("{ssh}/id_ed25519"),
            "-o",
            &format!("UserKnownHostsFile={ssh}/known_hosts"),
            "-o",
            "StrictHostKeyChecking=yes",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            &format!("root@{node}"),
            command,
        ])
        .output()
        .map_err(|error| format!("cannot run ssh: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!("{node}: {}", super::apply::said(&output)))
    }
}

/// A status's text as a status; a machine that gives none does not answer.
fn as_status(answer: Result<String, String>) -> Status {
    answer
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Status {
            answers: false,
            standby: false,
            streaming: 0,
            behind: 0,
        })
}

/// `scannerpulse db switchover`, as root on the ScannerPulse machine: the
/// standby becomes the primary and the primary its standby, while both are
/// healthy. Writes pause for the seconds it takes; nothing is lost, since
/// the primary stops cleanly and its standby has everything first.
pub fn db_switchover(say: impl Fn(&str)) -> Result<String, String> {
    let plan = Plan::read(Path::new(super::plan::PLAN))?;
    switchover(&plan.nodes, on_node, say)
}

/// The switchover between `nodes`, each asked its node commands through
/// `run`. When a step fails, the install is left with one primary and the
/// person is told what to run next.
fn switchover(
    nodes: &[String],
    run: impl Fn(&str, &str) -> Result<String, String>,
    say: impl Fn(&str),
) -> Result<String, String> {
    let status = |node: &str| as_status(run(node, "status"));
    let (p, s) = ready_to_switch(&status(&nodes[0]), &status(&nodes[1]))?;
    let (primary, standby) = (nodes[p].as_str(), nodes[s].as_str());
    // The primary, which may be stopped, is started again unchanged.
    let start_again = |why: String| match run(primary, "start") {
        Ok(_) => format!("{why}; {primary} was started again and is still the primary"),
        Err(error) => format!(
            "{why}; and {primary} did not start again ({error}): start its PostgreSQL there"
        ),
    };
    say(&format!("Stopping {primary}, the primary"));
    let last = match run(primary, "stop") {
        Ok(last) => last,
        Err(error) => return Err(start_again(format!("{primary} did not stop: {error}"))),
    };
    say(&format!("Making {standby} the primary"));
    if let Err(error) = run(standby, &format!("promote after {last}")) {
        // Started again only while the other is still a standby: one whose
        // promotion went ahead would make two primaries.
        let now = status(standby);
        if now.answers && now.standby {
            return Err(start_again(format!(
                "{standby} was not made the primary: {error}"
            )));
        }
        return Err(format!(
            "{standby} may be the primary now ({error}), so {primary} was left stopped: once {standby} answers as the primary, run `sudo scannerpulse db rejoin` on {primary}"
        ));
    }
    say(&format!("Making {primary} its standby"));
    run(primary, &format!("follow {standby}")).map_err(|error| {
        format!(
            "{standby} is the primary now, but {primary} did not follow it ({error}): run `sudo scannerpulse db rejoin` on {primary}"
        )
    })?;
    Ok(format!(
        "{standby} is the primary now and {primary} its standby; the services moved to {standby} within seconds."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            hub: "192.0.2.10".into(),
            hub_key: "ssh-ed25519 AAAAhub scannerpulse-hub\n".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            database: "scannerpulse".into(),
            role: "scannerpulse".into(),
            password: "app-secret-1".into(),
            replication_password: "copy-secret-2".into(),
            joined: Vec::new(),
        }
    }

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn the_access_rules_let_in_the_hub_and_both_machines_over_tls() {
        let text = hba(
            &plan(),
            &ips(&["192.0.2.10"]),
            &ips(&["192.0.2.21", "2001:db8::22"]),
        );
        assert_eq!(
            text,
            "# scannerpulse: the ScannerPulse machine, and both database machines\n\
             hostssl scannerpulse scannerpulse 192.0.2.10/32 scram-sha-256\n\
             hostssl replication replicator 192.0.2.21/32 scram-sha-256\n\
             hostssl postgres replicator 192.0.2.21/32 scram-sha-256\n\
             hostssl replication replicator 2001:db8::22/128 scram-sha-256\n\
             hostssl postgres replicator 2001:db8::22/128 scram-sha-256\n"
        );
    }

    #[test]
    fn the_hubs_key_may_run_only_the_node_commands_from_the_hub() {
        assert_eq!(
            authorized_line(
                &plan().hub_key,
                &ips(&["192.0.2.10", "2001:db8::10"]),
                Path::new("/usr/local/bin/scannerpulse")
            ),
            "restrict,from=\"192.0.2.10,2001:db8::10\",command=\"/usr/local/bin/scannerpulse node serve\" ssh-ed25519 AAAAhub scannerpulse-hub\n"
        );
        assert_eq!(asked("status"), Some(Asked::Status));
        assert_eq!(asked("promote"), Some(Asked::Promote));
        assert_eq!(asked("start"), Some(Asked::Start));
        assert_eq!(
            asked("promote after 0/313CE18"),
            Some(Asked::PromoteAfter("0/313CE18".into()))
        );
        assert_eq!(
            asked("follow db2.example.org"),
            Some(Asked::Follow("db2.example.org".into()))
        );
        for refused in [
            "",
            "sh",
            "follow db2;rm -rf /",
            "follow $(id)",
            "status now",
            "stop db1",
            "promote after 0/313CE18'",
            "promote after x/y",
            "promote after 0/3;rm",
            "promote after 313CE18",
        ] {
            assert_eq!(asked(refused), None, "{refused}");
        }
    }

    #[test]
    fn a_primary_stopped_cleanly_tells_where_its_last_checkpoint_is() {
        let said = |state: &str| {
            format!(
                "pg_control version number:            1700\n\
                 Database cluster state:               {state}\n\
                 Latest checkpoint location:           0/313CE18\n\
                 Latest checkpoint's REDO location:    0/313CE18\n"
            )
        };
        assert_eq!(shut_down_at(&said("shut down")), Ok("0/313CE18".into()));
        for state in [
            "in production",
            "shut down in recovery",
            "in crash recovery",
        ] {
            assert!(
                shut_down_at(&said(state))
                    .unwrap_err()
                    .contains("did not stop cleanly"),
                "{state}"
            );
        }
        assert!(shut_down_at("Database cluster state: shut down\n").is_err());
    }

    fn titles(steps: &[Step]) -> Vec<&str> {
        steps.iter().map(|s| s.title.as_str()).collect()
    }

    /// A step's program, its arguments, its environment and its input.
    type Ran<'a> = (
        &'a str,
        &'a [String],
        &'a [(String, String)],
        Option<&'a str>,
    );

    fn ran<'a>(steps: &'a [Step], title: &str) -> Ran<'a> {
        let step = steps
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("{title}"));
        let Action::Run {
            program,
            args,
            env,
            input,
            ..
        } = &step.action
        else {
            panic!("{title}")
        };
        (program, args, env, input.as_deref())
    }

    #[test]
    fn a_join_runs_again_whatever_role_the_machine_has_now() {
        use crate::setup::system::detect;
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let binary = Path::new("/usr/local/bin/scannerpulse");
        let (hub, nodes) = (ips(&["192.0.2.10"]), ips(&["192.0.2.21", "192.0.2.22"]));
        let first = steps(&plan(), 0, &debian, binary, &hub, &nodes);
        let second = steps(&plan(), 1, &debian, binary, &hub, &nodes);
        // Machine 1, a standby since a change of roles, takes its roles
        // from the primary; and no password reaches PostgreSQL's log.
        let (program, args, _, input) =
            ran(&first, "Make the database, its role, and the copying role");
        assert_eq!(program, "/bin/sh");
        let body = &args[1];
        assert!(
            body.find("pg_is_in_recovery").unwrap() < body.find("ON_ERROR_STOP").unwrap(),
            "{body}"
        );
        assert!(
            input
                .unwrap()
                .starts_with("SET log_min_error_statement = panic;\n"),
            "{input:?}"
        );
        // Machine 2, the primary since a change of roles, is left as it is;
        // a copy cut short is made again; Debian's cluster is stopped by
        // its own unit, which waits.
        let (_, args, env, _) = ran(&second, "Copy the primary's data, and follow it");
        assert!(args[1].contains("already the primary"), "{}", args[1]);
        assert!(args[1].contains("PG_VERSION"), "{}", args[1]);
        assert!(env.contains(&("SERVICE".into(), "postgresql@17-main".into())));
        // With no cluster left by a copy cut short, the steps before the
        // copy have nothing to change.
        for title in [
            "Let PostgreSQL take connections from the network, and keep what a rewind needs",
            "Restart PostgreSQL 17",
        ] {
            let (program, args, env, _) = ran(&second, title);
            assert_eq!(program, "/bin/sh", "{title}");
            assert!(
                args[1].starts_with("[ -f \"$DATA/PG_VERSION\" ] || exit 0"),
                "{title}"
            );
            assert!(env.contains(&("DATA".into(), "/var/lib/postgresql/17/main".into())));
        }
        let (_, _, env, _) = ran(&second, "Restart PostgreSQL 17");
        assert!(env.contains(&("SERVICE".into(), "postgresql@17-main".into())));
        // Machine 1, a standby since a change of roles whose copy was cut
        // short, copies the other machine before anything asks its cluster.
        let one = titles(&first);
        let copy = "Copy the other machine's data, if this machine has none";
        let at = |title: &str| one.iter().position(|t| *t == title).unwrap();
        assert!(at(copy) < at("Make the database, its role, and the copying role"));
        let (_, args, env, _) = ran(&first, copy);
        assert!(
            args[1].starts_with("[ -f \"$DATA/PG_VERSION\" ] && exit 0"),
            "{}",
            args[1]
        );
        assert!(env.contains(&("PRIMARY".into(), "db2.example.org".into())));
    }

    #[test]
    fn a_database_machine_under_firewalld_lets_postgresql_in() {
        use crate::setup::system::detect;
        let rocky = detect("ID=\"rocky\"\nVERSION_ID=\"9.6\"\n", "x86_64").unwrap();
        let all = steps(
            &plan(),
            1,
            &rocky,
            Path::new("/usr/local/bin/scannerpulse"),
            &ips(&["192.0.2.10"]),
            &ips(&["192.0.2.21", "192.0.2.22"]),
        );
        let list = titles(&all);
        let at = |title: &str| {
            list.iter()
                .position(|t| *t == title)
                .unwrap_or_else(|| panic!("{title}: {list:?}"))
        };
        let open = super::super::steps::firewall(true, 5432).title;
        assert!(at("Start PostgreSQL 17, now and at boot") < at(&open));
        assert!(at(&open) < at("Copy the primary's data, and follow it"));
    }

    #[test]
    fn the_first_machine_makes_the_database_and_the_second_copies_it() {
        use crate::setup::system::detect;
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let binary = Path::new("/usr/local/bin/scannerpulse");
        let hub = ips(&["192.0.2.10"]);
        let nodes = ips(&["192.0.2.21", "192.0.2.22"]);
        let first = steps(&plan(), 0, &debian, binary, &hub, &nodes);
        let second = steps(&plan(), 1, &debian, binary, &hub, &nodes);
        let one = titles(&first);
        let two = titles(&second);
        assert!(one.contains(&"Make the database, its role, and the copying role"));
        assert!(!one.contains(&"Copy the primary's data, and follow it"));
        assert!(two.contains(&"Copy the primary's data, and follow it"));
        assert!(!two.contains(&"Make the database, its role, and the copying role"));
        let at = |list: &[&str], title: &str| list.iter().position(|t| *t == title).unwrap();
        assert!(
            at(
                &two,
                "Let the ScannerPulse machine and both database machines in"
            ) < at(&two, "Restart PostgreSQL 17")
        );
        assert!(
            at(
                &two,
                "Write the copying role's password file, for the postgres account"
            ) < at(&two, "Copy the primary's data, and follow it")
        );
        let Action::Write { contents, mode, .. } = &first[at(
            &one,
            "Write this machine's place among the database machines",
        )]
        .action
        else {
            panic!()
        };
        assert_eq!(*mode, 0o600);
        // The nightly dump runs as postgres, and every command first looks
        // for the settings file there: none on a database machine.
        let Action::Run { args, .. } = &first[at(&one, "Make the settings folder")].action else {
            panic!()
        };
        assert_eq!(args[..3], ["-d", "-m", "0755"]);
        let file: NodeFile = serde_json::from_str(contents).unwrap();
        assert_eq!(file.index, 0);
        assert_eq!(file.other(), "db2.example.org");
        assert!(!format!("{file:?}").contains("secret"));
        let Action::Run { input, .. } =
            &first[at(&one, "Make the database, its role, and the copying role")].action
        else {
            panic!()
        };
        let sql = input.as_deref().unwrap();
        assert!(
            sql.contains("CREATE ROLE replicator REPLICATION LOGIN"),
            "{sql}"
        );
        assert!(
            sql.contains("pg_read_binary_file(text, bigint, bigint, boolean)"),
            "{sql}"
        );
    }

    /// A switchover against two pretend machines: `answer` gives each
    /// command's answer (the standby's status changes once it is asked to
    /// become the primary); every command asked is recorded.
    fn switch(
        answer: impl Fn(&str, &str, bool) -> Result<String, String>,
    ) -> (Result<String, String>, Vec<String>) {
        let asked = std::cell::RefCell::new(Vec::new());
        let promoted = std::cell::Cell::new(false);
        let nodes = ["db1.example.org".to_string(), "db2.example.org".to_string()];
        let result = switchover(
            &nodes,
            |node, command| {
                asked.borrow_mut().push(format!("{node} {command}"));
                if command.starts_with("promote") {
                    promoted.set(true);
                }
                answer(node, command, promoted.get())
            },
            |_| {},
        );
        (result, asked.into_inner())
    }

    const PRIMARY: &str = r#"{"answers":true,"standby":false,"streaming":1,"behind":0}"#;
    const STANDBY: &str = r#"{"answers":true,"standby":true,"streaming":0,"behind":0}"#;
    const PROMOTED: &str = r#"{"answers":true,"standby":false,"streaming":0,"behind":0}"#;

    fn healthy(node: &str, command: &str, promoted: bool) -> Result<String, String> {
        match (node, command) {
            ("db1.example.org", "status") => Ok(PRIMARY.into()),
            ("db2.example.org", "status") if promoted => Ok(PROMOTED.into()),
            ("db2.example.org", "status") => Ok(STANDBY.into()),
            (_, "stop") => Ok("0/313CE18".into()),
            _ => Ok("done".into()),
        }
    }

    #[test]
    fn a_switchover_stops_promotes_and_follows() {
        let (result, asked) = switch(healthy);
        assert!(
            result
                .unwrap()
                .contains("db2.example.org is the primary now")
        );
        assert_eq!(
            asked,
            [
                "db1.example.org status",
                "db2.example.org status",
                "db1.example.org stop",
                "db2.example.org promote after 0/313CE18",
                "db1.example.org follow db2.example.org",
            ]
        );
    }

    #[test]
    fn a_primary_that_did_not_stop_cleanly_is_started_again() {
        let (result, asked) = switch(|node, command, promoted| match command {
            "stop" => Err("PostgreSQL did not stop cleanly".into()),
            _ => healthy(node, command, promoted),
        });
        let error = result.unwrap_err();
        assert!(error.contains("started again"), "{error}");
        assert!(asked.contains(&"db1.example.org start".to_string()));
        assert!(!asked.iter().any(|a| a.contains("promote")));
    }

    #[test]
    fn a_standby_not_promoted_leaves_the_primary_as_it_was() {
        // The standby refused, and still answers as a standby.
        let (result, asked) = switch(|node, command, _| match command {
            c if c.starts_with("promote") => Err("not everything received".into()),
            _ => healthy(node, command, false),
        });
        let error = result.unwrap_err();
        assert!(error.contains("still the primary"), "{error}");
        assert_eq!(asked.last().unwrap(), "db1.example.org start");
    }

    #[test]
    fn a_standby_that_may_be_promoting_is_never_joined_by_a_second_primary() {
        // The answer was lost, but the promotion went ahead.
        let (result, asked) = switch(|node, command, promoted| match command {
            c if c.starts_with("promote") => Err("the connection was lost".into()),
            _ => healthy(node, command, promoted),
        });
        let error = result.unwrap_err();
        assert!(!asked.contains(&"db1.example.org start".to_string()));
        assert!(error.contains("left stopped"), "{error}");
        assert!(error.contains("db rejoin` on db1.example.org"), "{error}");
    }

    #[test]
    fn a_former_primary_that_cannot_follow_is_told_to_rejoin() {
        let (result, _) = switch(|node, command, promoted| match command {
            c if c.starts_with("follow") => Err("cannot start".into()),
            _ => healthy(node, command, promoted),
        });
        let error = result.unwrap_err();
        assert!(
            error.contains("db2.example.org is the primary now"),
            "{error}"
        );
        assert!(error.contains("db rejoin` on db1.example.org"), "{error}");
    }

    #[test]
    fn rejoin_is_refused_on_the_machine_with_the_newest_timeline() {
        let other = "db2.example.org";
        // The other is the primary and this one is behind: rejoin.
        assert_eq!(may_rejoin(Some(3), Some(4), other), Ok(()));
        assert_eq!(may_rejoin(None, Some(4), other), Ok(()));
        // The other is not the primary (a standby, or no answer).
        assert!(
            may_rejoin(Some(3), None, other)
                .unwrap_err()
                .contains("must be the primary first")
        );
        // Both answer as primaries and this one is the newest: rewinding it
        // would throw away everything written since the failover.
        for mine in [4, 5] {
            let error = may_rejoin(Some(mine), Some(4), other).unwrap_err();
            assert!(error.contains("newest primary"), "{error}");
            assert!(error.contains("db rejoin` on db2.example.org"), "{error}");
        }
    }

    #[test]
    fn a_machines_timeline_is_read_from_its_report_when_it_is_stopped() {
        let report = "Database cluster state:               shut down\n\
                      Latest checkpoint's TimeLineID:       7\n";
        assert_eq!(control_timeline(report), Some(7));
        assert_eq!(
            control_timeline("Database cluster state: shut down\n"),
            None
        );
    }

    #[test]
    fn a_switchover_needs_both_machines_and_a_caught_up_standby() {
        let primary = Status {
            answers: true,
            standby: false,
            streaming: 1,
            behind: 4096,
        };
        let standby = Status {
            answers: true,
            standby: true,
            streaming: 0,
            behind: 0,
        };
        assert_eq!(ready_to_switch(&primary, &standby), Ok((0, 1)));
        assert_eq!(ready_to_switch(&standby, &primary), Ok((1, 0)));
        let down = Status {
            answers: false,
            ..standby.clone()
        };
        assert!(ready_to_switch(&primary, &down).is_err());
        assert!(ready_to_switch(&primary, &primary).is_err());
        let alone = Status {
            streaming: 0,
            ..primary.clone()
        };
        assert!(
            ready_to_switch(&alone, &standby)
                .unwrap_err()
                .contains("not streaming")
        );
        let far = Status {
            behind: SWITCH_BEHIND + 1,
            ..primary
        };
        assert!(
            ready_to_switch(&far, &standby)
                .unwrap_err()
                .contains("behind")
        );
    }
}
