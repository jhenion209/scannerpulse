//! Runs a step's shell script in a test, with stub programs first on the
//! path, each logging its arguments.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::steps::{Action, Step};

pub struct Ran {
    pub ok: bool,
    /// Each stub's call, one a line: its name and arguments.
    pub calls: String,
    pub stderr: String,
}

/// Runs `step`'s script with `stubs` (name, body) first on the path, `env`
/// over the step's own, and `input` on standard input.
pub fn run(
    step: &Step,
    stubs: &[(&str, &str)],
    env: &[(&str, &str)],
    dir: &Path,
    input: &str,
) -> Ran {
    let Action::Run {
        program,
        args,
        env: own,
        ..
    } = &step.action
    else {
        panic!("{} is not a script", step.title)
    };
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    for (name, body) in stubs {
        let path = bin.join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"{name} $*\" >> \"$LOG\"\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let log = dir.join("calls");
    let mut child = std::process::Command::new(program)
        .args(&args[..])
        .envs(own.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .envs(env.iter().copied())
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("LOG", &log)
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    Ran {
        ok: output.status.success(),
        calls: std::fs::read_to_string(&log).unwrap_or_default(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}
