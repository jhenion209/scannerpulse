//! `scannerpulse setup`: the questions, their checks, and the steps that
//! build a working site on this machine.

pub mod apply;
pub mod ask;
pub mod folders;
pub mod screen;
pub mod steps;
pub mod system;

use std::path::Path;

use steps::{Action, Answers, Step};

/// Where the install script puts the program; the services run this copy.
pub const BINARY: &str = "/usr/local/bin/scannerpulse";

/// Sets up this machine: asks (or reads `answers_file`), shows the summary,
/// and after a yes does every step. `dry_run` shows the steps and changes
/// nothing.
pub async fn run(answers_file: Option<&Path>, dry_run: bool) -> Result<(), String> {
    if !dry_run {
        if !rustix::process::geteuid().is_root() {
            return Err(
                "setup installs packages and services: run it as root (sudo scannerpulse setup)"
                    .into(),
            );
        }
        if steps::env_path().exists() {
            return Err(format!(
                "this machine is set up already ({} exists); `scannerpulse uninstall` removes that setup first",
                steps::env_path().display()
            ));
        }
        let me = std::env::current_exe().and_then(|p| p.canonicalize()).ok();
        if me.as_deref() != Some(Path::new(BINARY)) {
            return Err(format!(
                "run setup from {BINARY}, where the install script puts it: the services run that copy"
            ));
        }
    }
    let os_release = std::fs::read_to_string("/etc/os-release")
        .map_err(|error| format!("/etc/os-release: {error}"))?;
    let system = system::detect(&os_release, std::env::consts::ARCH)?;
    let defaults = ask::defaults();
    let answers = match answers_file {
        Some(path) => ask::from_file(path, &defaults)?,
        None => {
            if screen::choose()?.is_none() {
                println!("Setup left; nothing was changed.");
                return Ok(());
            }
            match ask::interactive(&defaults)? {
                Some(answers) => answers,
                None => {
                    println!("Setup left; nothing was changed.");
                    return Ok(());
                }
            }
        }
    };
    let db_password = crate::sessions::random_hex(24);
    let steps = steps::steps(&answers, &system, Path::new(BINARY), &db_password);
    print!("{}", summary(&answers, &system, &steps));
    if dry_run {
        println!("\nThe steps' commands (a dry run: nothing was changed):");
        for (n, step) in steps.iter().enumerate() {
            println!("  {:>2}. {}", n + 1, detail(step));
        }
        return Ok(());
    }
    if answers_file.is_none() && !ask::confirm()? {
        println!("Nothing was changed.");
        return Ok(());
    }
    let context = apply::Context {
        database_url: steps::env_pairs(&answers)
            .into_iter()
            .find(|(name, _)| *name == "DATABASE_URL")
            .map(|(_, url)| url)
            .unwrap_or_default(),
        db_password,
        cert: steps::cert_path(),
    };
    apply::apply(&steps, &context, true).await?;
    print!("{}", finished(&answers));
    Ok(())
}

/// What setup will do, shown before anything is written.
pub fn summary(answers: &Answers, system: &system::System, steps: &[Step]) -> String {
    let mut text = format!(
        "\nSetup will make this machine a ScannerPulse site:\n\
         \n  System            {} {} ({})\
         \n  Recordings        {}\
         \n  Archive           {}\
         \n  Time zone         {}\
         \n  First admin       {}\
         \n  Transcription     {}\
         \n  Address           {}\
         \n  Settings file     {}\
         \n\nThe steps:\n",
        system.id,
        system.version,
        system.arch,
        answers.recordings.display(),
        answers.archive.display(),
        answers.time_zone,
        answers.admin,
        match &answers.transcription {
            Some(t) => format!("{} (model {})", t.url, t.model),
            None => "later, in the admin page".into(),
        },
        addresses(answers).join(", "),
        steps::env_path().display(),
    );
    for (n, step) in steps.iter().enumerate() {
        text.push_str(&format!("  {:>2}. {}\n", n + 1, step.title));
    }
    text
}

/// The addresses a browser opens the site at.
fn addresses(answers: &Answers) -> Vec<String> {
    answers
        .names
        .iter()
        .filter(|name| *name != "127.0.0.1")
        .map(|name| format!("https://{name}:{}/", answers.port).replace(":443/", "/"))
        .collect()
}

/// One step as a command line or a file; what goes in on standard input
/// and a file's contents stay unsaid, since passwords travel there.
fn detail(step: &Step) -> String {
    match &step.action {
        Action::Run {
            program,
            args,
            input,
            ..
        } => format!(
            "{program} {}{}",
            args.join(" "),
            if input.is_some() {
                " (with its input)"
            } else {
                ""
            }
        ),
        Action::Write {
            path, mode, owner, ..
        } => {
            format!("write {} (mode {mode:o}, owner {owner})", path.display())
        }
        Action::Folders {
            recordings,
            archive,
        } => format!(
            "record the archive {} and the recordings folder {} in the database",
            archive.display(),
            recordings.display()
        ),
        Action::WaitForWeb { port } => format!("wait for https://127.0.0.1:{port}/health"),
    }
}

/// Said when setup is done.
fn finished(answers: &Answers) -> String {
    format!(
        "\nScannerPulse is running. Open {}\n\
         The browser warns once about the certificate, which this machine made for itself:\n\
         accept it, or later give the site a Let's Encrypt or your own certificate.\n\
         The admin page is admin.html at the same address; sign in as {}.\n",
        addresses(answers).join(" or "),
        answers.admin,
    )
}

#[cfg(test)]
mod tests {
    use super::{detail, steps, summary, system};
    use std::path::{Path, PathBuf};

    fn answers() -> steps::Answers {
        steps::Answers {
            recordings: PathBuf::from("/srv/recordings"),
            archive: PathBuf::from("/srv/archive"),
            time_zone: "America/Chicago".into(),
            admin: "chief".into(),
            password: "long enough secret".into(),
            transcription: None,
            port: 443,
            host: "radio-a".into(),
            names: vec!["radio-a".into(), "192.0.2.10".into(), "127.0.0.1".into()],
        }
    }

    #[test]
    fn the_summary_names_the_folders_the_address_and_every_step() {
        let system = system::detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let steps = steps::steps(&answers(), &system, Path::new(super::BINARY), "dbsecret");
        let text = summary(&answers(), &system, &steps);
        for want in [
            "/srv/recordings",
            "/srv/archive",
            "America/Chicago",
            "chief",
            "https://radio-a/, https://192.0.2.10/",
            "later, in the admin page",
        ] {
            assert!(text.contains(want), "{want} missing from:\n{text}");
        }
        assert!(!text.contains("127.0.0.1"));
        assert!(text.contains(&format!(
            "{:>2}. {}",
            steps.len(),
            steps.last().unwrap().title
        )));
        assert!(!text.contains("long enough secret") && !text.contains("dbsecret"));
    }

    #[test]
    fn a_dry_run_shows_each_command_and_never_a_password() {
        let system = system::detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let steps = steps::steps(&answers(), &system, Path::new(super::BINARY), "dbsecret");
        let all: String = steps.iter().map(detail).collect::<Vec<_>>().join("\n");
        assert!(all.contains("apt-get"), "{all}");
        assert!(all.contains("(with its input)"));
        assert!(
            all.contains("pgpass (mode 600, owner scannerpulse:scannerpulse)"),
            "{all}"
        );
        assert!(
            !all.contains("long enough secret") && !all.contains("dbsecret"),
            "{all}"
        );
    }
}
