//! `scannerpulse setup`: the questions, their checks, and the steps that
//! build a working site on this machine.

pub mod apply;
pub mod ask;
pub mod change;
pub mod folders;
pub mod join;
pub mod node;
pub mod plan;
pub mod referee;
pub mod screen;
pub mod steps;
pub mod system;
pub mod uninstall;

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
                "setup installs packages and services: run it as root (sudo /usr/local/bin/scannerpulse setup)"
                    .into(),
            );
        }
        // A setup that stopped part way, even at its last steps (the web
        // unit written, the web not running), runs again; a finished one
        // does not.
        let running = || {
            std::process::Command::new("systemctl")
                .args(["is-active", "--quiet", "scannerpulse-web.service"])
                .status()
                .is_ok_and(|status| status.success())
        };
        if Path::new(steps::WEB_UNIT).exists() && running() {
            return Err(format!(
                "this machine is set up already ({} exists); `sudo /usr/local/bin/scannerpulse uninstall` removes that setup first",
                steps::WEB_UNIT
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
    // Another PostgreSQL here matters only to a database setup installs here.
    let seen = system.seen();
    let defaults = ask::defaults();
    if let Some(path) = answers_file
        && let Some(hub) = ask::join_from_file(path)?
    {
        return join::run(hub, dry_run, |_| true).await;
    }
    let mut answers = match answers_file {
        Some(path) => ask::from_file(path, &defaults)?,
        None => {
            use std::io::IsTerminal;
            if !std::io::stdin().is_terminal() {
                return Err("setup asks its questions on a terminal: run it in one (ssh -t for a remote machine), or give the answers with --answers FILE".into());
            }
            let Some(kind) = screen::choose()? else {
                println!("Setup left; nothing was changed.");
                return Ok(());
            };
            if kind == screen::Kind::Simple {
                system::other_postgres(&seen)?;
            } else if let Some(hub) = ask::ask_join().map_err(|e| e.to_string())? {
                return join::run(hub, dry_run, |question| {
                    inquire::Confirm::new(question)
                        .with_default(true)
                        .prompt()
                        .unwrap_or(false)
                })
                .await;
            }
            match ask::interactive(&defaults, None, kind == screen::Kind::Advanced)? {
                Some(answers) => answers,
                None => {
                    println!("Setup left; nothing was changed.");
                    return Ok(());
                }
            }
        }
    };
    let made_password = crate::sessions::random_hex(24);
    let (steps, db_password) = loop {
        if answers.database == steps::Database::Here {
            system::other_postgres(&seen)?;
        }
        // A database the person runs must answer, be PostgreSQL 17 and let
        // its role make tables, and a Cloudflare token must reach the site's
        // zone, before anything is planned on them.
        let online = async {
            if let steps::Database::Theirs(remote) = &answers.database {
                let version = ask::check_remote(remote).await?;
                println!("\nThe database answers: {version}.");
            }
            if let steps::Certificate::LetsEncrypt {
                proof: crate::acme::Proof::Cloudflare { token },
                names,
            } = &answers.certificate
            {
                ask::check_cloudflare(token, names).await?;
                println!(
                    "\nCloudflare's token may edit the DNS of {}.",
                    names.join(", ")
                );
            }
            Ok::<(), String>(())
        };
        match online.await {
            Ok(()) => {}
            Err(why) if answers_file.is_some() || dry_run => return Err(why),
            Err(why) => {
                println!("\n{why}\nChange the answers, or quit.");
                match ask::interactive(&defaults, Some(&answers), true)? {
                    Some(changed) => answers = changed,
                    None => {
                        println!("Setup left; nothing was changed.");
                        return Ok(());
                    }
                }
                continue;
            }
        }
        let db_password = match &answers.database {
            steps::Database::Here => made_password.clone(),
            steps::Database::Nodes(pair) => {
                plan::kept_password(plan::Plan::read(Path::new(plan::PLAN)).ok(), &pair.nodes)
                    .unwrap_or_else(|| made_password.clone())
            }
            steps::Database::Theirs(remote) => remote.password.clone(),
        };
        let steps = steps::steps(&answers, &system, Path::new(BINARY), &db_password);
        print!("{}", summary(&answers, &system, &steps));
        if defaults.port != 443 && answers.port == defaults.port {
            println!(
                "Port 443 is in use by {}, so the site is served on port {}.",
                ask::holder(443),
                answers.port
            );
        }
        if dry_run {
            println!("\nThe steps' commands (a dry run: nothing was changed):");
            for (n, step) in steps.iter().enumerate() {
                println!("  {:>2}. {}", n + 1, detail(step));
            }
            return Ok(());
        }
        if answers_file.is_some() {
            break (steps, db_password);
        }
        match ask::next()? {
            ask::Next::Go => break (steps, db_password),
            ask::Next::Quit => {
                println!("Nothing was changed.");
                return Ok(());
            }
            ask::Next::Change => {
                match ask::interactive(&defaults, Some(&answers), answers.advanced)? {
                    Some(changed) => answers = changed,
                    None => {
                        println!("Setup left; nothing was changed.");
                        return Ok(());
                    }
                }
            }
        }
    };
    let context = apply::Context {
        database_url: steps::env_pairs(&answers)
            .into_iter()
            .find(|(name, _)| *name == "DATABASE_URL")
            .map(|(_, url)| url)
            .unwrap_or_default(),
        db_password,
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
         \n  Certificate       {}\
         \n  Database          {}\
         \n  Backups           {}\
         \n  Reverse proxies   {}\
         \n  Logs              {}\
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
        match &answers.certificate {
            steps::Certificate::Own =>
                "made by setup for this machine (the browser warns once)".to_string(),
            steps::Certificate::Installed { cert, key } => format!(
                "yours, copied from {} and {}",
                cert.display(),
                key.display()
            ),
            steps::Certificate::LetsEncrypt { proof, names } => format!(
                "Let's Encrypt's for {}, proved {}; until it arrives, one setup makes",
                names.join(", "),
                match proof {
                    crate::acme::Proof::Web =>
                        "over the web (port 443 must reach this machine from the internet)",
                    crate::acme::Proof::Cloudflare { .. } => "through DNS at Cloudflare",
                }
            ),
        },
        match &answers.database {
            steps::Database::Here => format!(
                "PostgreSQL 17 on this machine, its data in {}",
                system.postgres_data()
            ),
            steps::Database::Theirs(remote) => format!(
                "yours: {} on {}:{}, as {}, TLS {}",
                remote.name,
                remote.host,
                remote.port,
                remote.role,
                remote.sslmode.name()
            ),
            steps::Database::Nodes(pair) => format!(
                "two machines of its own: {} (the primary) and {} (its standby, {}); this machine waits for them to join at {}",
                pair.nodes[0],
                pair.nodes[1],
                match &pair.referee {
                    Some(referee) => format!("promoted by itself, with {referee} as the referee"),
                    None => "promoted by hand".into(),
                },
                pair.hub
            ),
        },
        match steps::backups_folder(answers) {
            Some(folder) => format!(
                "{} (every night at 03:17: pgBackRest, full on Sundays;\n                    and a dump, the newest {} kept){}",
                folder.display(),
                crate::backup::KEEP_DUMPS,
                if answers.pushes.is_empty() {
                    String::new()
                } else {
                    let kinds: Vec<&str> = answers.pushes.iter().map(|(k, _)| k.as_str()).collect();
                    format!(
                        "\n                    a push address after each: {}",
                        kinds.join(", ")
                    )
                }
            ),
            None => "yours, with the database".into(),
        },
        if answers.proxies.is_empty() {
            "none".to_string()
        } else {
            answers.proxies.join(", ")
        },
        match &answers.syslog {
            Some(server) => format!("the journal, and {server}"),
            None => "the journal".into(),
        },
        steps::env_path().display(),
    );
    for (n, step) in steps.iter().enumerate() {
        text.push_str(&format!("  {:>2}. {}\n", n + 1, step.title));
    }
    text
}

/// The addresses a browser opens the site at.
fn addresses(answers: &Answers) -> Vec<String> {
    // A browser warns at a name the certificate does not cover.
    let names = match &answers.certificate {
        steps::Certificate::Installed { cert, .. } => crate::cert::read(cert)
            .map(|found| found.names)
            .unwrap_or_else(|_| answers.names.clone()),
        steps::Certificate::LetsEncrypt { names, .. } => names.clone(),
        steps::Certificate::Own => answers.names.clone(),
    };
    names
        .iter()
        .filter(|name| *name != "127.0.0.1" && !name.starts_with("*."))
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
        Action::Admin { login, .. } => {
            format!("add the admin {login} in the database, or give it the new password")
        }
        Action::WaitForWeb { port } => format!("wait for https://127.0.0.1:{port}/health"),
        Action::Plan {
            nodes,
            hub,
            referee,
            ..
        } => format!(
            "write {} (the database machines {} and {}{}, this machine as {hub}, the passwords; root's alone)",
            plan::PLAN,
            nodes[0],
            nodes[1],
            referee
                .as_ref()
                .map(|r| format!(", the referee {r}"))
                .unwrap_or_default()
        ),
        Action::WaitForNodes => "wait until the machines of the plan have joined".into(),
    }
}

/// Said when setup is done.
fn finished(answers: &Answers) -> String {
    format!(
        "\nScannerPulse is running. Open {}\n{}\
         The admin page is admin.html at the same address; sign in as {}.\n",
        addresses(answers).join(" or "),
        match answers.certificate {
            steps::Certificate::Own =>
                "The browser warns once about the certificate, which this machine made for itself:\n\
                 accept it, or later give the site a Let's Encrypt or your own certificate.\n",
            steps::Certificate::Installed { .. } => "",
            steps::Certificate::LetsEncrypt { .. } =>
                "Let's Encrypt's certificate comes within minutes; until then the browser warns.\n\
                 The admin page's This machine shows how its request went.\n",
        },
        answers.admin,
    )
}

#[cfg(test)]
mod tests {
    use super::{detail, finished, steps, summary, system};
    use std::path::{Path, PathBuf};

    #[test]
    fn with_your_certificate_the_site_is_opened_by_the_names_it_covers() {
        let dir = tempfile::tempdir().unwrap();
        let (cert, key) = (dir.path().join("site.pem"), dir.path().join("site.key"));
        crate::cert::make(
            &["radio.example.org".into(), "*.example.org".into()],
            &cert,
            &key,
        )
        .unwrap();
        let said = finished(&steps::Answers {
            certificate: steps::Certificate::Installed { cert, key },
            ..answers()
        });
        assert!(said.contains("Open https://radio.example.org/\n"), "{said}");
        assert!(!said.contains("radio-a"), "{said}");
        let lets_encrypt = finished(&steps::Answers {
            certificate: steps::Certificate::LetsEncrypt {
                proof: crate::acme::Proof::Web,
                names: vec!["radio.example.org".into()],
            },
            ..answers()
        });
        assert!(
            lets_encrypt.contains("Open https://radio.example.org/\n"),
            "{lets_encrypt}"
        );
        assert!(lets_encrypt.contains("Let's Encrypt"), "{lets_encrypt}");
        let own = finished(&answers());
        assert!(
            own.contains("Open https://radio-a/ or https://192.0.2.10/\n"),
            "{own}"
        );
    }

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
            advanced: false,
            database: steps::Database::Here,
            certificate: steps::Certificate::Own,
            proxies: Vec::new(),
            syslog: None,
            backups: None,
            pushes: Vec::new(),
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
            "/var/lib/postgresql/17/main",
            "/srv/archive/.scannerpulse-backups",
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
