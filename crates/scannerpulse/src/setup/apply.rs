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
    pub db_password: String,
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
    let options = context
        .database_url
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|error| error.to_string())?
        .password(&context.db_password);
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

/// Waits until `web` answers `/health` over HTTPS with the certificate setup made.
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
    use crate::setup::steps::{Action, Step};
    use std::os::unix::fs::PermissionsExt;

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
