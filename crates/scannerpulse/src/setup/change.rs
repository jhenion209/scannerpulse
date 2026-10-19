//! Changes to a machine already set up, made as root from the command line:
//! the certificate, and the backups' folder and push addresses. The web
//! service runs without root, so the admin page shows each of these as the
//! command to run here.

use std::path::{Path, PathBuf};

use crate::acme::Proof;

use super::steps::{ACCOUNT, Step, backup_steps, cert_path, env_path, key_path, run};
use super::system::System;

/// What to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Serve this certificate and key (PEM) from now on.
    TlsInstall { cert: PathBuf, key: PathBuf },
    /// Go back to a certificate this machine makes for itself.
    TlsSelf,
    /// Let's Encrypt's certificate for `names` from now on, proved over the
    /// web or (`dns`) through Cloudflare, with the token read from standard
    /// input like a push address.
    LetsEncrypt { dns: bool, names: Vec<String> },
    /// After each backup of `kind` succeeds, call the address read from
    /// standard input, or nothing (`off`). The address carries a token, so
    /// it is never an argument: sudo logs arguments, and ps shows them.
    Push { kind: String, off: bool },
    /// Keep the backups in `folder` from now on.
    Location(PathBuf),
    /// Add `name`, a second ScannerPulse machine sharing `shared`
    /// (address/prefix) with this one.
    Second {
        name: String,
        shared: Option<String>,
    },
}

/// The settings file, rewritten with `name` set to `value` (or removed).
fn settings_file(text: &str, name: &str, value: Option<&str>) -> Result<Step, String> {
    settings_with(text, &[(name, value)])
}

/// The settings file, rewritten with each name set to its value (or
/// removed).
fn settings_with(text: &str, changes: &[(&str, Option<&str>)]) -> Result<Step, String> {
    let mut contents = text.to_string();
    for (name, value) in changes {
        contents = crate::hostfile::changed(&contents, name, *value)?;
    }
    Ok(Step {
        title: "Write this machine's settings file".into(),
        action: super::steps::Action::Write {
            path: env_path(),
            contents,
            mode: 0o640,
            owner: format!("root:{ACCOUNT}"),
        },
    })
}

/// Let's Encrypt off: another certificate is served. Another authority or
/// a stand-in named for a trial goes too, so a later `tls letsencrypt` asks
/// Let's Encrypt itself.
fn without_acme(text: &str) -> Result<Step, String> {
    settings_with(
        text,
        &[
            ("SCANNERPULSE_ACME", None),
            ("SCANNERPULSE_ACME_NAMES", None),
            ("SCANNERPULSE_CLOUDFLARE_TOKEN", None),
            ("SCANNERPULSE_ACME_DIRECTORY", None),
            ("SCANNERPULSE_CLOUDFLARE_API", None),
        ],
    )
}

/// The web service reads the settings file when it starts: restarting it
/// serves a new certificate and shows the admin page a change.
fn restart_web(why: &str) -> Step {
    run(
        &format!("Restart the web service, {why}"),
        "systemctl",
        &["restart", "scannerpulse-web"],
    )
}

/// Copies a checked certificate and key in, and restarts the web service.
pub fn tls_install_steps(text: &str, cert: &Path, key: &Path) -> Result<Vec<Step>, String> {
    let (from_cert, from_key) = (cert.display().to_string(), key.display().to_string());
    let (to_cert, to_key) = (
        cert_path().display().to_string(),
        key_path().display().to_string(),
    );
    Ok(vec![
        run(
            "Copy the site's certificate",
            "install",
            &[
                "-m", "0644", "-o", ACCOUNT, "-g", ACCOUNT, &from_cert, &to_cert,
            ],
        ),
        run(
            "Copy its key, for the scannerpulse account alone",
            "install",
            &[
                "-m", "0600", "-o", ACCOUNT, "-g", ACCOUNT, &from_key, &to_key,
            ],
        ),
        without_acme(text)?,
        restart_web("to serve the certificate"),
    ])
}

/// Makes a new certificate for `names`, and restarts the web service.
pub fn tls_self_steps(text: &str, binary: &Path, names: &[String]) -> Result<Vec<Step>, String> {
    let (cert, key) = (
        cert_path().display().to_string(),
        key_path().display().to_string(),
    );
    let mut args = vec!["cert", "make", "--cert", &cert, "--key", &key];
    args.extend(names.iter().map(String::as_str));
    Ok(vec![
        run(
            "Make this machine's certificate",
            &binary.display().to_string(),
            &args,
        ),
        run(
            "Give the certificate's key to the scannerpulse account",
            "chown",
            &[&format!("{ACCOUNT}:{ACCOUNT}"), &key],
        ),
        without_acme(text)?,
        restart_web("to serve the certificate"),
    ])
}

/// Let's Encrypt for `names`, proved as `proof` says: the settings file
/// names them, and the web, restarted, asks for the certificate. The one
/// served now stays until it arrives.
pub fn letsencrypt_steps(text: &str, proof: &Proof, names: &[String]) -> Result<Vec<Step>, String> {
    for name in names {
        crate::acme::check_name(name)?;
    }
    let token = match proof {
        Proof::Web => None,
        Proof::Cloudflare { token } => Some(token.as_str()),
    };
    Ok(vec![
        settings_with(
            text,
            &[
                ("SCANNERPULSE_ACME", Some(proof.name())),
                ("SCANNERPULSE_ACME_NAMES", Some(&names.join(" "))),
                ("SCANNERPULSE_CLOUDFLARE_TOKEN", token),
            ],
        )?,
        restart_web("to ask Let's Encrypt for the certificate"),
    ])
}

/// Setup keeps backups only of a database it installed, and then the
/// settings file (`text`) names their folder.
fn keeps_backups(text: &str) -> Result<(), String> {
    let pairs = crate::hostfile::parse(text)?;
    if pairs.iter().any(|(name, _)| name == "SCANNERPULSE_BACKUPS") {
        Ok(())
    } else {
        Err("this machine keeps no backups: its database is one you run elsewhere".into())
    }
}

/// The settings file `text` with `kind`'s push address set or removed.
pub fn push_steps(text: &str, kind: &str, url: Option<&str>) -> Result<Vec<Step>, String> {
    keeps_backups(text)?;
    let var = crate::backup::push_var(kind)
        .ok_or_else(|| format!("{kind:?} is not a kind of backup: pgbackrest or dump"))?;
    if let Some(url) = url {
        super::ask::check_push(url)?;
    }
    Ok(vec![
        settings_file(text, var, url)?,
        restart_web("to show it on the admin page"),
    ])
}

/// The backups moved to `folder`: pgBackRest's place made there (its first
/// full backup taken at once, since the old ones stay where they were), the
/// nightly units rewritten, and then, once that backup is there, the
/// settings file pointed at it.
pub fn location_steps(
    text: &str,
    folder: &Path,
    binary: &Path,
    system: &System,
) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    backup_steps(&mut steps, folder, binary, system);
    steps.push(settings_file(
        text,
        "SCANNERPULSE_BACKUPS",
        Some(&folder.display().to_string()),
    )?);
    steps.push(run("Read the new units", "systemctl", &["daemon-reload"]));
    steps.push(restart_web("to show it on the admin page"));
    Ok(steps)
}

/// A new backups `folder` may be inside neither a recordings folder nor the
/// archive that the database records (`known`, from it), nor hold one. The
/// database must answer to say which they are.
pub(super) fn location_apart(
    folder: &Path,
    known: Option<(Vec<PathBuf>, Option<PathBuf>, i64)>,
) -> Result<(), String> {
    let Some((recordings, archive, _)) = known else {
        return Err("the database must answer, to check the folder against the recordings folder and the archive (and for the full backup that follows): start PostgreSQL, then try again".into());
    };
    let mut others: Vec<(&Path, &str)> = recordings
        .iter()
        .map(|r| (r.as_path(), "the recordings folder"))
        .collect();
    others.extend(archive.as_deref().map(|a| (a, "the archive")));
    super::ask::backups_apart(folder, &others)
}

/// A secret (a push address, a token): asked with hidden input on a
/// terminal, else the first line of standard input.
fn read_secret(question: &str) -> Result<String, String> {
    use std::io::IsTerminal;
    let secret = if std::io::stdin().is_terminal() {
        inquire::Password::new(question)
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()
            .map_err(|error| error.to_string())?
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| format!("cannot read standard input: {error}"))?;
        line
    };
    Ok(secret.trim().to_string())
}

/// The plan names a second ScannerPulse machine.
fn second_planned() -> bool {
    super::plan::Plan::read(Path::new(super::plan::PLAN)).is_ok_and(|plan| plan.second.is_some())
}

/// Makes `change` on this machine, as root, saying each step.
pub async fn run_change(change: Change) -> Result<(), String> {
    if !rustix::process::geteuid().is_root() {
        return Err("this changes the machine's own files: run it as root (sudo /usr/local/bin/scannerpulse ...)".into());
    }
    let text = std::fs::read_to_string(env_path()).map_err(|error| {
        format!(
            "{}: {error} (is this machine set up?)",
            env_path().display()
        )
    })?;
    let binary = Path::new(super::BINARY);
    let (steps, said) = match &change {
        Change::TlsInstall { cert, key } => {
            let found = crate::cert::check(cert, key)?;
            (
                tls_install_steps(&text, cert, key)?,
                format!(
                    "The site serves the certificate for {}, good until {}.",
                    found.names.join(", "),
                    found.not_after.format("%Y-%m-%d")
                ),
            )
        }
        Change::TlsSelf => {
            let names = super::ask::defaults().names;
            let said = format!(
                "The site serves a certificate this machine made for {}; the browser warns once.",
                names.join(", ")
            );
            (tls_self_steps(&text, binary, &names)?, said)
        }
        Change::LetsEncrypt { dns, names } => {
            if !dns && second_planned() {
                return Err("with a second ScannerPulse machine, Let's Encrypt proves the names through DNS (dns-cloudflare): over the web, only the machine holding the address could answer".into());
            }
            let names: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
            for name in &names {
                crate::acme::check_name(name)?;
            }
            let proof = if *dns {
                let token =
                    read_secret("Cloudflare API token (Zone, DNS, Edit on the site's zone):")?;
                let cloudflare = crate::cloudflare::Cloudflare::from_env(&token);
                for name in &names {
                    cloudflare.check_edit(name).await?;
                }
                Proof::Cloudflare { token }
            } else {
                Proof::Web
            };
            let how = match proof {
                Proof::Web => {
                    "Let's Encrypt must reach this machine's HTTPS port at each name, through port 443 from the internet (a router may forward 443 to it)"
                }
                Proof::Cloudflare { .. } => {
                    "each name is proved by a TXT record the site writes at Cloudflare"
                }
            };
            (
                letsencrypt_steps(&text, &proof, &names)?,
                format!(
                    "The site asks Let's Encrypt for a certificate for {} now, and serves its current one until it arrives; {how}. The admin page's This machine shows how it went.",
                    names.join(", ")
                ),
            )
        }
        Change::Push { kind, off } => {
            let url = if *off {
                None
            } else {
                Some(read_secret(&format!(
                    "The push address after each {kind} backup:"
                ))?)
            };
            (
                push_steps(&text, kind, url.as_deref())?,
                match url {
                    Some(_) => {
                        format!("Each {kind} backup that succeeds is pushed to the address given.")
                    }
                    None => format!("No push follows a {kind} backup."),
                },
            )
        }
        Change::Location(folder) => {
            keeps_backups(&text)?;
            let found = super::folders::check(folder)?;
            location_apart(&found.path, super::uninstall::folders().await)?;
            let os_release = std::fs::read_to_string("/etc/os-release")
                .map_err(|error| format!("/etc/os-release: {error}"))?;
            let system = super::system::detect(&os_release, std::env::consts::ARCH)?;
            let old = std::env::var("SCANNERPULSE_BACKUPS").unwrap_or_default();
            (
                location_steps(&text, &found.path, binary, &system)?,
                format!(
                    "The backups now go to {}. The ones taken before stay in {old}: remove them when you no longer need them.",
                    found.path.display()
                ),
            )
        }
        Change::Second { name, shared } => {
            let plan = super::plan::Plan::read(Path::new(super::plan::PLAN)).map_err(|_| {
                "a second ScannerPulse machine goes with two database machines, which this install does not have".to_string()
            })?;
            let le_web = crate::hostfile::parse(&text)?
                .iter()
                .any(|(name, value)| name == "SCANNERPULSE_ACME" && value == "web");
            super::ask::check_machines(
                &plan.nodes,
                plan.referee.as_deref(),
                &plan.hub,
                Some(name),
                shared.as_deref(),
                le_web,
                super::plan::resolve,
            )?;
            let (recordings, archive, _) = super::uninstall::folders().await.unwrap_or_default();
            let (Some(recordings), Some(archive)) = (recordings.first(), archive) else {
                return Err(
                    "the folders are recorded in the database, which does not answer".into(),
                );
            };
            let os_release = std::fs::read_to_string("/etc/os-release")
                .map_err(|error| format!("/etc/os-release: {error}"))?;
            let system = super::system::detect(&os_release, std::env::consts::ARCH)?;
            let port = super::second::port_of(&text)?;
            // ponytail: the plan carries the first recordings folder only; a
            // second source the second machine cannot reach is the person's.
            (
                super::second::add_steps(
                    &plan,
                    name,
                    shared.as_deref(),
                    &archive,
                    recordings,
                    &system,
                    port,
                ),
                format!(
                    "{name} is the second ScannerPulse machine{}. Join each database machine again (`sudo /usr/local/bin/scannerpulse setup` there, Advanced, then \"join an existing install\"), then {name} the same way, after the install line.",
                    shared
                        .as_ref()
                        .map(|s| format!(", sharing {s} with this one"))
                        .unwrap_or_default()
                ),
            )
        }
    };
    let context = super::apply::Context {
        database_url: std::env::var("DATABASE_URL").unwrap_or_default(),
        db_password: String::new(),
        nodes: crate::primary::from_env()?,
    };
    super::apply::apply(&steps, &context, true).await?;
    println!("{said}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{letsencrypt_steps, location_steps, push_steps, tls_install_steps};
    use crate::acme::Proof;
    use crate::setup::steps::Action;
    use crate::setup::system::detect;
    use std::path::Path;

    const SETTINGS: &str = "SCANNERPULSE_HOST=\"radio-a\"\nSCANNERPULSE_BACKUPS=\"/srv/archive/.scannerpulse-backups\"\n";

    fn settings(steps: &[crate::setup::steps::Step]) -> String {
        match &steps
            .iter()
            .find(|s| s.title == "Write this machine's settings file")
            .unwrap()
            .action
        {
            Action::Write { contents, .. } => contents.clone(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_installed_certificate_is_copied_and_the_web_restarted() {
        let with_acme = format!(
            "{SETTINGS}SCANNERPULSE_ACME=\"web\"\nSCANNERPULSE_ACME_NAMES=\"radio.example.org\"\nSCANNERPULSE_ACME_DIRECTORY=\"https://acme-staging-v02.api.letsencrypt.org/directory\"\n"
        );
        let steps = tls_install_steps(
            &with_acme,
            Path::new("/root/site.pem"),
            Path::new("/root/site.key"),
        )
        .unwrap();
        let titles: Vec<&str> = steps.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Copy the site's certificate",
                "Copy its key, for the scannerpulse account alone",
                "Write this machine's settings file",
                "Restart the web service, to serve the certificate"
            ]
        );
        assert!(!settings(&steps).contains("ACME"), "Let's Encrypt is off");
        let own = super::tls_self_steps(
            &with_acme,
            Path::new("/usr/local/bin/scannerpulse"),
            &["radio-a".to_string()],
        )
        .unwrap();
        assert!(
            !settings(&own).contains("ACME"),
            "tls self turns it off too"
        );
    }

    #[test]
    fn lets_encrypt_is_named_in_the_settings_file_and_the_web_restarted() {
        let names = vec!["radio.example.org".to_string()];
        let dns = letsencrypt_steps(
            SETTINGS,
            &Proof::Cloudflare {
                token: "cf-token-1".into(),
            },
            &names,
        )
        .unwrap();
        let text = settings(&dns);
        for want in [
            "SCANNERPULSE_ACME=\"dns-cloudflare\"\n",
            "SCANNERPULSE_ACME_NAMES=\"radio.example.org\"\n",
            "SCANNERPULSE_CLOUDFLARE_TOKEN=\"cf-token-1\"\n",
            "SCANNERPULSE_HOST=\"radio-a\"\n",
        ] {
            assert!(text.contains(want), "{want}: {text}");
        }
        assert_eq!(
            dns.last().unwrap().title,
            "Restart the web service, to ask Let's Encrypt for the certificate"
        );
        let web = letsencrypt_steps(&text, &Proof::Web, &names).unwrap();
        let text = settings(&web);
        assert!(text.contains("SCANNERPULSE_ACME=\"web\"\n"), "{text}");
        assert!(!text.contains("CLOUDFLARE"), "the token goes: {text}");
        let refused = letsencrypt_steps(SETTINGS, &Proof::Web, &["192.0.2.10".to_string()]);
        assert!(refused.unwrap_err().contains("radio.example.org"));
    }

    #[test]
    fn a_push_address_is_set_or_removed_in_the_settings_file() {
        let contents = |steps: Vec<crate::setup::steps::Step>| match &steps[0].action {
            Action::Write { contents, .. } => contents.clone(),
            other => panic!("{other:?}"),
        };
        let set = contents(
            push_steps(
                SETTINGS,
                "dump",
                Some("https://kuma.example.org/api/push/x?status=up"),
            )
            .unwrap(),
        );
        assert!(
            set.contains(
                "SCANNERPULSE_BACKUP_PUSH_DUMP=\"https://kuma.example.org/api/push/x?status=up\"\n"
            ),
            "{set}"
        );
        assert!(set.contains("SCANNERPULSE_HOST=\"radio-a\"\n"));
        let off = contents(push_steps(&set, "dump", None).unwrap());
        assert!(!off.contains("PUSH_DUMP"), "{off}");
        assert!(
            push_steps(SETTINGS, "weekly", None)
                .unwrap_err()
                .contains("not a kind of backup")
        );
        assert!(push_steps(SETTINGS, "dump", Some("kuma")).is_err());
        let theirs = "SCANNERPULSE_HOST=\"radio-a\"\n";
        for url in [Some("https://kuma.example.org/api/push/x"), None] {
            let error = push_steps(theirs, "dump", url).unwrap_err();
            assert!(error.contains("keeps no backups"), "{error}");
        }
        let refused = push_steps(SETTINGS, "dump", Some("kuma-token-4f2a")).unwrap_err();
        assert!(
            !refused.contains("kuma-token-4f2a"),
            "the address is not echoed: {refused}"
        );
    }

    #[test]
    fn moving_the_backups_points_everything_at_the_new_folder() {
        let steps = location_steps(
            SETTINGS,
            Path::new("/srv/backups"),
            Path::new("/usr/local/bin/scannerpulse"),
            &detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap(),
        )
        .unwrap();
        let write = |title: &str| match &steps.iter().find(|s| s.title == title).unwrap().action {
            Action::Write { contents, .. } => contents.clone(),
            other => panic!("{other:?}"),
        };
        assert!(
            write("Write this machine's settings file")
                .contains("SCANNERPULSE_BACKUPS=\"/srv/backups\"\n")
        );
        assert!(
            write("Write pgBackRest's settings").contains("repo1-path=/srv/backups/pgbackrest\n")
        );
        assert!(
            write("Write scannerpulse-backup.service")
                .contains("RequiresMountsFor=\"/srv/backups\"\n")
        );
        let titles: Vec<&str> = steps.iter().map(|s| s.title.as_str()).collect();
        // The settings file (and so the admin page) names the new folder only
        // once its first full backup is there.
        let at = |title: &str| titles.iter().position(|t| *t == title).unwrap();
        assert!(at("Write this machine's settings file") > at("Take the first full backup"));
        assert_eq!(
            &titles[titles.len() - 3..],
            [
                "Write this machine's settings file",
                "Read the new units",
                "Restart the web service, to show it on the admin page"
            ]
        );
    }

    #[test]
    fn a_new_backups_folder_is_checked_against_the_recorded_folders() {
        use super::location_apart;
        use std::path::PathBuf;
        let (rec, arc) = (
            PathBuf::from("/srv/recordings"),
            PathBuf::from("/srv/archive"),
        );
        let silent = location_apart(Path::new("/srv/backups"), None).unwrap_err();
        assert!(silent.contains("the database must answer"), "{silent}");
        let no_source = Some((vec![], Some(arc.clone()), 0));
        let inside = location_apart(Path::new("/srv/archive/b"), no_source).unwrap_err();
        assert!(inside.contains("may not be inside each other"), "{inside}");
        let known = Some((vec![rec.clone()], Some(arc.clone()), 0));
        assert!(location_apart(Path::new("/srv/recordings/b"), known.clone()).is_err());
        assert!(location_apart(Path::new("/srv/backups"), known).is_ok());
    }
}
