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
    if system.family == Family::Apt {
        steps.push(drop_debians_cluster());
    }
    steps.push(super::steps::firewall(true, 5432));
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
                    "5432",
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
    steps.push(super::node::script(
        "Let both database machines in",
        super::node::REPLACE_BLOCK,
        &[("MARK", MARK), ("HBA", &format!("{data}/pg_hba.conf"))],
        Some(hba(nodes)),
    ));
    steps.extend(autoctl_service(system, &data));
    steps.push(super::node::script(
        "Give the database machines' role its password",
        "for i in $(seq 120); do runuser -u postgres -- psql -d pg_auto_failover -tAXc 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done\nrunuser -u postgres -- psql -d pg_auto_failover -q -v ON_ERROR_STOP=1",
        &[],
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
            monitor_password: "referee-secret-3".into(),
            joined: Vec::new(),
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
}
