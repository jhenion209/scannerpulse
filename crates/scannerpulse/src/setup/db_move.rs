//! `scannerpulse db move`: the database of an install on one machine,
//! setup's own PostgreSQL there, moved to two database machines of its own
//! (5E2), the standby promoted by hand. The first database machine joins as
//! a copy of this machine's database and follows it; `db move --switch`
//! stops this machine's PostgreSQL cleanly and makes that copy the primary,
//! with writes paused for the seconds it takes; the second joins after it;
//! and `db move --retire` removes the old database once the person agrees.

use std::net::IpAddr;
use std::path::Path;

use super::node::{self, REPLACE_BLOCK};
use super::plan::Plan;
use super::steps::{ACCOUNT, Action, DATABASE, PORT_PG, Step, run};
use super::system::{Family, System};

/// The marker of setup's block in this machine's access rules.
const MARK: &str = "# scannerpulse: the database machines, which copy this one";

/// Whether the settings are those of an install whose database is setup's
/// own PostgreSQL on this machine: the only database that moves.
pub fn database_here(settings: &str) -> Result<(), String> {
    let pairs = crate::hostfile::parse(settings)?;
    let value = |name: &str| {
        pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    if value(crate::primary::HOSTS).is_some() {
        return Err("this install's database is on database machines already".into());
    }
    let local = value("DATABASE_URL")
        .and_then(|url| reqwest::Url::parse(url).ok())
        .is_some_and(|url| url.host_str() == Some("localhost"));
    if !local || value("SCANNERPULSE_BACKUPS").is_none() {
        return Err("this install's database is not setup's own PostgreSQL on this machine: a database you run moves with your own tools".into());
    }
    Ok(())
}

/// The site's password, from its password file: the password of the line
/// for the database and its role, as setup writes it (the rest of the line).
pub fn app_password(pgpass: &str) -> Option<String> {
    pgpass.lines().find_map(|line| {
        // The first four fields, a `\` escaping the character after it.
        let mut fields = Vec::new();
        let (mut field, mut escaped, mut rest) = (String::new(), false, None);
        for (at, c) in line.char_indices() {
            match c {
                _ if escaped => {
                    field.push(c);
                    escaped = false;
                }
                '\\' => escaped = true,
                ':' => {
                    fields.push(std::mem::take(&mut field));
                    if fields.len() == 4 {
                        rest = Some(&line[at + 1..]);
                        break;
                    }
                }
                _ => field.push(c),
            }
        }
        (fields.len() == 4 && fields[2] == DATABASE && fields[3] == ACCOUNT)
            .then(|| rest.unwrap_or_default().to_string())
    })
}

/// The site's password file once the database is on the database machines.
pub fn moved_pgpass(password: &str) -> String {
    super::steps::Remote {
        port: PORT_PG,
        name: DATABASE.into(),
        role: ACCOUNT.into(),
        password: password.to_string(),
        ..Default::default()
    }
    .pgpass_line()
}

/// The settings once the database is on `nodes`, the first the primary, as
/// setup writes them for database machines: the backups are theirs.
pub fn moved_settings(text: &str, nodes: &[String]) -> Result<String, String> {
    let pair = super::steps::Pair {
        nodes: [nodes[0].clone(), nodes[1].clone()],
        hub: String::new(),
        referee: None,
        second: None,
        shared: None,
    };
    let mut text = text.to_string();
    for (name, value) in [
        ("DATABASE_URL", Some(pair.url())),
        (crate::primary::HOSTS, Some(pair.hosts())),
        ("SCANNERPULSE_BACKUPS", None),
    ] {
        text = crate::hostfile::changed(&text, name, value.as_deref())?;
    }
    Ok(text)
}

/// The steps that let the database machines (`nodes`, their addresses)
/// copy this machine's PostgreSQL: TLS, the network, what a later rewind
/// needs, the copying role, and the access rules; then a restart.
pub fn hub_steps(plan: &Plan, system: &System, binary: &Path, nodes: &[IpAddr]) -> Vec<Step> {
    let mut steps = Vec::new();
    let data = system.postgres_data();
    let mut sql = String::from(
        "ALTER SYSTEM SET listen_addresses = '*';\nALTER SYSTEM SET wal_log_hints = on;\n",
    );
    if system.family == Family::Dnf {
        // Debian's packages give PostgreSQL a certificate; these do not.
        steps.push(node::script(
            "Give PostgreSQL a certificate, for TLS",
            "[ -e \"$DATA/server.key\" ] || { \"$BINARY\" cert make --cert \"$DATA/server.crt\" --key \"$DATA/server.key\" \"$NAME\" && chown postgres:postgres \"$DATA/server.crt\" \"$DATA/server.key\"; }",
            &[
                ("DATA", data),
                ("BINARY", &binary.display().to_string()),
                ("NAME", &plan.hub),
            ],
            None,
        ));
        sql.push_str("ALTER SYSTEM SET ssl = on;\n");
    }
    sql.push_str(&node::replicator_sql(&plan.replication_password));
    steps.push(node::script(
        "Let PostgreSQL take the database machines' connections, and make the copying role",
        "runuser -u postgres -- psql -q -v ON_ERROR_STOP=1",
        &[],
        Some(sql),
    ));
    let rules = node::hba(plan, &[], nodes).replacen(node::MARK, MARK, 1);
    steps.push(node::script(
        "Let the database machines copy this machine's database",
        REPLACE_BLOCK,
        &[("MARK", MARK), ("HBA", system.postgres_hba())],
        Some(rules),
    ));
    steps.push(super::steps::firewall(true, PORT_PG));
    // The access rules need a reload only; the network and the rewind's
    // setting a restart, once.
    steps.push(node::script(
        "Restart PostgreSQL 17 if its new settings need it (writes pause for a few seconds)",
        "runuser -u postgres -- psql -qtAXc 'SELECT pg_reload_conf()' >/dev/null && sleep 1\nn=$(runuser -u postgres -- psql -tAXc 'SELECT count(*) FROM pg_settings WHERE pending_restart') || exit 1\n[ \"$n\" = 0 ] || systemctl restart \"$SERVICE\"",
        &[("SERVICE", system.postgres_cluster())],
        None,
    ));
    steps
}

/// The switch itself: this machine's PostgreSQL stops cleanly (`stop_here`
/// gives where its last checkpoint starts) and is kept from starting at
/// boot (`boot_here(false)`), so a reboot after the promotion never brings
/// it back; then `node0`, its copy, is made the primary once it has
/// replayed past that point. When a step fails, one primary is left and
/// the person is told what to run next.
fn switch_roles(
    node0: &str,
    stop_here: impl FnOnce() -> Result<String, String>,
    boot_here: impl Fn(bool) -> Result<(), String>,
    start_here: impl Fn() -> Result<(), String>,
    ask: impl Fn(&str, &str) -> Result<String, String>,
    say: impl Fn(&str),
) -> Result<(), String> {
    let start_again = |why: String| match start_here() {
        Ok(()) => format!(
            "{why}; this machine's PostgreSQL was started again and still holds the database: nothing moved"
        ),
        Err(error) => format!(
            "{why}; and this machine's PostgreSQL did not start again ({error}): start it, and nothing moved"
        ),
    };
    say("Stopping this machine's PostgreSQL");
    let last = match stop_here() {
        Ok(last) => last,
        Err(error) => return Err(start_again(format!("it did not stop: {error}"))),
    };
    if let Err(error) = boot_here(false) {
        return Err(start_again(format!(
            "it could not be kept from starting at boot: {error}"
        )));
    }
    say(&format!("Making {node0} the primary"));
    if let Err(error) = ask(node0, &format!("promote after {last}")) {
        // Started again only while the copy is still a standby: one whose
        // promotion went ahead would make two primaries.
        let now = node::as_status(ask(node0, "status"));
        if now.answers && now.standby {
            let why = format!("{node0} was not made the primary: {error}");
            return Err(match boot_here(true) {
                Ok(()) => start_again(why),
                Err(again) => start_again(format!(
                    "{why}; this machine's PostgreSQL is still kept from starting at boot ({again})"
                )),
            });
        }
        return Err(format!(
            "{node0} may be the primary now ({error}), so this machine's PostgreSQL was left stopped: once {node0} answers as the primary, run `sudo /usr/local/bin/scannerpulse db move --switch` again to finish"
        ));
    }
    Ok(())
}

/// Debian's file that says whether PostgreSQL 17's cluster starts at boot.
const START_CONF: &str = "/etc/postgresql/17/main/start.conf";

/// Lets this machine's old database start at boot (`start`), or keeps it
/// from starting; on Debian through `start_conf`.
pub fn boot(system: &System, start: bool, start_conf: &Path) -> Result<(), String> {
    match system.family {
        Family::Apt => std::fs::write(start_conf, if start { "auto\n" } else { "manual\n" })
            .map_err(|error| format!("{}: {error}", start_conf.display())),
        Family::Dnf => systemctl(&[
            if start { "enable" } else { "disable" },
            system.postgres_cluster(),
        ]),
    }
}

/// The units that read the settings and are not the site's: restarted
/// after the switch, so that they reach the database machines too.
const RESTARTED: [&str; 1] = ["scannerpulse-agent.service"];

/// The two database machines and this machine (`hub`) are each a machine
/// of their own, by name or by the addresses their names give, and each
/// name is a plain one.
fn machines_apart(
    nodes: &[String; 2],
    hub: &str,
    resolve: impl Fn(&str) -> Vec<IpAddr>,
) -> Result<(), String> {
    for name in nodes.iter().map(String::as_str).chain([hub]) {
        if !super::join::plain(name) {
            return Err(format!(
                "{name:?} is not a name or an address: letters, digits, dots, dashes and colons"
            ));
        }
    }
    super::ask::distinct_machines(
        &[
            ("database machine 1", &nodes[0]),
            ("database machine 2", &nodes[1]),
            ("this machine", hub),
        ],
        resolve,
    )
}

/// The old database's backups stopped: the site no longer uses it.
/// pgBackRest's settings for it go too, as `uninstall` would reach for it
/// through them.
pub fn hold_steps() -> Vec<Step> {
    vec![
        run(
            "Stop the nightly backups of this machine's old database",
            "systemctl",
            &["disable", "--now", "scannerpulse-backup.timer"],
        ),
        run(
            "Remove pgBackRest's settings for it",
            "rm",
            &["-f", crate::backup::PGBACKREST_CONF],
        ),
    ]
}

/// The old database removed.
pub fn retire_steps(system: &System) -> Vec<Step> {
    vec![match system.family {
        Family::Apt => node::script(
            "Remove the old database (PostgreSQL 17's cluster 17 main)",
            "[ -d /var/lib/postgresql/17/main ] || exit 0\npg_dropcluster --stop 17 main",
            &[],
            None,
        ),
        Family::Dnf => node::script(
            "Remove the old database",
            "systemctl disable --now \"$SERVICE\" && rm -r -f \"$DATA\"",
            &[
                ("SERVICE", system.postgres_cluster()),
                ("DATA", system.postgres_data()),
            ],
            None,
        ),
    }]
}

fn read_settings() -> Result<String, String> {
    std::fs::read_to_string(super::steps::env_path())
        .map_err(|_| "this machine runs no ScannerPulse site".to_string())
}

fn this_system() -> Result<System, String> {
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    super::system::detect(&os_release, std::env::consts::ARCH)
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|error| format!("cannot run systemctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "systemctl {}: {}",
            args.join(" "),
            super::apply::said(&output)
        ))
    }
}

fn context() -> super::apply::Context {
    super::apply::Context {
        database_url: String::new(),
        db_password: String::new(),
        nodes: Vec::new(),
    }
}

/// `scannerpulse db move NODE1 NODE2 --hub ADDRESS`, as root on the
/// ScannerPulse machine: the plan of the two database machines (`hub` is
/// this machine's address as they reach it), and this machine's PostgreSQL
/// opened to them. Run again, it says how far the first one's copy is.
pub async fn begin(
    conn: &mut sqlx::PgConnection,
    nodes: [String; 2],
    hub: String,
) -> Result<String, String> {
    let settings = read_settings()?;
    let path = Path::new(super::plan::PLAN);
    if super::second::is_second(&settings, path.exists()) {
        return Err("run this on the install's first ScannerPulse machine".into());
    }
    database_here(&settings)?;
    machines_apart(&nodes, &hub, super::plan::resolve)?;
    if let Ok(kept) = Plan::read(path)
        && !(kept.moving && kept.nodes == nodes)
    {
        return Err(if kept.moving {
            format!(
                "the database is moving to {} already",
                kept.nodes.join(" and ")
            )
        } else {
            "this install has a plan of database machines already".into()
        });
    }
    let mut node_ips = Vec::new();
    for node in &nodes {
        let found = super::plan::resolve(node);
        if found.is_empty() {
            return Err(format!("{node} does not resolve to an address here"));
        }
        node_ips.extend(found);
    }
    let pgpass = std::fs::read_to_string(super::steps::pgpass_path())
        .map_err(|error| format!("{}: {error}", super::steps::pgpass_path().display()))?;
    let password =
        app_password(&pgpass).ok_or("the site's password file has no line for its database")?;
    let archive = super::archive_move::current(conn)
        .await
        .map_err(|error| error.to_string())?
        .ok_or("the database names no archive")?;
    let recordings: Option<String> =
        sqlx::query_scalar("SELECT path FROM sources ORDER BY id LIMIT 1")
            .fetch_optional(&mut *conn)
            .await
            .map_err(|error| error.to_string())?;
    let key = Path::new(super::plan::SSH).join("id_ed25519");
    if !key.exists() {
        let made = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "install -d -m 0700 \"$1\" && ssh-keygen -q -t ed25519 -N '' -C scannerpulse-hub -f \"$1/id_ed25519\"",
                "keygen",
                super::plan::SSH,
            ])
            .output()
            .map_err(|error| format!("cannot run ssh-keygen: {error}"))?;
        if !made.status.success() {
            return Err(format!("this machine's key: {}", super::apply::said(&made)));
        }
    }
    super::apply::write_plan(Plan {
        hub: hub.clone(),
        hub_key: String::new(),
        nodes: nodes.to_vec(),
        database: DATABASE.into(),
        role: ACCOUNT.into(),
        password,
        replication_password: String::new(),
        referee: None,
        monitor_password: String::new(),
        second: None,
        shared: None,
        install_id: String::new(),
        archive: archive.display().to_string(),
        recordings: recordings.unwrap_or_default(),
        ready: false,
        joined: Vec::new(),
        moving: true,
    })?;
    let plan = Plan::read(path)?;
    let steps = hub_steps(&plan, &this_system()?, Path::new(super::BINARY), &node_ips);
    super::apply::apply(&steps, &context(), true).await?;
    let [first, second] = &nodes;
    let progress = if plan.joined.contains(first) {
        let here = node::status();
        if here.streaming > 0 {
            format!(
                "{first} follows this database, {} KiB behind: run `sudo /usr/local/bin/scannerpulse db move --switch` when you are ready (writes pause for a few seconds).",
                here.behind / 1024
            )
        } else {
            format!(
                "{first} has joined but does not follow this database now: see its PostgreSQL's log."
            )
        }
    } else {
        format!(
            "On {first}: run the install line, then `sudo /usr/local/bin/scannerpulse setup`, choose Advanced, then \"join an existing install\", and give this machine's address ({hub}). It copies this database and follows it. Run this command again to see how far it has come."
        )
    };
    Ok(format!(
        "The database moves to {first} (the primary) and {second} (its standby).\n{progress}\n{second} joins the same way after the switch."
    ))
}

/// Whether the site signs in with `settings` (its database's address) and
/// `password`, and its database answers.
async fn signs_in(settings: &str, password: &str) -> Result<(), String> {
    use sqlx::Connection;
    let url = crate::hostfile::parse(settings)?
        .into_iter()
        .find(|(name, _)| name == "DATABASE_URL")
        .map(|(_, url)| url)
        .ok_or("no DATABASE_URL")?;
    let options: sqlx::postgres::PgConnectOptions = url
        .parse()
        .map_err(|error: sqlx::Error| error.to_string())?;
    let ask = async {
        let mut conn = sqlx::PgConnection::connect_with(&options.password(password)).await?;
        sqlx::query("SELECT 1").execute(&mut conn).await?;
        conn.close().await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), ask).await {
        Err(_) => Err("no answer within 10 s".into()),
        Ok(answer) => answer.map_err(|error| error.to_string()),
    }
}

/// Starts the site's services when dropped, so that a switch that ends
/// early, by an error or a panic, does not leave the site stopped.
struct Site;

impl Drop for Site {
    fn drop(&mut self) {
        let _ = systemctl(&[
            "start",
            "scannerpulse-web.service",
            "scannerpulse-ingest.service",
        ]);
    }
}

/// `scannerpulse db move --switch`, as root on the ScannerPulse machine:
/// the first database machine, caught up, becomes the primary, and the
/// site moves to it. Writes pause for the seconds it takes; nothing is
/// lost, since this machine's PostgreSQL stops cleanly and the copy has
/// everything first.
pub async fn switch(say: impl Fn(&str)) -> Result<String, String> {
    let plan = Plan::read(Path::new(super::plan::PLAN))?;
    let settings = read_settings()?;
    if !plan.moving {
        return Err(if database_here(&settings).is_err() {
            "the database has moved already".into()
        } else {
            "no move of the database is under way: `scannerpulse db move NODE1 NODE2 --hub ADDRESS` starts one".into()
        });
    }
    let node0 = plan.nodes[0].clone();
    if !plan.joined.contains(&node0) {
        return Err(format!(
            "{node0} has not joined yet: join it first (`scannerpulse db move NODE1 NODE2 --hub ADDRESS` says how)"
        ));
    }
    let system = this_system()?;
    let _lock = crate::update::exclusive(Path::new(crate::update::LOCK))?;
    let here = node::status();
    let there = node::as_status(node::ask(&node0, "status", &[]));
    // A switch cut short after the promotion: this machine's PostgreSQL is
    // stopped and the copy is the primary; only the rest is left.
    let finishing = !here.answers && there.answers && !there.standby;
    if !finishing {
        if !here.answers || here.standby {
            return Err("this machine's PostgreSQL must answer as the primary".into());
        }
        if !there.answers || !there.standby {
            return Err(format!("{node0} must answer as this database's standby"));
        }
        if here.streaming == 0 {
            return Err(format!("{node0} is not following this database now"));
        }
        if here.behind > node::SWITCH_BEHIND {
            return Err(format!(
                "{node0} is {} MiB behind: try again when it has caught up",
                here.behind / (1024 * 1024)
            ));
        }
    }
    // The site must be able to sign in to the copy as its role, over TLS,
    // before anything stops: its access rules, its certificate and the
    // network all come into it.
    let pgpass = std::fs::read_to_string(super::steps::pgpass_path())
        .map_err(|error| format!("{}: {error}", super::steps::pgpass_path().display()))?;
    let password =
        app_password(&pgpass).ok_or("the site's password file has no line for its database")?;
    let moved = moved_settings(&settings, &plan.nodes)?;
    signs_in(&moved, &password).await.map_err(|error| {
        format!(
            "the site cannot sign in to {node0} ({error}), so nothing was changed: join {node0} again, then try again"
        )
    })?;
    // Ctrl-C, a closed terminal or a stop wait for the end of the switch,
    // and the guard starts the site again however it ends.
    use tokio::signal::unix::{SignalKind, signal};
    let _held = [
        SignalKind::interrupt(),
        SignalKind::hangup(),
        SignalKind::terminate(),
    ]
    .into_iter()
    .map(signal)
    .collect::<std::io::Result<Vec<_>>>()
    .map_err(|error| format!("cannot hold off signals: {error}"))?;
    let _site = Site;
    say("Pausing the site: writes stop for the seconds the switch takes");
    systemctl(&[
        "stop",
        "scannerpulse-web.service",
        "scannerpulse-ingest.service",
    ])?;
    let start_conf = Path::new(START_CONF);
    if finishing {
        boot(&system, false, start_conf)?;
    } else {
        switch_roles(
            &node0,
            node::stop,
            |start| boot(&system, start, start_conf),
            node::start,
            |node, command| node::ask(node, command, &[]),
            &say,
        )?;
    }
    // From here the database is on the copy: a step that fails is finished
    // by the same command again.
    let finish = |error: String| {
        format!(
            "{error}; the database is on {node0} now: run `sudo /usr/local/bin/scannerpulse db move --switch` again to finish"
        )
    };
    say("Pointing the site at the database machines");
    let backups = crate::hostfile::parse(&settings)?
        .into_iter()
        .find(|(name, _)| name == "SCANNERPULSE_BACKUPS")
        .map(|(_, value)| value);
    let mut steps = vec![
        super::change::settings_with(&moved, &[]).map_err(finish)?,
        Step {
            title: "Write the database's password file".into(),
            action: Action::Write {
                path: super::steps::pgpass_path(),
                contents: moved_pgpass(&password),
                mode: 0o600,
                owner: format!("{ACCOUNT}:{ACCOUNT}"),
            },
        },
    ];
    steps.extend(hold_steps());
    super::apply::apply(&steps, &context(), true)
        .await
        .map_err(finish)?;
    Plan::update(Path::new(super::plan::PLAN), |plan| plan.moving = false).map_err(finish)?;
    say("Starting the site again");
    systemctl(&[
        "start",
        "scannerpulse-web.service",
        "scannerpulse-ingest.service",
    ])?;
    let mut restart = vec!["try-restart"];
    restart.extend(RESTARTED);
    systemctl(&restart)?;
    let kept = backups
        .map(|folder| format!(", and its backups stay in {folder}"))
        .unwrap_or_default();
    Ok(format!(
        "The database is on {node0} now, and the site uses it. Next, {second} joins the same way {node0} did, as its standby. This machine's old database is stopped and kept as it was{kept}. Once you are satisfied, `sudo /usr/local/bin/scannerpulse db move --retire` removes it.",
        second = plan.nodes[1]
    ))
}

/// `scannerpulse db move --retire`, as root on the ScannerPulse machine,
/// after the switch: the old database removed, once a database machine
/// answers as the primary and the person agrees.
pub async fn retire(confirm: impl Fn(&str) -> bool) -> Result<String, String> {
    let plan = Plan::read(Path::new(super::plan::PLAN))?;
    if plan.moving || database_here(&read_settings()?).is_ok() {
        return Err("the site still uses this machine's database: switch first (`sudo /usr/local/bin/scannerpulse db move --switch`)".into());
    }
    let system = this_system()?;
    if !Path::new(system.postgres_data()).exists() {
        return Ok("This machine has no old database left.".into());
    }
    let primary = plan
        .nodes
        .iter()
        .find(|node| {
            let status = node::as_status(node::ask(node, "status", &[]));
            status.answers && !status.standby
        })
        .ok_or("no database machine answers as the primary now, so nothing was removed")?;
    if !confirm(&format!(
        "{primary} holds the site's database. Remove this machine's old one ({}) for good?",
        system.postgres_data()
    )) {
        return Ok("Nothing was removed.".into());
    }
    super::apply::apply(&retire_steps(&system), &context(), true).await?;
    Ok("This machine's old database is removed. Its old backups stay in the backups' folder until you remove them.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::system::detect;

    const HERE: &str = "SCANNERPULSE_HOST=\"radio-a\"\nDATABASE_URL=\"postgres://scannerpulse@localhost:5432/scannerpulse\"\nSCANNERPULSE_BACKUPS=\"/srv/archive/.scannerpulse-backups\"\n";

    fn plan() -> Plan {
        serde_json::from_str(r#"{"hub":"192.0.2.10","hub_key":"ssh-ed25519 AAAAhub","nodes":["db1.example.org","2001:db8::22"],"database":"scannerpulse","role":"scannerpulse","password":"app-secret-1","replication_password":"copy-secret-2","moving":true}"#).unwrap()
    }

    #[test]
    fn only_setups_own_database_here_moves() {
        assert_eq!(database_here(HERE), Ok(()));
        let nodes = format!("{HERE}SCANNERPULSE_DATABASE_HOSTS=\"db1:5432 db2:5432\"\n");
        let error = database_here(&nodes).unwrap_err();
        assert!(error.contains("database machines already"), "{error}");
        let theirs = "DATABASE_URL=\"postgres://scannerpulse@db.example.org:5432/scannerpulse?sslmode=require\"\n";
        let error = database_here(theirs).unwrap_err();
        assert!(error.contains("not setup's own"), "{error}");
    }

    #[test]
    fn the_sites_password_comes_from_its_password_file() {
        assert_eq!(
            app_password("localhost:5432:scannerpulse:scannerpulse:s3cret:with\\colon\n")
                .as_deref(),
            Some("s3cret:with\\colon")
        );
        assert_eq!(app_password("localhost:5432:other:someone:x\n"), None);
        assert_eq!(
            moved_pgpass("s3cret"),
            "*:5432:scannerpulse:scannerpulse:s3cret\n"
        );
    }

    #[test]
    fn after_the_switch_the_settings_name_both_database_machines() {
        let moved = moved_settings(HERE, &plan().nodes).unwrap();
        assert!(moved.contains("DATABASE_URL=\"postgres://scannerpulse@db1.example.org:5432/scannerpulse?sslmode=require\""), "{moved}");
        assert!(
            moved.contains(
                "SCANNERPULSE_DATABASE_HOSTS=\"db1.example.org:5432 [2001:db8::22]:5432\""
            ),
            "{moved}"
        );
        assert!(!moved.contains("SCANNERPULSE_BACKUPS"), "{moved}");
        assert!(moved.contains("SCANNERPULSE_HOST=\"radio-a\""), "{moved}");
    }

    #[test]
    fn this_machine_lets_the_database_machines_copy_its_database() {
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let nodes: Vec<IpAddr> = ["192.0.2.21", "2001:db8::22"]
            .iter()
            .map(|ip| ip.parse().unwrap())
            .collect();
        let all = hub_steps(&plan(), &debian, Path::new("/b"), &nodes);
        let input = |title: &str| match &all
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("{title}"))
            .action
        {
            Action::Run { input, .. } => input.clone().unwrap_or_default(),
            _ => panic!("{title}"),
        };
        let sql = input(
            "Let PostgreSQL take the database machines' connections, and make the copying role",
        );
        assert!(sql.contains("listen_addresses = '*'"), "{sql}");
        assert!(sql.contains("wal_log_hints = on"), "{sql}");
        assert!(sql.contains("CREATE ROLE replicator"), "{sql}");
        assert!(sql.contains("copy-secret-2"), "{sql}");
        let rules = input("Let the database machines copy this machine's database");
        assert!(
            rules.contains("hostssl replication replicator 192.0.2.21/32 scram-sha-256"),
            "{rules}"
        );
        assert!(
            rules.contains("hostssl replication replicator 2001:db8::22/128"),
            "{rules}"
        );
        // This machine's own site keeps its way in.
        assert!(
            !rules.contains("hostssl scannerpulse scannerpulse"),
            "{rules}"
        );
        // Run again (to see how far the copy is), nothing restarts: only a
        // setting PostgreSQL reads at its start does.
        let last = all.last().unwrap();
        assert_eq!(
            last.title,
            "Restart PostgreSQL 17 if its new settings need it (writes pause for a few seconds)"
        );
        let Action::Run { args, .. } = &last.action else {
            panic!()
        };
        assert!(args[1].contains("pending_restart"), "{}", args[1]);
    }

    /// The switch with `answer` for the first database machine's commands,
    /// and what was asked, this machine's own steps included.
    fn switched(
        stop: Result<&str, &str>,
        answer: impl Fn(&str) -> Result<String, String>,
    ) -> (Result<(), String>, Vec<String>) {
        let asked = std::cell::RefCell::new(Vec::new());
        let result = switch_roles(
            "db1.example.org",
            || {
                asked.borrow_mut().push("stop here".into());
                stop.map(String::from).map_err(String::from)
            },
            |start| {
                asked
                    .borrow_mut()
                    .push(if start { "release here" } else { "hold here" }.into());
                Ok(())
            },
            || {
                asked.borrow_mut().push("start here".into());
                Ok(())
            },
            |node, command| {
                asked.borrow_mut().push(format!("{node} {command}"));
                answer(command)
            },
            |_| {},
        );
        (result, asked.into_inner())
    }

    const STANDBY: &str = r#"{"answers":true,"standby":true,"streaming":0,"behind":0}"#;
    const PRIMARY: &str = r#"{"answers":true,"standby":false,"streaming":0,"behind":0}"#;

    #[test]
    fn the_switch_stops_this_machine_and_promotes_its_copy() {
        let (result, asked) = switched(Ok("0/313CE18"), |_| Ok("promoted".into()));
        assert_eq!(result, Ok(()));
        // Held from starting at boot before the promotion: a reboot after
        // it never brings the old primary back.
        assert_eq!(
            asked,
            [
                "stop here",
                "hold here",
                "db1.example.org promote after 0/313CE18"
            ]
        );
    }

    #[test]
    fn a_switch_that_fails_leaves_one_primary() {
        // This machine did not stop cleanly: it is started again.
        let (result, asked) = switched(Err("did not stop cleanly"), |_| Ok(String::new()));
        let error = result.unwrap_err();
        assert!(error.contains("nothing moved"), "{error}");
        assert_eq!(asked, ["stop here", "start here"]);
        // The copy refused and is still a standby: this machine is started
        // again.
        let (result, asked) = switched(Ok("0/3"), |command| match command {
            "status" => Ok(STANDBY.into()),
            _ => Err("not everything received".into()),
        });
        let error = result.unwrap_err();
        assert!(error.contains("nothing moved"), "{error}");
        assert_eq!(asked[asked.len() - 2..], ["release here", "start here"]);
        // The answer was lost but the promotion went ahead: this machine
        // stays stopped, so there are never two primaries.
        let (result, asked) = switched(Ok("0/3"), |command| match command {
            "status" => Ok(PRIMARY.into()),
            _ => Err("the connection was lost".into()),
        });
        let error = result.unwrap_err();
        assert!(!asked.contains(&"start here".to_string()));
        assert!(!asked.contains(&"release here".to_string()));
        assert!(error.contains("left stopped"), "{error}");
        assert!(error.contains("db move --switch"), "{error}");
    }

    #[test]
    fn the_old_database_is_kept_from_starting_at_boot_and_let_again() {
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("start.conf");
        std::fs::write(&conf, "# Automatic startup\nauto\n").unwrap();
        boot(&debian, false, &conf).unwrap();
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "manual\n");
        boot(&debian, true, &conf).unwrap();
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "auto\n");
    }

    #[test]
    fn the_machines_must_be_three_machines() {
        // As `plan::resolve`: an address is itself.
        let dns = |name: &str| match name {
            "db1.example.org" => vec!["192.0.2.21".parse().unwrap()],
            _ => name.parse().into_iter().collect(),
        };
        let pair = |a: &str, b: &str| [a.to_string(), b.to_string()];
        for (nodes, hub) in [
            (pair("db1.example.org", "192.0.2.21"), "192.0.2.10"),
            (pair("DB1.example.org", "db1.example.org"), "192.0.2.10"),
            (pair("db1.example.org", "db2.example.org"), "192.0.2.21"),
        ] {
            let error = machines_apart(&nodes, hub, dns).unwrap_err();
            assert!(error.contains("same machine"), "{error}");
        }
        let error =
            machines_apart(&pair("db1.example.org", "db2.example.org"), "a b", dns).unwrap_err();
        assert!(error.contains("not a name or an address"), "{error}");
        assert_eq!(
            machines_apart(
                &pair("db1.example.org", "db2.example.org"),
                "192.0.2.10",
                dns
            ),
            Ok(())
        );
    }

    #[test]
    fn the_root_helper_reads_the_moved_settings_too() {
        assert!(RESTARTED.contains(&"scannerpulse-agent.service"));
    }

    #[test]
    fn the_old_database_is_kept_from_starting_and_then_removed() {
        let debian = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let held = format!("{:?}", hold_steps());
        assert!(held.contains("scannerpulse-backup.timer"), "{held}");
        // pgBackRest's settings go at the switch: `uninstall` would reach
        // for the old database through them.
        assert!(held.contains(crate::backup::PGBACKREST_CONF), "{held}");
        let gone = format!("{:?}", retire_steps(&debian));
        assert!(gone.contains("pg_dropcluster"), "{gone}");
        let rocky = detect("ID=\"rocky\"\nVERSION_ID=\"9.4\"\n", "x86_64").unwrap();
        let Action::Run { env, .. } = &retire_steps(&rocky)[0].action else {
            panic!()
        };
        assert!(env.contains(&("DATA".to_string(), "/var/lib/pgsql/17/data".to_string())));
    }
}
