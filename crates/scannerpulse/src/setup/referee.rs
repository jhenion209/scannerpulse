//! The referee: pg_auto_failover's monitor, on a small machine of its own.
//! It decides which database machine is the primary, so that the standby
//! is promoted by itself when the primary is lost, and two machines never
//! both take writes. The database machines reach it as `autoctl_node`; the
//! ScannerPulse machine's key may run only its node commands.

use std::net::IpAddr;
use std::path::Path;

use super::node::{
    AUTOCTL_UNIT, autoctl, autoctl_folder, autoctl_service, drop_debians_cluster, hub_key_step,
    install,
};
use super::plan::Plan;
use super::steps::{Action, Step};
use super::system::{Family, System};

/// The marker of setup's lines in the referee's access rules.
const MARK: &str = "# scannerpulse: both database machines";

/// The monitor's data directory.
pub fn monitor_data(system: &System) -> String {
    format!("{}/17/monitor", system.postgres_home())
}

/// The access rules that let both database machines reach the monitor.
fn hba(nodes: &[IpAddr]) -> String {
    let mut text = format!("{MARK}\n");
    for ip in nodes {
        let bits = if ip.is_ipv4() { 32 } else { 128 };
        text.push_str(&format!(
            "hostssl pg_auto_failover autoctl_node {ip}/{bits} scram-sha-256{}\n",
            super::node::TAG
        ));
    }
    text
}

/// The steps that make this machine the referee of `plan`. `hub` and
/// `nodes` are the addresses the hub's and the database machines' names
/// give.
pub fn steps(
    plan: &Plan,
    system: &System,
    binary: &Path,
    hub: &[IpAddr],
    nodes: &[IpAddr],
) -> Vec<Step> {
    let mut steps = install(system, true);
    let data = monitor_data(system);
    let referee = plan.referee.as_deref().unwrap_or_default();
    let port = plan.referee_port().to_string();
    if system.family == Family::Apt {
        steps.push(drop_debians_cluster());
    }
    steps.push(super::steps::firewall(true, plan.referee_port()));
    steps.push(autoctl_folder("Make the referee's data directory", &data));
    steps.push(Step {
        title: "Make the referee (pg_auto_failover's monitor)".into(),
        action: Action::Run {
            program: "runuser".into(),
            args: autoctl(
                system,
                &[
                    "create",
                    "monitor",
                    "--pgdata",
                    &data,
                    "--pgport",
                    &port,
                    "--hostname",
                    referee,
                    "--auth",
                    "scram-sha-256",
                    "--ssl-self-signed",
                ],
            ),
            input: None,
            env: Vec::new(),
            unless: Some(AUTOCTL_UNIT.into()),
        },
    });
    // The ScannerPulse machine's own database is in the formation from the
    // start of a move until it is retired.
    let mut clients = nodes.to_vec();
    if plan.moving || plan.joined.contains(&plan.hub) {
        clients.extend(hub);
    }
    steps.push(super::node::script(
        "Let both database machines in",
        super::node::REPLACE_BLOCK,
        &[("MARK", MARK), ("HBA", &format!("{data}/pg_hba.conf"))],
        Some(hba(&clients)),
    ));
    steps.extend(autoctl_service(system, &data));
    steps.push(super::node::script(
        "Give the database machines' role its password",
        // The referee's own port: another PostgreSQL here may hold 5432.
        // And only to the monitor in this machine's own folder: what answers
        // on the port may be another PostgreSQL's (the plan named the wrong
        // port), whose role must keep its password.
        "for i in $(seq 120); do runuser -u postgres -- psql -p \"$PORT\" -d pg_auto_failover -tAXc 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done\ncanon() { readlink -f \"$1\" 2>/dev/null || printf '%s\\n' \"$1\"; }\nshown=$(runuser -u postgres -- psql -p \"$PORT\" -d pg_auto_failover -tAXc 'SHOW data_directory')\n[ \"$(canon \"$shown\")\" = \"$(canon \"$DATA\")\" ] || { echo \"port $PORT answers for the PostgreSQL in ${shown:-another folder}, not this machine's referee in $DATA: its password is left alone\" >&2; exit 1; }\nrunuser -u postgres -- psql -p \"$PORT\" -d pg_auto_failover -q -v ON_ERROR_STOP=1",
        &[("PORT", &port), ("DATA", &data)],
        Some(format!(
            "{}ALTER USER autoctl_node PASSWORD '{}';\nSELECT pg_reload_conf();\n",
            super::steps::QUIET_SQL,
            plan.monitor_password.replace('\'', "''")
        )),
    ));
    steps.push(hub_key_step(plan, hub, binary));
    steps
}

/// `pg_autoctl perform switchover` on this machine's monitor: the referee
/// makes the standby the primary, and the primary its standby.
pub fn switchover(system: &System) -> Result<String, String> {
    let data = monitor_data(system);
    let output = std::process::Command::new("runuser")
        .args(autoctl(
            system,
            &["perform", "switchover", "--pgdata", &data],
        ))
        .current_dir("/")
        .output()
        .map_err(|error| format!("cannot run pg_autoctl: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(format!("pg_autoctl: {}", super::apply::said(&output)))
    }
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
            referee: Some("ref.example.org".into()),
            referee_port: None,
            monitor_password: "referee-secret-3".into(),
            second: None,
            shared: None,
            install_id: String::new(),
            archive: String::new(),
            recordings: String::new(),
            backups: None,
            ready: false,
            joined: Vec::new(),
            moving: false,
            retired: false,
        }
    }

    #[test]
    fn the_referee_is_pg_auto_failovers_monitor_open_to_both_database_machines() {
        use crate::setup::system::detect;
        let rocky = detect("ID=\"rocky\"\nVERSION_ID=\"9.6\"\n", "x86_64").unwrap();
        let ips =
            |list: &[&str]| -> Vec<IpAddr> { list.iter().map(|s| s.parse().unwrap()).collect() };
        let all = steps(
            &plan(),
            &rocky,
            Path::new("/usr/local/bin/scannerpulse"),
            &ips(&["192.0.2.10"]),
            &ips(&["192.0.2.21", "2001:db8::22"]),
        );
        let titles: Vec<&str> = all.iter().map(|s| s.title.as_str()).collect();
        let at = |title: &str| {
            titles
                .iter()
                .position(|t| *t == title)
                .unwrap_or_else(|| panic!("{title}: {titles:?}"))
        };
        let Action::Run { args, .. } =
            &all[at("Install PostgreSQL 17, pg_auto_failover and SSH")].action
        else {
            panic!()
        };
        assert!(args.contains(&"pg_auto_failover_17".to_string()));
        let make = "Make the referee (pg_auto_failover's monitor)";
        let Action::Run {
            program,
            args,
            unless,
            ..
        } = &all[at(make)].action
        else {
            panic!()
        };
        assert_eq!(program, "runuser");
        let line = args.join(" ");
        assert!(line.contains("PATH=/usr/pgsql-17/bin:/usr/bin:/bin pg_autoctl create monitor --pgdata /var/lib/pgsql/17/monitor --pgport 5432 --hostname ref.example.org --auth scram-sha-256 --ssl-self-signed"), "{line}");
        assert!(
            !line.contains("--no-run"),
            "pg_autoctl 2.2 has no --no-run: {line}"
        );
        assert_eq!(
            unless.as_deref(),
            Some(Path::new("/etc/systemd/system/pgautofailover.service"))
        );
        let Action::Run { args, .. } = &all[at("Make the referee's data directory")].action else {
            panic!()
        };
        assert!(
            args[1].contains("install -d -m 0755 -o postgres -g postgres"),
            "{}",
            args[1]
        );
        let Action::Run { input, .. } = &all[at("Let both database machines in")].action else {
            panic!()
        };
        assert_eq!(
            input.as_deref(),
            Some(
                "# scannerpulse: both database machines\nhostssl pg_auto_failover autoctl_node 192.0.2.21/32 scram-sha-256 # scannerpulse\nhostssl pg_auto_failover autoctl_node 2001:db8::22/128 scram-sha-256 # scannerpulse\n"
            )
        );
        assert!(at(make) < at("Let both database machines in"));
        assert!(at("Let both database machines in") < at("Start pg_autoctl, now and at boot"));
        // Its role's password goes on standard input, after the service
        // answers, and never into PostgreSQL's log.
        let password = "Give the database machines' role its password";
        assert!(at("Start pg_autoctl, now and at boot") < at(password));
        let Action::Run { args, input, .. } = &all[at(password)].action else {
            panic!()
        };
        assert!(!args.join(" ").contains("secret"));
        assert!(input.as_deref().unwrap().starts_with("SET log_statement = none;\nSET log_min_error_statement = panic;\nALTER USER autoctl_node PASSWORD 'referee-secret-3';"));
        assert!(titles.contains(&"Open port 5432 in the firewall, if firewalld runs"));
        assert!(
            titles.contains(&"Let the ScannerPulse machine run this machine's database commands")
        );
        // No database of the install's on the referee.
        assert!(!titles.iter().any(|t| t.contains("nightly dump")));
    }

    #[test]
    fn a_referee_beside_another_postgresql_uses_its_own_port_throughout() {
        use crate::setup::system::detect;
        let ubuntu = detect("ID=ubuntu\nVERSION_ID=\"24.04\"\n", "aarch64").unwrap();
        let plan = Plan {
            referee_port: Some(5433),
            ..plan()
        };
        let ip: Vec<IpAddr> = vec!["192.0.2.21".parse().unwrap()];
        let all = steps(
            &plan,
            &ubuntu,
            Path::new("/usr/local/bin/scannerpulse"),
            &ip,
            &ip,
        );
        let step = |title: &str| {
            all.iter()
                .find(|s| s.title == title)
                .unwrap_or_else(|| panic!("{title}"))
        };
        let Action::Run { args, .. } =
            &step("Make the referee (pg_auto_failover's monitor)").action
        else {
            panic!()
        };
        assert!(args.join(" ").contains("--pgport 5433 "), "{args:?}");
        step("Open port 5433 in the firewall, if firewalld runs");
        // Every psql of the referee's names its port: a bare one would set
        // the role's password in the other PostgreSQL, on 5432.
        let Action::Run { args, env, .. } =
            &step("Give the database machines' role its password").action
        else {
            panic!()
        };
        let script = &args[1];
        assert_eq!(
            script.matches("psql").count(),
            script.matches("psql -p \"$PORT\"").count(),
            "{script}"
        );
        assert!(env.contains(&("PORT".into(), "5433".into())), "{env:?}");
        // The other PostgreSQL's packages keep their versions.
        for title in [
            "Install the PostgreSQL project's repository tool",
            "Install PostgreSQL 17, pg_auto_failover and SSH",
        ] {
            let Action::Run { args, .. } = &step(title).action else {
                panic!()
            };
            assert!(
                args.contains(&"--no-upgrade".to_string()),
                "{title}: {args:?}"
            );
        }
    }

    #[test]
    fn the_password_goes_only_to_the_monitor_this_machine_made() {
        use crate::setup::system::detect;
        let ubuntu = detect("ID=ubuntu\nVERSION_ID=\"24.04\"\n", "aarch64").unwrap();
        let ip: Vec<IpAddr> = vec!["192.0.2.21".parse().unwrap()];
        let all = steps(
            &plan(),
            &ubuntu,
            Path::new("/usr/local/bin/scannerpulse"),
            &ip,
            &ip,
        );
        let step = all
            .iter()
            .find(|s| s.title == "Give the database machines' role its password")
            .unwrap();
        // The port answers, but for another PostgreSQL (another site's
        // referee on 5432, say): its role keeps its password.
        let stub = "case \"$*\" in *data_directory*) echo \"$SHOWN\" ;; esac";
        for (shown, ours) in [
            ("/var/lib/postgresql/monitor", false),
            ("/var/lib/postgresql/17/monitor", true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let ran = crate::setup::test_script::run(
                step,
                &[("runuser", stub)],
                &[("SHOWN", shown)],
                dir.path(),
                "ALTER USER autoctl_node PASSWORD 'x';\n",
            );
            assert_eq!(ran.ok, ours, "{shown}: {}", ran.stderr);
            assert_eq!(ran.calls.contains("ON_ERROR_STOP=1"), ours, "{}", ran.calls);
            if !ours {
                assert!(
                    ran.stderr.contains("/var/lib/postgresql/17/monitor"),
                    "{}",
                    ran.stderr
                );
            }
        }
    }

    #[test]
    fn while_the_database_moves_the_scannerpulse_machine_reaches_the_referee_too() {
        use crate::setup::system::detect;
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let ips =
            |list: &[&str]| -> Vec<IpAddr> { list.iter().map(|s| s.parse().unwrap()).collect() };
        let rules = |plan: &Plan| {
            let all = steps(
                plan,
                &debian,
                Path::new("/usr/local/bin/scannerpulse"),
                &ips(&["192.0.2.10"]),
                &ips(&["192.0.2.21", "192.0.2.22"]),
            );
            match &all
                .iter()
                .find(|s| s.title == "Let both database machines in")
                .unwrap()
                .action
            {
                Action::Run { input, .. } => input.clone().unwrap_or_default(),
                _ => panic!(),
            }
        };
        let hub_line = "hostssl pg_auto_failover autoctl_node 192.0.2.10/32";
        assert!(!rules(&plan()).contains(hub_line));
        // Its own database is in the referee's formation from the move until
        // it is retired.
        let mut moving = Plan {
            moving: true,
            retired: false,
            ..plan()
        };
        assert!(rules(&moving).contains(hub_line));
        moving.moving = false;
        moving.mark_joined("192.0.2.10");
        assert!(rules(&moving).contains(hub_line));
    }
}
