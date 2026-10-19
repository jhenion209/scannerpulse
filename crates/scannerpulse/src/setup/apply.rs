//! Runs setup's steps in order, saying each one as it starts. The first that
//! fails stops setup with what failed; every step can run again, so setup is
//! finished by running it again once the cause is fixed.

use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use super::steps::{Action, Step};

/// How long setup waits for `web` to answer at the end.
pub const WEB_WAIT: Duration = Duration::from_secs(60);

/// What the steps that touch the database need.
pub struct Context {
    pub database_url: String,
    /// The database role's password; none keeps the URL's.
    pub db_password: String,
    /// The database machines, when there are two: the steps that reach the
    /// database reach the primary.
    pub nodes: Vec<crate::primary::Node>,
}

/// Runs `steps`, saying each; `quiet` keeps a step's own output (apt's, for
/// one) unless it fails.
pub async fn apply(steps: &[Step], context: &Context, quiet: bool) -> Result<(), String> {
    let total = steps.len();
    for (n, step) in steps.iter().enumerate() {
        let n = n + 1;
        println!("[{n}/{total}] {}", step.title);
        let fail = |why: String| {
            format!(
                "setup stopped at step {n} of {total}, {}: {why}",
                step.title
            )
        };
        match &step.action {
            Action::Run {
                program,
                args,
                input,
                env,
                unless,
            } => {
                if unless.as_deref().is_some_and(Path::exists) {
                    println!("      already done");
                    continue;
                }
                run(program, args, input.as_deref(), env, quiet).map_err(fail)?;
            }
            Action::Write {
                path,
                contents,
                mode,
                owner,
            } => {
                write(path, contents, *mode, owner).map_err(fail)?;
            }
            Action::Folders {
                recordings,
                archive,
            } => {
                folders(context, recordings, archive).await.map_err(fail)?;
            }
            Action::Admin { login, password } => {
                let mut conn = connect(context).await.map_err(fail)?;
                first_admin(&mut conn, login, password)
                    .await
                    .map_err(fail)?;
            }
            Action::WaitForWeb { port } => wait_for_web(*port).await.map_err(fail)?,
            Action::Plan {
                nodes,
                hub,
                password,
                referee,
                second,
                shared,
                archive,
                recordings,
            } => write_plan(super::plan::Plan {
                hub: hub.clone(),
                hub_key: String::new(),
                nodes: nodes.clone(),
                database: super::steps::DATABASE.into(),
                role: super::steps::ACCOUNT.into(),
                password: password.clone(),
                replication_password: String::new(),
                referee: referee.clone(),
                monitor_password: String::new(),
                second: second.clone(),
                shared: shared.clone(),
                install_id: String::new(),
                archive: archive.display().to_string(),
                recordings: recordings.display().to_string(),
                ready: false,
                joined: Vec::new(),
            })
            .map_err(fail)?,
            Action::Ready => super::plan::Plan::update(Path::new(super::plan::PLAN), |plan| {
                plan.ready = true;
            })
            .map_err(fail)?,
            Action::WaitForNodes => wait_for_nodes(context).await.map_err(fail)?,
            Action::WaitForSecond => wait_for_second().await.map_err(fail)?,
        }
    }
    Ok(())
}

fn run(
    program: &str,
    args: &[String],
    input: Option<&str>,
    env: &[(String, String)],
    quiet: bool,
) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args).envs(env.iter().map(|(k, v)| (k, v)));
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    if quiet {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("cannot write to {program}: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("{program}: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let tail = said(&output);
    Err(format!(
        "{program} ended with {}{}",
        output.status,
        if tail.is_empty() {
            String::new()
        } else {
            format!(":\n{tail}")
        }
    ))
}

/// The last 10 lines a program printed: its standard output, then its
/// errors, so the reason a program gives last survives its progress lines.
pub fn said(output: &std::process::Output) -> String {
    let lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&output.stderr).lines())
        .map(str::to_string)
        .collect();
    lines[lines.len().saturating_sub(10)..].join("\n")
}

fn write(path: &Path, contents: &str, mode: u32, owner: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .and_then(|()| file.write_all(contents.as_bytes()))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    run(
        "chown",
        &[owner.to_string(), path.display().to_string()],
        None,
        &[],
        true,
    )
}

async fn connect(context: &Context) -> Result<sqlx::PgConnection, String> {
    use sqlx::Connection;
    let mut options = context
        .database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|error| error.to_string())?;
    if !context.db_password.is_empty() {
        options = options.password(&context.db_password);
    }
    let options = crate::primary::options_on(options, &context.nodes)
        .await
        .map_err(|error| format!("cannot reach the new database: {error}"))?;
    sqlx::PgConnection::connect_with(&options)
        .await
        .map_err(|error| format!("cannot reach the new database: {error}"))
}

async fn folders(context: &Context, recordings: &Path, archive: &Path) -> Result<(), String> {
    let mut conn = connect(context).await?;
    super::folders::record(&mut conn, recordings, archive)
        .await
        .map_err(|error| format!("cannot record the folders: {error}"))
}

/// Adds the admin `login`, or gives it `password` if it exists already.
pub async fn first_admin(
    conn: &mut sqlx::PgConnection,
    login: &str,
    password: &str,
) -> Result<(), String> {
    use crate::accounts;
    let found = accounts::by_login(&mut *conn, login)
        .await
        .map_err(|error| error.to_string())?;
    match found {
        None => accounts::add(conn, login, password).await.map(drop),
        Some(_) => accounts::set_password(conn, login, password).await,
    }
    .map_err(|error| format!("the admin {login}: {error}"))
}

/// Writes the plan of the database machines with this machine's public
/// key. A plan for the same machines keeps who has joined and the
/// replication role's password, so setup can run again.
fn write_plan(mut fresh: super::plan::Plan) -> Result<(), String> {
    use super::plan::{PLAN, Plan, SSH};
    let key_path = format!("{SSH}/id_ed25519.pub");
    fresh.hub_key = std::fs::read_to_string(&key_path)
        .map_err(|error| format!("{key_path}: {error}"))?
        .trim()
        .to_string();
    // A machine joining meanwhile waits for the plan's lock.
    Plan::replace(Path::new(PLAN), |kept| {
        may_replan(kept.as_ref(), &fresh.nodes, fresh.referee.as_deref())?;
        let plan = next_plan(kept, fresh);
        if plan.second.is_some() {
            mark_archive(&plan)?;
        }
        Ok(plan)
    })
}

/// Whether the plan may name `referee` (or none) for `nodes`: once the
/// same machines have joined, their referee (or their promotion by hand)
/// stays, since they are registered with it; replacing the referee is the
/// README's, and a change of mode a new install.
pub(super) fn may_replan(
    kept: Option<&super::plan::Plan>,
    nodes: &[String],
    referee: Option<&str>,
) -> Result<(), String> {
    match kept {
        Some(old)
            if old.nodes == nodes
                && old.referee.as_deref() != referee
                && !old.joined.is_empty() =>
        {
            let had = match &old.referee {
                Some(r) => format!("{r} as their referee"),
                None => "promotion by hand".into(),
            };
            Err(format!(
                "the database machines have joined with {had}: setup keeps it (to replace a referee, give the new one the same name, as the README says)"
            ))
        }
        _ => Ok(()),
    }
}

/// The install's mark in the archive, which the second machine reads to
/// know it reaches the same share; written as the service account, which a
/// network share lets write where root may not.
fn mark_archive(plan: &super::plan::Plan) -> Result<(), String> {
    let mark = Path::new(&plan.archive).join(super::second::MARK);
    let output = std::process::Command::new("runuser")
        .args([
            "-u",
            super::steps::ACCOUNT,
            "--",
            "sh",
            "-c",
            "printf %s \"$1\" > \"$2\"",
            "mark",
        ])
        .arg(&plan.install_id)
        .arg(&mark)
        .output()
        .map_err(|error| format!("cannot run runuser: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "cannot write {}: {}",
            mark.display(),
            said(&output)
        ))
    }
}

/// The plan made again from `fresh` (this run's answers), keeping from the
/// plan `kept` what the machines already hold: while the database machines
/// and the referee stay, their passwords and who has joined; the install's
/// mark always. A second machine added or changed sends the database
/// machines back to join again, since their access rules must let it in.
pub(super) fn next_plan(
    kept: Option<super::plan::Plan>,
    fresh: super::plan::Plan,
) -> super::plan::Plan {
    let install_id = kept
        .as_ref()
        .map(|old| old.install_id.clone())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| crate::sessions::random_hex(8));
    let same = kept.filter(|old| old.nodes == fresh.nodes && old.referee == fresh.referee);
    let joined = match &same {
        Some(old) if old.second == fresh.second => old.joined.clone(),
        Some(old) => old
            .joined
            .iter()
            .filter(|m| !fresh.nodes.contains(m))
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    super::plan::Plan {
        replication_password: same
            .as_ref()
            .map(|old| old.replication_password.clone())
            .unwrap_or_else(|| crate::sessions::random_hex(24)),
        monitor_password: match (&same, &fresh.referee) {
            (Some(old), _) => old.monitor_password.clone(),
            (None, Some(_)) => crate::sessions::random_hex(24),
            (None, None) => String::new(),
        },
        install_id,
        ready: false,
        joined,
        ..fresh
    }
}

/// What the first machine says while it waits: who it waits for, and how
/// each joins, the first of them first.
pub(super) fn waiting_line(plan: &super::plan::Plan) -> String {
    let waiting = plan.waiting();
    match waiting.first() {
        None => "      all have joined; waiting for the primary to answer".into(),
        Some(first) => format!(
            "      waiting for: {}. On each, run the install line, then `sudo /usr/local/bin/scannerpulse setup`, choose Advanced, then \"join an existing install\", and give this machine's address ({}). {first} goes first.",
            waiting.join(", "),
            plan.hub
        ),
    }
}

/// Waits until both database machines have joined (each says so through
/// `scannerpulse join done`) and one of them answers as the primary.
async fn wait_for_nodes(context: &Context) -> Result<(), String> {
    use super::plan::{PLAN, Plan};
    let (mut said, mut why_said) = (None, String::new());
    loop {
        let plan = Plan::read(Path::new(PLAN))?;
        let waiting: Vec<String> = plan.waiting().into_iter().map(String::from).collect();
        if waiting.is_empty() {
            let options = context
                .database_url
                .parse::<sqlx::postgres::PgConnectOptions>()
                .map_err(|error| error.to_string())?
                .password(&context.db_password);
            let nodes: Vec<crate::primary::Node> = plan
                .nodes
                .iter()
                .map(|host| crate::primary::Node {
                    host: host.clone(),
                    port: super::steps::PORT_PG,
                })
                .collect();
            match crate::primary::find_why(&options, &nodes).await {
                Ok(_) => return Ok(()),
                Err(why) if why != why_said => {
                    println!("      no primary yet: {why}");
                    why_said = why;
                }
                Err(_) => {}
            }
        }
        if said.as_ref() != Some(&waiting) {
            println!("{}", waiting_line(&plan));
            said = Some(waiting);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Waits until every machine of the plan has joined, the second
/// ScannerPulse machine last, saying who is left when that changes.
async fn wait_for_second() -> Result<(), String> {
    use super::plan::{PLAN, Plan};
    let mut said = None;
    loop {
        let plan = Plan::read(Path::new(PLAN))?;
        let left: Vec<String> = plan.waiting_all().into_iter().map(String::from).collect();
        if left.is_empty() {
            return Ok(());
        }
        if said.as_ref() != Some(&left) {
            println!(
                "      waiting for: {}. On each, run the install line, then `sudo /usr/local/bin/scannerpulse setup`, choose Advanced, then \"join an existing install\", and give this machine's address ({}). Setup is done here: Ctrl-C leaves this wait.",
                left.join(", "),
                plan.hub
            );
            said = Some(left);
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Waits until `web` answers on this machine. Its certificate is not
/// checked: one the person installs names their site, not 127.0.0.1, and
/// nothing secret goes to the loopback address.
async fn wait_for_web(port: u16) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let url = format!("https://127.0.0.1:{port}/health");
    let started = std::time::Instant::now();
    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        if started.elapsed() > WEB_WAIT {
            return Err(format!(
                "the web interface did not answer at {url} within {} s; see `journalctl -u scannerpulse-web`",
                WEB_WAIT.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{Context, apply};
    use crate::setup::plan::Plan;
    use crate::setup::steps::{Action, Step};
    use std::os::unix::fs::PermissionsExt;

    fn nodes() -> Vec<String> {
        vec!["db1.example.org".into(), "192.0.2.22".into()]
    }

    /// The plan made from these answers over `kept`.
    fn make(
        kept: Option<Plan>,
        nodes: &[String],
        hub: &str,
        referee: Option<&str>,
        second: Option<&str>,
    ) -> Plan {
        super::next_plan(
            kept,
            Plan {
                hub: hub.into(),
                hub_key: "key".into(),
                nodes: nodes.to_vec(),
                database: "scannerpulse".into(),
                role: "scannerpulse".into(),
                password: "app".into(),
                replication_password: String::new(),
                referee: referee.map(String::from),
                monitor_password: String::new(),
                second: second.map(String::from),
                shared: None,
                install_id: String::new(),
                archive: "/srv/archive".into(),
                recordings: "/srv/recordings".into(),
                ready: true,
                joined: Vec::new(),
            },
        )
    }

    #[test]
    fn a_second_machine_added_sends_the_database_machines_to_join_again() {
        let mut kept = make(None, &nodes(), "192.0.2.10", Some("ref.example.org"), None);
        assert_eq!(kept.install_id.len(), 16);
        assert!(!kept.ready);
        for machine in ["ref.example.org", "db1.example.org", "192.0.2.22"] {
            kept.mark_joined(machine);
        }
        let again = make(
            Some(kept.clone()),
            &nodes(),
            "192.0.2.10",
            Some("ref.example.org"),
            Some("web2.example.org"),
        );
        // Their access rules must let the second in: they join again; the
        // referee stays joined, and so does the install's mark.
        assert_eq!(again.joined, ["ref.example.org"]);
        assert_eq!(again.install_id, kept.install_id);
        assert_eq!(again.replication_password, kept.replication_password);
        assert_eq!(again.second.as_deref(), Some("web2.example.org"));
        assert!(
            !again.ready,
            "the first machine is ready only at the end of its setup"
        );
    }

    #[test]
    fn the_referee_is_not_changed_once_the_machines_have_joined() {
        let mut kept = make(None, &nodes(), "192.0.2.10", Some("ref.example.org"), None);
        // Nothing has joined yet: any referee, or none.
        assert_eq!(
            super::may_replan(Some(&kept), &nodes(), Some("ref2.example.org")),
            Ok(())
        );
        assert_eq!(super::may_replan(Some(&kept), &nodes(), None), Ok(()));
        kept.mark_joined("ref.example.org");
        assert_eq!(
            super::may_replan(Some(&kept), &nodes(), Some("ref.example.org")),
            Ok(())
        );
        for other in [Some("ref2.example.org"), None] {
            let error = super::may_replan(Some(&kept), &nodes(), other).unwrap_err();
            assert!(error.contains("have joined"), "{error}");
        }
        // Promoted by hand, joined: no referee now either.
        let mut by_hand = make(None, &nodes(), "192.0.2.10", None, None);
        by_hand.mark_joined("db1.example.org");
        assert!(super::may_replan(Some(&by_hand), &nodes(), Some("ref.example.org")).is_err());
        // Other machines are a new install.
        let others = vec!["db3.example.org".to_string(), "db4.example.org".to_string()];
        assert_eq!(super::may_replan(Some(&kept), &others, None), Ok(()));
        assert_eq!(super::may_replan(None, &nodes(), None), Ok(()));
    }

    #[test]
    fn a_plan_made_again_keeps_what_its_machines_hold() {
        let first = make(None, &nodes(), "192.0.2.10", Some("ref.example.org"), None);
        assert_eq!(first.replication_password.len(), 48);
        assert_eq!(first.monitor_password.len(), 48);
        let mut kept = first.clone();
        kept.mark_joined("ref.example.org");
        // The same machines and referee: the passwords they hold, and who
        // has joined, stay.
        let again = make(
            Some(kept.clone()),
            &nodes(),
            "192.0.2.11",
            Some("ref.example.org"),
            None,
        );
        assert_eq!(again.replication_password, first.replication_password);
        assert_eq!(again.monitor_password, first.monitor_password);
        assert_eq!(again.joined, ["ref.example.org"]);
        assert_eq!(again.hub, "192.0.2.11");
        // Another referee, or other machines: new passwords, none joined.
        for (machines, referee) in [
            (nodes(), Some("ref2.example.org")),
            (
                vec!["db1.example.org".to_string(), "db3.example.org".to_string()],
                Some("ref.example.org"),
            ),
        ] {
            let other = make(Some(kept.clone()), &machines, "192.0.2.10", referee, None);
            assert_ne!(other.replication_password, first.replication_password);
            assert!(other.joined.is_empty());
        }
        // By hand: no referee's password.
        let by_hand = make(None, &nodes(), "192.0.2.10", None, None);
        assert_eq!(by_hand.monitor_password, "");
    }

    #[test]
    fn the_waiting_line_names_who_is_awaited_and_the_next_to_join() {
        let mut plan: Plan = make(None, &nodes(), "192.0.2.10", Some("ref.example.org"), None);
        let line = super::waiting_line(&plan);
        assert!(
            line.contains("waiting for: ref.example.org, db1.example.org, 192.0.2.22"),
            "{line}"
        );
        assert!(line.contains("(192.0.2.10)"), "{line}");
        assert!(line.contains("ref.example.org goes first"), "{line}");
        for machine in ["ref.example.org", "db1.example.org", "192.0.2.22"] {
            plan.mark_joined(machine);
        }
        assert!(super::waiting_line(&plan).contains("all have joined"));
    }

    fn me() -> String {
        let user = std::process::Command::new("id")
            .arg("-un")
            .output()
            .unwrap();
        let group = std::process::Command::new("id")
            .arg("-gn")
            .output()
            .unwrap();
        format!(
            "{}:{}",
            String::from_utf8_lossy(&user.stdout).trim(),
            String::from_utf8_lossy(&group.stdout).trim()
        )
    }

    fn context() -> Context {
        Context {
            database_url: String::new(),
            db_password: String::new(),
            nodes: Vec::new(),
        }
    }

    fn run(
        title: &str,
        program: &str,
        args: &[&str],
        input: Option<&str>,
        unless: Option<std::path::PathBuf>,
    ) -> Step {
        Step {
            title: title.into(),
            action: Action::Run {
                program: program.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
                input: input.map(String::from),
                env: vec![],
                unless,
            },
        }
    }

    #[tokio::test]
    async fn steps_run_in_order_a_done_one_is_skipped_and_a_file_gets_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let written = dir.path().join("written");
        let copy = format!("cat > {}", out.display());
        let steps = vec![
            run(
                "copy standard input",
                "/bin/sh",
                &["-c", &copy],
                Some("secret\n"),
                None,
            ),
            run(
                "already done",
                "/usr/bin/false",
                &[],
                None,
                Some(out.clone()),
            ),
            Step {
                title: "write".into(),
                action: Action::Write {
                    path: written.clone(),
                    contents: "x=1\n".into(),
                    mode: 0o640,
                    owner: me(),
                },
            },
        ];
        apply(&steps, &context(), true).await.unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "secret\n");
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "x=1\n");
        assert_eq!(
            std::fs::metadata(&written).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[tokio::test]
    async fn the_first_failing_step_stops_setup_and_says_which_and_why() {
        let steps = vec![
            run(
                "fails",
                "/bin/sh",
                &["-c", "echo the reason >&2; exit 3"],
                None,
                None,
            ),
            run("never runs", "/usr/bin/false", &[], None, None),
        ];
        let error = apply(&steps, &context(), true).await.unwrap_err();
        assert!(
            error.starts_with("setup stopped at step 1 of 2, fails:"),
            "{error}"
        );
        assert!(error.contains("the reason"), "{error}");
    }

    #[tokio::test]
    async fn a_failed_steps_message_ends_with_its_error_output_after_a_chatty_one() {
        let steps = vec![run(
            "chatty",
            "/bin/sh",
            &[
                "-c",
                "for n in 1 2 3 4 5 6 7 8 9 10 11 12; do echo progress $n; done; echo the reason >&2; exit 1",
            ],
            None,
            None,
        )];
        let error = apply(&steps, &context(), true).await.unwrap_err();
        assert!(error.ends_with("the reason"), "{error}");
        assert!(
            error.contains("progress 12") && !error.contains("progress 3\n"),
            "{error}"
        );
    }
}
