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
    /// The referee, when it promotes the standby; none by hand.
    #[serde(default)]
    pub referee: Option<String>,
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
            "hostssl {} {} {} scram-sha-256{TAG}",
            plan.database,
            plan.role,
            cidr(ip)
        ));
    }
    for ip in nodes {
        for database in ["replication", "postgres"] {
            lines.push(format!(
                "hostssl {database} {REPLICATOR} {} scram-sha-256{TAG}",
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
    const QUIET: &str = super::steps::QUIET_SQL;
    format!(
        "{QUIET}\
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
pub(super) fn script(
    title: &str,
    body: &str,
    vars: &[(&str, &str)],
    input: Option<String>,
) -> Step {
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
    let mut steps = install(system, plan.referee.is_some());
    match &plan.referee {
        Some(referee) => steps.extend(by_referee(plan, index, referee, system, hub)),
        None => steps.extend(by_hand(plan, index, system, binary, hub, nodes)),
    }
    steps.push(hub_key_step(plan, hub, binary));
    steps.extend(tail(plan, index, system, binary));
    steps
}

/// PostgreSQL 17 from the PostgreSQL project's repository, with
/// pg_auto_failover when a referee promotes the standby (`auto`).
pub(super) fn install(system: &System, auto: bool) -> Vec<Step> {
    let mut steps = Vec::new();
    let title = if auto {
        "Install PostgreSQL 17, pg_auto_failover and SSH"
    } else {
        "Install PostgreSQL 17 and SSH"
    };
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
            let mut args = vec![
                "-o",
                wait,
                "install",
                "-y",
                "postgresql-17",
                "openssh-server",
            ];
            if auto {
                args.push("postgresql-17-auto-failover");
            }
            steps.push(run(title, "apt-get", &args));
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
            let mut args = vec!["install", "-y", "postgresql17-server", "openssh-server"];
            if auto {
                args.push("pg_auto_failover_17");
            }
            steps.push(run(title, "dnf", &args));
        }
    }
    steps
}

/// Debian's packages make a cluster (17 main) and start it; with the
/// referee, pg_autoctl keeps its own: the package's, still empty, goes.
pub(super) fn drop_debians_cluster() -> Step {
    script(
        "Remove the empty cluster Debian's package made, for pg_autoctl's own",
        "D=/var/lib/postgresql/17/main\n[ -d \"$D/base\" ] || exit 0\ntables=$(runuser -u postgres -- psql -d postgres -tAXc \"SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.relkind IN ('r', 'p') AND n.nspname NOT IN ('pg_catalog', 'information_schema')\") || { echo \"PostgreSQL 17's cluster (17 main) does not answer, so setup cannot see that it is empty: start it, or remove it yourself\" >&2; exit 1; }\nif [ \"$(ls \"$D/base\" | wc -l)\" -gt 3 ] || [ \"$tables\" != 0 ]; then\n  echo \"PostgreSQL 17's cluster (17 main) holds databases or tables of its own: setup will not remove it\" >&2; exit 1\nfi\npg_dropcluster --stop 17 main",
        &[],
        None,
    )
}

/// The steps by hand: the package's own cluster, its access rules, and the
/// standby's copy.
fn by_hand(
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
    if system.family == Family::Dnf {
        {
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
        REPLACE_BLOCK,
        &[("MARK", MARK), ("HBA", system.postgres_hba())],
        Some(hba(plan, hub, nodes)),
    ));
    steps.push(script(
        "Restart PostgreSQL 17",
        "[ -f \"$DATA/PG_VERSION\" ] || exit 0\nsystemctl restart \"$SERVICE\"",
        &[("DATA", data), ("SERVICE", system.postgres_cluster())],
        None,
    ));
    steps.push(pgpass_step(
        "Write the copying role's password file, for the postgres account",
        system,
        &[REPLICATOR],
        format!("*:*:*:{REPLICATOR}:{}\n", plan.replication_password),
    ));
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
    steps
}

/// The referee's monitor, as pg_autoctl and psql reach it.
pub fn monitor_uri(referee: &str) -> String {
    let host = if referee.contains(':') {
        format!("[{referee}]")
    } else {
        referee.to_string()
    };
    format!("postgres://autoctl_node@{host}:5432/pg_auto_failover?sslmode=require")
}

/// pg_autoctl's data directory on a database machine.
pub fn autoctl_data(system: &System) -> String {
    format!("{}/17/sp", system.postgres_home())
}

/// `pg_autoctl ARGS` as the postgres account, with PostgreSQL 17's programs
/// first on its path.
pub(super) fn autoctl(system: &System, args: &[&str]) -> Vec<String> {
    let mut all: Vec<String> = [
        "-u",
        "postgres",
        "--",
        "env",
        "-u",
        "XDG_RUNTIME_DIR",
        "LANG=C.UTF-8",
        "LC_ALL=C.UTF-8",
    ]
    .map(String::from)
    .to_vec();
    all.push(format!("PATH={}:/usr/bin:/bin", system.postgres_bin()));
    all.push("pg_autoctl".into());
    all.extend(args.iter().map(|a| a.to_string()));
    all
}

/// pg_autoctl's service unit, written once its node is made: a node whose
/// making was cut short has none, and `pg_autoctl create` resumes it.
pub(super) const AUTOCTL_UNIT: &str = "/etc/systemd/system/pgautofailover.service";

/// pg_autoctl's data directory, in a folder of postgres's: pg_autoctl
/// keeps its backups beside it.
pub(super) fn autoctl_folder(title: &str, data: &str) -> Step {
    script(
        title,
        "install -d -m 0755 -o postgres -g postgres \"$(dirname \"$DATA\")\" && install -d -m 0700 -o postgres -g postgres \"$DATA\"",
        &[("DATA", data)],
        None,
    )
}

/// pg_autoctl's service, as pg_autoctl writes it, started now and at boot.
pub(super) fn autoctl_service(system: &System, data: &str) -> Vec<Step> {
    vec![
        script(
            "Write pg_autoctl's service",
            "runuser -u postgres -- env -u XDG_RUNTIME_DIR PATH=\"$BIN:/usr/bin:/bin\" pg_autoctl -q show systemd --pgdata \"$DATA\" > \"$UNIT.new\" && mv \"$UNIT.new\" \"$UNIT\"",
            &[
                ("BIN", system.postgres_bin()),
                ("DATA", data),
                ("UNIT", AUTOCTL_UNIT),
            ],
            None,
        ),
        run("Read the new units", "systemctl", &["daemon-reload"]),
        run(
            "Start pg_autoctl, now and at boot",
            "systemctl",
            &["enable", "--now", "pgautofailover"],
        ),
    ]
}

/// The steps with a referee: pg_autoctl keeps this machine's cluster,
/// registered with the referee, which makes the second a copy of the first
/// and promotes it by itself.
fn by_referee(
    plan: &Plan,
    index: usize,
    referee: &str,
    system: &System,
    hub: &[IpAddr],
) -> Vec<Step> {
    let mut steps = Vec::new();
    let data = autoctl_data(system);
    let name = plan.nodes[index].as_str();
    if system.family == Family::Apt {
        steps.push(drop_debians_cluster());
    }
    steps.push(super::steps::firewall(true, 5432));
    steps.push(pgpass_step(
        "Write the passwords for the copy and the referee, for the postgres account",
        system,
        &["pgautofailover_replicator", "autoctl_node"],
        format!(
            "*:*:*:pgautofailover_replicator:{}\n*:*:pg_auto_failover:autoctl_node:{}\n",
            plan.replication_password, plan.monitor_password
        ),
    ));
    steps.push(autoctl_folder("Make pg_autoctl's data directory", &data));
    let monitor = monitor_uri(referee);
    steps.push(Step {
        title: "Register this machine with the referee".into(),
        action: Action::Run {
            program: "runuser".into(),
            args: autoctl(
                system,
                &[
                    "create",
                    "postgres",
                    "--pgdata",
                    &data,
                    "--pgport",
                    "5432",
                    "--hostname",
                    name,
                    "--name",
                    name,
                    "--dbname",
                    "postgres",
                    "--monitor",
                    &monitor,
                    "--auth",
                    "scram-sha-256",
                    "--ssl-self-signed",
                    "--replication-quorum",
                    "true",
                    "--candidate-priority",
                    "50",
                ],
            ),
            input: None,
            env: Vec::new(),
            unless: Some(AUTOCTL_UNIT.into()),
        },
    });
    steps.push(script(
        "Check that the referee still knows this machine",
        CHECK_REGISTERED,
        &[
            ("UNIT", AUTOCTL_UNIT),
            ("MONITOR", &monitor),
            ("NAME", name),
            ("DATA", &data),
            ("BIN", system.postgres_bin()),
        ],
        None,
    ));
    steps.push(script(
        "Let the ScannerPulse machine in",
        REPLACE_BLOCK,
        &[("MARK", HUB_MARK), ("HBA", &format!("{data}/pg_hba.conf"))],
        Some(hub_hba(hub)),
    ));
    steps.extend(autoctl_service(system, &data));
    // Once its PostgreSQL answers: the access rules read again, and on the
    // primary the database, its role, and the copying role's password
    // (which the standby's copy and every rewind sign in with).
    let wait = "for i in $(seq 120); do runuser -u postgres -- psql -tAXc 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done\nrunuser -u postgres -- psql -qtAXc 'SELECT pg_reload_conf()' >/dev/null";
    if index == 0 {
        steps.push(script(
            "Make the database, its role, and the copying role's password",
            &format!("{wait}\nif runuser -u postgres -- psql -tAXc 'SELECT pg_is_in_recovery()' | grep -qx t; then\n  echo \"a standby: its roles come from the primary\"; exit 0\nfi\nrunuser -u postgres -- psql -q -v ON_ERROR_STOP=1"),
            &[],
            Some(
                super::steps::database_sql(&plan.password)
                    + super::steps::QUIET_SQL
                    + &format!(
                        "SELECT 'CREATE ROLE pgautofailover_replicator LOGIN REPLICATION' WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'pgautofailover_replicator') \\gexec\nALTER ROLE pgautofailover_replicator PASSWORD '{}';\n",
                        plan.replication_password.replace('\'', "''")
                    ),
            ),
        ));
    } else {
        steps.push(script(
            "Wait for this machine's PostgreSQL",
            wait,
            &[],
            None,
        ));
    }
    steps
}

/// A machine registered before (its pg_autoctl service written) that the
/// referee no longer knows: the referee was made anew. Registering it again
/// is pg_auto_failover's own, the primary first: the steps are said.
const CHECK_REGISTERED: &str = r#"[ -f "$UNIT" ] || exit 0
n=$(runuser -u postgres -- psql "$MONITOR" -tAXc "SELECT count(*) FROM pgautofailover.node WHERE nodehost = '$NAME'") || { echo "the referee does not answer ($MONITOR)" >&2; exit 1; }
[ "$n" -gt 0 ] && exit 0
echo "the referee no longer knows this machine (was it made anew?). Register it again, the primary first, then the standby: runuser -u postgres -- env PATH=$BIN:/usr/bin:/bin pg_autoctl disable monitor --force --pgdata $DATA; then runuser -u postgres -- env PATH=$BIN:/usr/bin:/bin pg_autoctl enable monitor --pgdata $DATA '$MONITOR'; then systemctl restart pgautofailover" >&2
exit 1
"#;

/// The end of each of setup's lines in an access-rules file: pg_autoctl and
/// the person add their own rules to the same file.
pub(super) const TAG: &str = " # scannerpulse";

/// Setup's block in an access-rules file (`$HBA`), its marker `$MARK` and
/// its lines (each ending in `TAG`), made anew from standard input. Only
/// setup's own lines go: pg_autoctl's and the person's stay, and so do the
/// file's owner and mode. A block from before the lines were tagged is its
/// marker and the lines setup writes right after it.
pub(super) const REPLACE_BLOCK: &str = "T=$(mktemp) && awk -v m=\"$MARK\" '$0 == m { old = 1; next } / # scannerpulse$/ { next } old && /^hostssl (scannerpulse scannerpulse|replication replicator|postgres replicator|pg_auto_failover autoctl_node) / { next } { old = 0; print }' \"$HBA\" > \"$T\" && cat >> \"$T\" && cat \"$T\" > \"$HBA\" && rm -f \"$T\"";

/// The marker of setup's line for the ScannerPulse machine in pg_autoctl's
/// access rules.
const HUB_MARK: &str = "# scannerpulse: the ScannerPulse machine";

/// The access rules that let the ScannerPulse machine reach the database.
fn hub_hba(hub: &[IpAddr]) -> String {
    let mut text = format!("{HUB_MARK}\n");
    for ip in hub {
        let bits = if ip.is_ipv4() { 32 } else { 128 };
        text.push_str(&format!(
            "hostssl {} {} {ip}/{bits} scram-sha-256{TAG}\n",
            super::steps::DATABASE,
            super::steps::ACCOUNT
        ));
    }
    text
}

/// postgres's password file with `lines` for `roles`: the file's lines for
/// other roles stay; the file is postgres's alone.
fn pgpass_step(title: &str, system: &System, roles: &[&str], lines: String) -> Step {
    let path = Path::new(system.postgres_home())
        .join(".pgpass")
        .display()
        .to_string();
    script(
        title,
        "umask 077\nT=$(mktemp) && { [ -f \"$PGPASS\" ] && awk -F: -v roles=\" $ROLES \" 'index(roles, \" \" $4 \" \") == 0' \"$PGPASS\"; cat; } > \"$T\" && cat \"$T\" > \"$PGPASS\" && rm -f \"$T\" && chmod 600 \"$PGPASS\" && { [ -z \"$OWNER\" ] || chown \"$OWNER\" \"$PGPASS\"; }",
        &[
            ("PGPASS", &path),
            ("ROLES", &roles.join(" ")),
            ("OWNER", "postgres:postgres"),
        ],
        Some(lines),
    )
}

/// The ScannerPulse machine's key may run this machine's node commands.
pub(super) fn hub_key_step(plan: &Plan, hub: &[IpAddr], binary: &Path) -> Step {
    script(
        "Let the ScannerPulse machine run this machine's database commands",
        "install -d -m 0700 \"$(dirname \"$AK\")\" && touch \"$AK\" && chmod 600 \"$AK\" && T=$(mktemp) && { grep -vF \"$KEY\" \"$AK\" || true; } > \"$T\" && cat >> \"$T\" && cat \"$T\" > \"$AK\" && rm -f \"$T\"",
        &[
            ("KEY", plan.hub_key.trim()),
            ("AK", "/root/.ssh/authorized_keys"),
        ],
        Some(authorized_line(&plan.hub_key, hub, binary)),
    )
}

/// The settings folder, this machine's place, and the nightly dump.
fn tail(plan: &Plan, index: usize, system: &System, binary: &Path) -> Vec<Step> {
    let mut steps = Vec::new();
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
        referee: plan.referee.clone(),
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
    let (service_unit, timer) = dump_units(binary, system, plan.referee.is_some());
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
pub fn dump_units(binary: &Path, system: &System, autoctl: bool) -> (String, String) {
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
            pg = if autoctl {
                "pgautofailover"
            } else {
                system.postgres_service()
            },
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
            "{other} must be the primary first: `sudo /usr/local/bin/scannerpulse db promote` there"
        ));
    };
    match mine {
        Some(mine) if mine >= theirs => Err(format!(
            "this machine is the newest primary (timeline {mine}; {other} is on {theirs}): run `sudo /usr/local/bin/scannerpulse db rejoin` on {other} instead"
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
    /// On the referee: the referee switches the roles.
    Switchover,
    Promote,
    /// Promote once replayed past this location (`hex/hex`).
    PromoteAfter(String),
    Follow(String),
    /// This machine's version and processor (`scannerpulse update` asks
    /// every machine first).
    Version,
    /// The release's program of this name, on standard input with the
    /// release's signed checksums, takes this machine's program's place once
    /// they check out, and its services restart.
    Update(String),
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
        ["switchover"] => Some(Asked::Switchover),
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
        ["version"] => Some(Asked::Version),
        ["update", name] if crate::update::is_program(name) => {
            Some(Asked::Update(name.to_string()))
        }
        _ => None,
    }
}

/// `scannerpulse node serve`: the forced command of the hub's key.
pub fn serve() -> Result<String, String> {
    let command = std::env::var("SSH_ORIGINAL_COMMAND").unwrap_or_default();
    let refereed = NodeFile::read().is_ok_and(|node| node.referee.is_some());
    if let Some(asked) = asked(&command)
        && refused_with_referee(refereed, &asked)
    {
        return Err(format!(
            "{command:?} is not for this machine: the referee changes the roles here"
        ));
    }
    match asked(&command) {
        None => Err(format!(
            "{command:?} is not a database machine's command: status, stop, start, promote [after LSN], follow HOST, switchover, version or update PROGRAM"
        )),
        Some(Asked::Status) => serde_json::to_string(&status()).map_err(|e| e.to_string()),
        Some(Asked::Stop) => stop(),
        Some(Asked::Start) => start().map(|()| "started".into()),
        Some(Asked::Switchover) => super::referee::switchover(&system()?),
        Some(Asked::Promote) => promote().map(|()| "promoted".into()),
        Some(Asked::PromoteAfter(lsn)) => promote_after(&lsn).map(|()| "promoted".into()),
        Some(Asked::Follow(host)) => follow(&host).map(|()| format!("following {host}")),
        Some(Asked::Version) => Ok(crate::update::running_here()),
        Some(Asked::Update(name)) => {
            let version = crate::update::receive(
                Path::new(super::BINARY),
                &name,
                &mut std::io::stdin().lock(),
                &crate::update::release_key()?,
                &crate::update::work_folder(),
                crate::update::VERSION,
            )?;
            crate::update::restart()?;
            Ok(format!("now runs {version}"))
        }
    }
}

/// The commands that change the roles by hand, which a machine with a
/// referee leaves to it.
fn refused_with_referee(referee: bool, asked: &Asked) -> bool {
    referee
        && matches!(
            asked,
            Asked::Stop | Asked::Start | Asked::Promote | Asked::PromoteAfter(_) | Asked::Follow(_)
        )
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
    by_hand_only(node.referee.as_deref(), "promote")?;
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
        "This machine is the primary now; the ScannerPulse machines move to it within seconds. When {} is back, run `sudo /usr/local/bin/scannerpulse db rejoin` there.",
        node.other()
    ))
}

/// `scannerpulse db rejoin`, as root on a former primary (or a standby that
/// no longer receives): it becomes the standby of the other machine, rewound to where their histories split,
/// or (after the person agrees) copied whole.
pub fn db_rejoin(confirm: impl Fn(&str) -> bool) -> Result<String, String> {
    let node = NodeFile::read()?;
    by_hand_only(node.referee.as_deref(), "rejoin")?;
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
    ask(node, command, &[])
}

/// Runs a node command on `node` with the hub's key, `input` on its
/// standard input.
pub fn ask(node: &str, command: &str, input: &[u8]) -> Result<String, String> {
    let ssh = super::plan::SSH;
    let mut child = Command::new("ssh")
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
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot run ssh: {error}"))?;
    // Written while ssh reads, then closed, so the far side sees its end.
    let mut stdin = child.stdin.take();
    let written = stdin
        .as_mut()
        .map(|stdin| std::io::Write::write_all(stdin, input))
        .transpose();
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot run ssh: {error}"))?;
    if let Err(error) = written
        && output.status.success()
    {
        return Err(format!("{node}: the program could not be sent: {error}"));
    }
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
    if let Some(referee) = &plan.referee {
        say(&format!(
            "Asking the referee, {referee}, to switch the roles"
        ));
        return by_the_referee(referee, on_node);
    }
    switchover(&plan.nodes, on_node, say)
}

/// The switchover with a referee: it makes the standby the primary and the
/// primary its standby, through its node command.
fn by_the_referee(
    referee: &str,
    run: impl Fn(&str, &str) -> Result<String, String>,
) -> Result<String, String> {
    run(referee, "switchover")
        .map(|_| {
            "Switched by the referee: the standby is the primary now, and the primary its standby; the services move to the new primary within seconds."
                .into()
        })
        .map_err(|error| format!("the referee ({referee}) did not switch the roles: {error}"))
}

/// `db promote` and `db rejoin` are for machines promoted by hand: with a
/// referee, it promotes the standby, and a former primary rejoins by itself.
fn by_hand_only(referee: Option<&str>, command: &str) -> Result<(), String> {
    match referee {
        Some(referee) => Err(format!(
            "the referee ({referee}) promotes the standby and rejoins a former primary by itself here, so `db {command}` is not needed; for a planned change of roles, run `sudo /usr/local/bin/scannerpulse db switchover` on the ScannerPulse machine"
        )),
        None => Ok(()),
    }
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
    // A machine that does not answer is named with the reason.
    let answers: Vec<Result<String, String>> = nodes.iter().map(|n| run(n, "status")).collect();
    let silent: Vec<String> = nodes
        .iter()
        .zip(&answers)
        .filter_map(|(node, answer)| answer.as_ref().err().map(|why| format!("{node}: {why}")))
        .collect();
    if !silent.is_empty() {
        return Err(format!(
            "both database machines must answer; {}",
            silent.join("; ")
        ));
    }
    let [first, second] = [&answers[0], &answers[1]].map(|a| as_status(a.clone()));
    let (p, s) = ready_to_switch(&first, &second)?;
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
            "{standby} may be the primary now ({error}), so {primary} was left stopped: once {standby} answers as the primary, run `sudo /usr/local/bin/scannerpulse db rejoin` on {primary}"
        ));
    }
    say(&format!("Making {primary} its standby"));
    run(primary, &format!("follow {standby}")).map_err(|error| {
        format!(
            "{standby} is the primary now, but {primary} did not follow it ({error}): run `sudo /usr/local/bin/scannerpulse db rejoin` on {primary}"
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
            referee: None,
            monitor_password: String::new(),
            second: None,
            shared: None,
            install_id: String::new(),
            archive: String::new(),
            recordings: String::new(),
            ready: false,
            joined: Vec::new(),
        }
    }

    /// The same machines, the standby promoted by the referee.
    fn refereed() -> Plan {
        Plan {
            referee: Some("ref.example.org".into()),
            monitor_password: "referee-secret-3".into(),
            ..plan()
        }
    }

    /// Runs a step's script with `/bin/sh`, its values in the environment
    /// with `extra` added, and its input on standard input.
    fn run_script(step: &Step, extra: &[(&str, &str)]) {
        use std::io::Write;
        let Action::Run {
            program,
            args,
            env,
            input,
            ..
        } = &step.action
        else {
            panic!("{}", step.title)
        };
        assert_eq!(program, "/bin/sh", "{}", step.title);
        let mut child = Command::new(program)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .envs(extra.iter().copied())
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_deref().unwrap_or("").as_bytes())
            .unwrap();
        assert!(child.wait().unwrap().success(), "{}", step.title);
    }

    #[test]
    fn joining_again_replaces_the_rules_and_the_key_line() {
        use crate::setup::system::detect;
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let binary = Path::new("/usr/local/bin/scannerpulse");
        let dir = tempfile::tempdir().unwrap();
        // The access rules: another rule, then setup's block for an old
        // address of the hub.
        let rules = dir.path().join("pg_hba.conf");
        let old = steps(
            &plan(),
            0,
            &debian,
            binary,
            &ips(&["192.0.2.9"]),
            &ips(&["192.0.2.21", "192.0.2.22"]),
        );
        let step =
            |all: &[Step], title: &str| all.iter().find(|s| s.title == title).unwrap().clone();
        let hba_title = "Let the ScannerPulse machine and both database machines in";
        std::fs::write(&rules, "local all postgres peer\n").unwrap();
        let path = rules.display().to_string();
        run_script(&step(&old, hba_title), &[("HBA", &path)]);
        let new = steps(
            &plan(),
            0,
            &debian,
            binary,
            &ips(&["192.0.2.10"]),
            &ips(&["192.0.2.21", "192.0.2.22"]),
        );
        // pg_autoctl's own rules, and a person's, after setup's block.
        let others = "hostssl replication \"pgautofailover_replicator\" 192.0.2.22/32 scram-sha-256 # Auto-generated by pg_auto_failover\nhostssl all someone 192.0.2.50/32 scram-sha-256\n";
        let mut text = std::fs::read_to_string(&rules).unwrap();
        text.push_str(others);
        std::fs::write(&rules, &text).unwrap();
        run_script(&step(&new, hba_title), &[("HBA", &path)]);
        let text = std::fs::read_to_string(&rules).unwrap();
        assert!(text.starts_with("local all postgres peer\n"), "{text}");
        assert!(!text.contains("192.0.2.9/"), "{text}");
        assert_eq!(text.matches("192.0.2.10/32").count(), 1, "{text}");
        assert_eq!(text.matches(MARK).count(), 1, "{text}");
        assert!(text.contains(others), "the others' rules stay: {text}");
        // A block written before setup tagged its lines (0.4.0): its own
        // lines go, and a person's rule after it stays.
        std::fs::write(
            &rules,
            format!("local all postgres peer\n{MARK}\nhostssl scannerpulse scannerpulse 192.0.2.9/32 scram-sha-256\nhostssl replication replicator 192.0.2.22/32 scram-sha-256\nhostssl all someone 192.0.2.50/32 scram-sha-256\n"),
        )
        .unwrap();
        run_script(&step(&new, hba_title), &[("HBA", &path)]);
        let text = std::fs::read_to_string(&rules).unwrap();
        assert!(!text.contains("192.0.2.9/"), "{text}");
        assert!(
            text.contains("hostssl all someone 192.0.2.50/32 scram-sha-256\n"),
            "{text}"
        );
        assert_eq!(text.matches(MARK).count(), 1, "{text}");
        // The key: its old line (an old address) gives way to the new one.
        let keys = dir.path().join("authorized_keys");
        std::fs::write(&keys, "ssh-ed25519 AAAAother someone\n").unwrap();
        let key_title = "Let the ScannerPulse machine run this machine's database commands";
        let path = keys.display().to_string();
        run_script(&step(&old, key_title), &[("AK", &path)]);
        run_script(&step(&new, key_title), &[("AK", &path)]);
        let text = std::fs::read_to_string(&keys).unwrap();
        assert!(
            text.starts_with("ssh-ed25519 AAAAother someone\n"),
            "{text}"
        );
        assert!(!text.contains("192.0.2.9"), "{text}");
        assert_eq!(text.matches("AAAAhub").count(), 1, "{text}");
    }

    #[test]
    fn with_a_referee_pg_autoctl_keeps_the_cluster_and_registers_it() {
        use crate::setup::system::detect;
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let binary = Path::new("/usr/local/bin/scannerpulse");
        let (hub, nodes) = (ips(&["192.0.2.10"]), ips(&["192.0.2.21", "192.0.2.22"]));
        let first = steps(&refereed(), 0, &debian, binary, &hub, &nodes);
        let second = steps(&refereed(), 1, &debian, binary, &hub, &nodes);
        let one = titles(&first);
        let at = |title: &str| {
            one.iter()
                .position(|t| *t == title)
                .unwrap_or_else(|| panic!("{title}: {one:?}"))
        };
        // pg_auto_failover's package; the package's own empty cluster goes.
        let (_, args, _, _) = ran(&first, "Install PostgreSQL 17, pg_auto_failover and SSH");
        assert!(args.contains(&"postgresql-17-auto-failover".to_string()));
        let drop = "Remove the empty cluster Debian's package made, for pg_autoctl's own";
        let register = "Register this machine with the referee";
        assert!(at(drop) < at(register));
        // The passwords reach pg_autoctl through postgres's password file,
        // never an argument; the file's other lines stay.
        let passwords =
            "Write the passwords for the copy and the referee, for the postgres account";
        let (_, args, _, input) = ran(&first, passwords);
        assert_eq!(
            input,
            Some(
                "*:*:*:pgautofailover_replicator:copy-secret-2\n*:*:pg_auto_failover:autoctl_node:referee-secret-3\n"
            )
        );
        assert!(!args.join(" ").contains("secret"));
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".pgpass");
        std::fs::write(
            &file,
            "*:*:*:replicator:kept-1\nh:5432:db:pgautofailover_replicator:old-2\n",
        )
        .unwrap();
        let path = file.display().to_string();
        run_script(
            &first.iter().find(|s| s.title == passwords).unwrap().clone(),
            &[("PGPASS", &path), ("OWNER", "")],
        );
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.starts_with("*:*:*:replicator:kept-1\n"), "{text}");
        assert!(!text.contains("old-2"), "{text}");
        assert!(
            text.contains("pgautofailover_replicator:copy-secret-2"),
            "{text}"
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // Debian's cluster is dropped only if it holds no table of the
        // person's, in any database.
        let (_, args, _, _) = ran(&first, drop);
        assert!(args[1].contains("pg_class"), "{}", args[1]);
        // The nightly dump follows pg_autoctl's service.
        let (unit, _) = dump_units(binary, &debian, true);
        assert!(unit.contains("After=pgautofailover.service"), "{unit}");
        assert!(
            at("Write the passwords for the copy and the referee, for the postgres account")
                < at(register)
        );
        let (program, args, _, _) = ran(&first, register);
        assert_eq!(program, "runuser");
        let line = args.join(" ");
        for part in [
            "env -u XDG_RUNTIME_DIR",
            "pg_autoctl create postgres --pgdata /var/lib/postgresql/17/sp --pgport 5432",
            "--hostname db1.example.org --name db1.example.org",
            "--monitor postgres://autoctl_node@ref.example.org:5432/pg_auto_failover?sslmode=require",
            "--auth scram-sha-256 --ssl-self-signed",
        ] {
            assert!(line.contains(part), "{part}: {line}");
        }
        assert!(!line.contains("secret"), "{line}");
        // Run again, a registered machine (its service written) is left as
        // it is; one whose registration was cut short resumes it.
        let Action::Run { unless, .. } = &first[at(register)].action else {
            panic!()
        };
        assert_eq!(
            unless.as_deref(),
            Some(Path::new("/etc/systemd/system/pgautofailover.service"))
        );
        // pg_autoctl keeps its backups beside the data directory, in a
        // folder of postgres's.
        let (_, args, env, _) = ran(&first, "Make pg_autoctl's data directory");
        assert!(
            args[1].contains("install -d -m 0755 -o postgres -g postgres \"$(dirname \"$DATA\")\""),
            "{}",
            args[1]
        );
        assert!(env.contains(&("DATA".into(), "/var/lib/postgresql/17/sp".into())));
        // Joined again, a machine the referee no longer knows (a referee
        // made anew) is refused, with the steps that register it again.
        let check = "Check that the referee still knows this machine";
        assert_eq!(at(check), at(register) + 1);
        let (_, args, env, _) = ran(&first, check);
        assert!(
            args[1].starts_with("[ -f \"$UNIT\" ] || exit 0"),
            "{}",
            args[1]
        );
        assert!(args[1].contains("pgautofailover.node"), "{}", args[1]);
        assert!(
            args[1].contains("disable monitor --force") && args[1].contains("enable monitor"),
            "{}",
            args[1]
        );
        assert!(env.contains(&("NAME".into(), "db1.example.org".into())));
        assert!(!args.join(" ").contains("secret"));
        // The ScannerPulse machine's access, then pg_autoctl's service.
        let hba = "Let the ScannerPulse machine in";
        let (_, _, _, input) = ran(&first, hba);
        assert!(
            input
                .unwrap()
                .contains("hostssl scannerpulse scannerpulse 192.0.2.10/32 scram-sha-256"),
            "{input:?}"
        );
        assert!(at(register) < at(hba));
        assert!(at(hba) < at("Start pg_autoctl, now and at boot"));
        // The first makes the database and its role, and gives the copying
        // role its password, on the primary only; the second has nothing
        // of the kind, and no copy of its own: pg_autoctl makes it.
        let make = "Make the database, its role, and the copying role's password";
        assert!(at("Start pg_autoctl, now and at boot") < at(make));
        let (_, args, _, input) = ran(&first, make);
        assert!(args[1].contains("pg_is_in_recovery"), "{}", args[1]);
        let sql = input.unwrap();
        assert!(sql.starts_with(super::super::steps::QUIET_SQL), "{sql}");
        assert!(
            sql.contains("ALTER ROLE pgautofailover_replicator PASSWORD 'copy-secret-2'"),
            "{sql}"
        );
        let two = titles(&second);
        assert!(!two.contains(&make));
        assert!(!two.contains(&"Copy the primary's data, and follow it"));
        assert!(two.contains(&register));
        // Nothing of the machines promoted by hand.
        assert!(!one.contains(&"Let the ScannerPulse machine and both database machines in"));
        let file: NodeFile = first
            .iter()
            .find_map(|s| match &s.action {
                Action::Write { path, contents, .. } if path == Path::new(NODE) => {
                    serde_json::from_str(contents).ok()
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(file.referee.as_deref(), Some("ref.example.org"));
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
             hostssl scannerpulse scannerpulse 192.0.2.10/32 scram-sha-256 # scannerpulse\n\
             hostssl replication replicator 192.0.2.21/32 scram-sha-256 # scannerpulse\n\
             hostssl postgres replicator 192.0.2.21/32 scram-sha-256 # scannerpulse\n\
             hostssl replication replicator 2001:db8::22/128 scram-sha-256 # scannerpulse\n\
             hostssl postgres replicator 2001:db8::22/128 scram-sha-256 # scannerpulse\n"
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
        assert_eq!(asked("switchover"), Some(Asked::Switchover));
        assert_eq!(
            asked("promote after 0/313CE18"),
            Some(Asked::PromoteAfter("0/313CE18".into()))
        );
        assert_eq!(
            asked("follow db2.example.org"),
            Some(Asked::Follow("db2.example.org".into()))
        );
        assert_eq!(asked("version"), Some(Asked::Version));
        assert_eq!(
            asked("update scannerpulse-x86_64-linux"),
            Some(Asked::Update("scannerpulse-x86_64-linux".into()))
        );
        for refused in [
            "update",
            &format!("update {}", "a".repeat(64)),
            "update scannerpulse-../../x-linux",
            "update scannerpulse--linux",
            "update scannerpulse-x86_64-linux now",
            "version now",
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
            input.unwrap().starts_with(super::super::steps::QUIET_SQL),
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
    fn the_referees_address_is_a_uri_host() {
        assert_eq!(
            monitor_uri("ref.example.org"),
            "postgres://autoctl_node@ref.example.org:5432/pg_auto_failover?sslmode=require"
        );
        assert_eq!(
            monitor_uri("2001:db8::31"),
            "postgres://autoctl_node@[2001:db8::31]:5432/pg_auto_failover?sslmode=require"
        );
    }

    #[test]
    fn with_a_referee_the_hubs_key_may_not_change_the_roles_by_hand() {
        for asked in [
            Asked::Stop,
            Asked::Start,
            Asked::Promote,
            Asked::PromoteAfter("0/3".into()),
            Asked::Follow("db2.example.org".into()),
        ] {
            assert!(refused_with_referee(true, &asked), "{asked:?}");
            assert!(!refused_with_referee(false, &asked), "{asked:?}");
        }
        for asked in [
            Asked::Status,
            Asked::Switchover,
            Asked::Version,
            Asked::Update("scannerpulse-x86_64-linux".into()),
        ] {
            assert!(!refused_with_referee(true, &asked), "{asked:?}");
        }
    }

    #[test]
    fn with_a_referee_the_switchover_is_the_referees() {
        let asked = std::cell::RefCell::new(Vec::new());
        let said = by_the_referee("ref.example.org", |node, command| {
            asked.borrow_mut().push(format!("{node} {command}"));
            Ok("switched".into())
        })
        .unwrap();
        assert_eq!(asked.into_inner(), ["ref.example.org switchover"]);
        assert!(said.contains("the referee"), "{said}");
        let error = by_the_referee("ref.example.org", |_, _| Err("no answer".into())).unwrap_err();
        assert!(
            error.contains("ref.example.org") && error.contains("no answer"),
            "{error}"
        );
    }

    #[test]
    fn with_a_referee_promote_and_rejoin_are_its_own() {
        assert!(
            by_hand_only(Some("ref.example.org"), "promote")
                .unwrap_err()
                .contains("the referee (ref.example.org)")
        );
        assert_eq!(by_hand_only(None, "promote"), Ok(()));
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
    fn a_machine_that_does_not_answer_is_named_with_its_reason() {
        let (result, asked) = switch(|node, command, promoted| match (node, command) {
            ("db2.example.org", "status") => {
                Err("ssh: connect to host db2.example.org port 22: Connection refused".into())
            }
            _ => healthy(node, command, promoted),
        });
        let error = result.unwrap_err();
        assert!(error.contains("db2.example.org"), "{error}");
        assert!(error.contains("Connection refused"), "{error}");
        assert!(!asked.iter().any(|a| a.contains("stop")));
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
