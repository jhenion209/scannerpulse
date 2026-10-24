//! The disk heartbeat: every 5 minutes, a push to a monitor's address
//! (an Uptime Kuma push monitor, say) while this machine's `/` is less than
//! `LIMIT` full. The monitor alerts when the pushes stop, so no push means
//! "disk": the heartbeat needs neither the database nor a share. It runs on
//! any of the install's machines, each with its own address.

use std::path::Path;

use super::steps::{Action, Step, run};

/// The push address, root's alone: it carries a token.
pub const PUSH: &str = "/etc/scannerpulse/disk-push";
/// No push from this much (in percent) of `/` used.
pub const LIMIT: u64 = 80;
pub const SERVICE: &str = "/etc/systemd/system/scannerpulse-disk.service";
pub const TIMER: &str = "/etc/systemd/system/scannerpulse-disk.timer";

/// How full the file system holding `path` is, in percent, as `df` counts
/// it: the blocks used against those used and free to ordinary accounts,
/// rounded up.
pub fn used_percent(path: &Path) -> Result<u64, String> {
    let stats =
        rustix::fs::statvfs(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let used = stats.f_blocks.saturating_sub(stats.f_bfree);
    let total = used + stats.f_bavail;
    if total == 0 {
        return Err(format!("{}: no blocks", path.display()));
    }
    Ok((used * 100).div_ceil(total))
}

/// One heartbeat: the push when `/` has room, else why there is none.
pub async fn check(push: &Path) -> Result<String, String> {
    let url = std::fs::read_to_string(push).map_err(|error| {
        format!(
            "{}: {error} (set it with `scannerpulse disk push`)",
            push.display()
        )
    })?;
    let used = used_percent(Path::new("/"))?;
    if used >= LIMIT {
        return Err(format!("/ is {used}% full (limit {LIMIT}%): no heartbeat"));
    }
    crate::backup::push(url.trim()).await?;
    Ok(format!("/ is {used}% full: heartbeat sent"))
}

/// The heartbeat set to `url`: the address kept, the timer started.
pub fn on_steps(url: &str, binary: &Path) -> Result<Vec<Step>, String> {
    super::ask::check_push(url)?;
    let write = |title: &str, path: &str, contents: String, mode: u32| Step {
        title: title.into(),
        action: Action::Write {
            path: path.into(),
            contents,
            mode,
            owner: "root:root".into(),
        },
    };
    Ok(vec![
        // The referee has none; another machine's is left as it is.
        run(
            "Make the settings folder, if this machine has none",
            "mkdir",
            &["-p", "-m", "0755", super::steps::ETC],
        ),
        write("Keep the push address", PUSH, format!("{url}\n"), 0o600),
        write(
            "Write the heartbeat's service",
            SERVICE,
            format!(
                "# Written by `scannerpulse disk push`.\n\
                 [Unit]\n\
                 Description=ScannerPulse disk heartbeat\n\
                 \n\
                 [Service]\n\
                 Type=oneshot\n\
                 ExecStart={} disk check\n",
                binary.display()
            ),
            0o644,
        ),
        write(
            "Write the heartbeat's timer",
            TIMER,
            "# Written by `scannerpulse disk push`.\n\
             [Unit]\n\
             Description=ScannerPulse disk heartbeat, every 5 minutes\n\
             \n\
             [Timer]\n\
             OnCalendar=*:0/5\n\
             AccuracySec=30s\n\
             \n\
             [Install]\n\
             WantedBy=timers.target\n"
                .into(),
            0o644,
        ),
        run("Read the new units", "systemctl", &["daemon-reload"]),
        run(
            "Start the heartbeat, now and at boot",
            "systemctl",
            &["enable", "--now", "scannerpulse-disk.timer"],
        ),
    ])
}

/// `scannerpulse disk push [off]`: the heartbeat set to the address asked
/// for (hidden on a terminal, else the first line of standard input; never
/// an argument, which sudo would log), then one heartbeat at once; or
/// stopped.
pub async fn push(off: bool, binary: &Path) -> Result<String, String> {
    let steps = if off {
        off_steps()
    } else {
        on_steps(
            &super::change::read_secret("The push address for the disk heartbeat:")?,
            binary,
        )?
    };
    let context = super::apply::Context {
        database_url: String::new(),
        db_password: String::new(),
        nodes: Vec::new(),
    };
    super::apply::apply(&steps, &context, true).await?;
    if off {
        return Ok("The disk heartbeat is stopped.".into());
    }
    match check(Path::new(PUSH)).await {
        Ok(said) => Ok(format!("{said}. The next ones follow every 5 minutes.")),
        Err(why) => Err(format!(
            "the heartbeat is set, but the first did not go: {why}"
        )),
    }
}

/// The heartbeat stopped: its timer, its units and its address gone. Run
/// where there is none, it does no harm.
pub fn off_steps() -> Vec<Step> {
    vec![
        super::node::script(
            "Stop the heartbeat",
            "systemctl disable --now scannerpulse-disk.timer 2>/dev/null || true",
            &[],
            None,
        ),
        run(
            "Remove its units and its address",
            "rm",
            &["-f", TIMER, SERVICE, PUSH],
        ),
        run("Read the units again", "systemctl", &["daemon-reload"]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disk_is_as_full_as_df_says() {
        let used = used_percent(Path::new("/")).unwrap();
        assert!((1..=100).contains(&used), "{used}");
        let error = used_percent(Path::new("/no/such/folder")).unwrap_err();
        assert!(error.starts_with("/no/such/folder: "), "{error}");
    }

    #[tokio::test]
    async fn a_full_disk_sends_no_heartbeat_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let missing = check(&dir.path().join("disk-push")).await.unwrap_err();
        assert!(missing.contains("scannerpulse disk push"), "{missing}");
        // A push address nobody answers: either / is full and nothing is
        // tried, or the push itself fails. Never a heartbeat.
        let push = dir.path().join("disk-push");
        std::fs::write(&push, "http://127.0.0.1:1/api/push/token\n").unwrap();
        let said = check(&push).await.unwrap_err();
        let used = used_percent(Path::new("/")).unwrap();
        if used >= LIMIT {
            assert_eq!(said, format!("/ is {used}% full (limit 80%): no heartbeat"));
        } else {
            assert!(said.contains("127.0.0.1:1 did not answer"), "{said}");
            assert!(!said.contains("token"), "{said}");
        }
    }

    #[test]
    fn the_heartbeat_is_kept_root_s_alone_and_runs_every_five_minutes() {
        let steps = on_steps(
            "https://kuma.example.org/api/push/abc",
            Path::new("/usr/local/bin/scannerpulse"),
        )
        .unwrap();
        let written = |path: &str| {
            steps
                .iter()
                .find_map(|s| match &s.action {
                    Action::Write {
                        path: p,
                        contents,
                        mode,
                        owner,
                    } if p == Path::new(path) => Some((contents.clone(), *mode, owner.clone())),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{path}"))
        };
        assert_eq!(
            written(PUSH),
            (
                "https://kuma.example.org/api/push/abc\n".into(),
                0o600,
                "root:root".into()
            )
        );
        assert!(
            written(SERVICE)
                .0
                .contains("ExecStart=/usr/local/bin/scannerpulse disk check\n")
        );
        assert!(written(TIMER).0.contains("OnCalendar=*:0/5\n"));
        // The referee has no settings folder; the hub's is the account's
        // (0750) and must stay so: made only when missing, never reset.
        let Action::Run { program, args, .. } = &steps[0].action else {
            panic!("{}", steps[0].title)
        };
        assert_eq!(
            (program.as_str(), args.join(" ")),
            ("mkdir", format!("-p -m 0755 {}", super::super::steps::ETC))
        );
        for step in &steps {
            if let Action::Run { program, .. } = &step.action {
                assert_ne!(program, "install", "{}", step.title);
            }
        }
        // The address is never an argument: sudo and ps would show it.
        for step in &steps {
            if let Action::Run { args, .. } = &step.action {
                assert!(!args.join(" ").contains("abc"), "{args:?}");
            }
        }
        assert!(on_steps("ftp://kuma.example.org/x", Path::new("/x")).is_err());
        let off: Vec<String> = off_steps().into_iter().map(|s| s.title).collect();
        assert_eq!(
            off,
            [
                "Stop the heartbeat",
                "Remove its units and its address",
                "Read the units again"
            ]
        );
    }
}
