//! Changes to a machine already set up, made as root from the command line:
//! the certificate, and the backups' folder and push addresses. The web
//! service runs without root, so the admin page shows each of these as the
//! command to run here.

use std::path::{Path, PathBuf};

use super::steps::{ACCOUNT, Step, backup_steps, cert_path, env_path, key_path, run};
use super::system::System;

/// What to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// Serve this certificate and key (PEM) from now on.
    TlsInstall { cert: PathBuf, key: PathBuf },
    /// Go back to a certificate this machine makes for itself.
    TlsSelf,
    /// Call `url` after each backup of `kind` succeeds, or nothing (`None`).
    Push { kind: String, url: Option<String> },
    /// Keep the backups in `folder` from now on.
    Location(PathBuf),
}

/// The settings file, rewritten with `name` set to `value` (or removed).
fn settings_file(text: &str, name: &str, value: Option<&str>) -> Result<Step, String> {
    Ok(Step {
        title: "Write this machine's settings file".into(),
        action: super::steps::Action::Write {
            path: env_path(),
            contents: crate::hostfile::changed(text, name, value)?,
            mode: 0o640,
            owner: format!("root:{ACCOUNT}"),
        },
    })
}

fn restart_web() -> Step {
    run(
        "Restart the web service, to serve the certificate",
        "systemctl",
        &["restart", "scannerpulse-web"],
    )
}

/// Copies a checked certificate and key in, and restarts the web service.
pub fn tls_install_steps(cert: &Path, key: &Path) -> Vec<Step> {
    let (from_cert, from_key) = (cert.display().to_string(), key.display().to_string());
    let (to_cert, to_key) = (
        cert_path().display().to_string(),
        key_path().display().to_string(),
    );
    vec![
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
        restart_web(),
    ]
}

/// Makes a new certificate for `names`, and restarts the web service.
pub fn tls_self_steps(binary: &Path, names: &[String]) -> Vec<Step> {
    let (cert, key) = (
        cert_path().display().to_string(),
        key_path().display().to_string(),
    );
    let mut args = vec!["cert", "make", "--cert", &cert, "--key", &key];
    args.extend(names.iter().map(String::as_str));
    vec![
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
        restart_web(),
    ]
}

/// The settings file `text` with `kind`'s push address set or removed.
pub fn push_steps(text: &str, kind: &str, url: Option<&str>) -> Result<Vec<Step>, String> {
    let var = crate::backup::push_var(kind)
        .ok_or_else(|| format!("{kind:?} is not a kind of backup: pgbackrest or dump"))?;
    if let Some(url) = url {
        super::ask::check_push(url)?;
    }
    Ok(vec![settings_file(text, var, url)?])
}

/// The backups moved to `folder`: pgBackRest's place made there (its first
/// full backup taken at once, since the old ones stay where they were), the
/// nightly units rewritten, and the settings file pointed at it.
pub fn location_steps(
    text: &str,
    folder: &Path,
    binary: &Path,
    system: &System,
) -> Result<Vec<Step>, String> {
    let mut steps = vec![settings_file(
        text,
        "SCANNERPULSE_BACKUPS",
        Some(&folder.display().to_string()),
    )?];
    backup_steps(&mut steps, folder, binary, system);
    steps.push(run("Read the new units", "systemctl", &["daemon-reload"]));
    Ok(steps)
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
                tls_install_steps(cert, key),
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
            (tls_self_steps(binary, &names), said)
        }
        Change::Push { kind, url } => (
            push_steps(&text, kind, url.as_deref())?,
            match url {
                Some(_) => {
                    format!("Each {kind} backup that succeeds is pushed to the address given.")
                }
                None => format!("No push follows a {kind} backup."),
            },
        ),
        Change::Location(folder) => {
            if !Path::new(crate::backup::PGBACKREST_CONF).exists() {
                return Err(
                    "this machine keeps no backups: its database is one you run elsewhere".into(),
                );
            }
            let found = super::folders::check(folder)?;
            if let Some((recordings, Some(archive), _)) = super::uninstall::folders().await {
                for source in &recordings {
                    super::ask::check_backups(&found.path, source, &archive)?;
                }
            }
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
    };
    let context = super::apply::Context {
        database_url: std::env::var("DATABASE_URL").unwrap_or_default(),
        db_password: String::new(),
    };
    super::apply::apply(&steps, &context, true).await?;
    println!("{said}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{location_steps, push_steps, tls_install_steps};
    use crate::setup::steps::Action;
    use crate::setup::system::detect;
    use std::path::Path;

    const SETTINGS: &str = "SCANNERPULSE_HOST=\"radio-a\"\nSCANNERPULSE_BACKUPS=\"/srv/archive/.scannerpulse-backups\"\n";

    #[test]
    fn an_installed_certificate_is_copied_and_the_web_restarted() {
        let steps = tls_install_steps(Path::new("/root/site.pem"), Path::new("/root/site.key"));
        let titles: Vec<&str> = steps.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Copy the site's certificate",
                "Copy its key, for the scannerpulse account alone",
                "Restart the web service, to serve the certificate"
            ]
        );
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
        assert!(titles.contains(&"Take the first full backup"));
        assert_eq!(titles.last(), Some(&"Read the new units"));
    }
}
