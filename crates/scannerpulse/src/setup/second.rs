//! The second ScannerPulse machine: it joins last, through the first (the
//! hub), which sends it its settings, its database password file and its
//! certificate over SSH. It serves the same site from the same archive (a
//! network share both mount at the same path, which the first machine
//! marked), and the two share an address when the plan names one.

use std::path::Path;

use super::plan::Plan;
use super::steps::{
    ACCOUNT, Action, Step, account_step, cert_path, env_path, folder_steps, key_path, pgpass_path,
    run, settings_folder_steps, start_steps,
};
use super::system::{Family, System};

/// The mark the first machine leaves in the archive: the install's id.
pub const MARK: &str = ".scannerpulse-install";

/// The archive (`$ARCHIVE`) is a network share, which both machines mount:
/// a folder of this machine's own would be cut off when it is down.
pub const SHARE_CHECK: &str = r#"kind=$(stat -f -c %T "$ARCHIVE") || exit 1
case "$kind" in
  nfs*|cifs|smb*|ceph) ;;
  *) echo "the archive $ARCHIVE is not a network share ($kind): with a second machine, both mount the same share (NFS or SMB) at that path" >&2; exit 1 ;;
esac"#;

/// What the first machine sends the second: its files, as they are.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HubFiles {
    pub settings: String,
    pub pgpass: String,
    pub cert: String,
    pub key: String,
}

/// Never their contents.
impl std::fmt::Debug for HubFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubFiles").finish_non_exhaustive()
    }
}

impl HubFiles {
    /// The first machine's files, read where setup wrote them.
    pub fn read() -> Result<HubFiles, String> {
        let read = |path: std::path::PathBuf| {
            std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
        };
        Ok(HubFiles {
            settings: read(env_path())?,
            pgpass: read(pgpass_path())?,
            cert: read(cert_path())?,
            key: read(key_path())?,
        })
    }
}

/// The settings file for the second machine: the first's, with this
/// machine's own name, and not the preferred filer.
pub fn settings_for(settings: &str, name: &str) -> Result<String, String> {
    let text = crate::hostfile::changed(settings, "SCANNERPULSE_HOST", Some(name))?;
    crate::hostfile::changed(&text, "SCANNERPULSE_PREFERRED", Some("false"))
}

/// The site's port, from the settings file.
pub fn port_of(settings: &str) -> Result<u16, String> {
    crate::hostfile::parse(settings)?
        .into_iter()
        .find(|(name, _)| name == "SCANNERPULSE_WEB_LISTEN")
        .and_then(|(_, listen)| listen.parse::<std::net::SocketAddr>().ok())
        .map(|listen| listen.port())
        .ok_or_else(|| "the first machine's settings name no port".into())
}

/// The first machine's check, before anything is changed, that the archive
/// is a network share.
pub fn share_check(archive: &Path) -> Step {
    super::node::script(
        "Check that the archive is a network share, for the second machine",
        SHARE_CHECK,
        &[("ARCHIVE", &archive.display().to_string())],
        None,
    )
}

/// The first machine's last steps for its second machine, `second`: hold
/// the shared address with it, and let it join.
pub fn first_steps(
    system: &System,
    hub: &str,
    second: &str,
    shared: Option<&str>,
    port: u16,
) -> Vec<Step> {
    let mut steps = Vec::new();
    if let Some(shared) = shared {
        steps.extend(super::keepalived::steps(
            system,
            shared,
            second,
            true,
            super::keepalived::router_id(hub, second),
            port,
        ));
    }
    steps.push(Step {
        title: "Let the second ScannerPulse machine join".into(),
        action: Action::Ready,
    });
    steps
}

/// The steps that add `second` (sharing `shared`) to the install of `plan`,
/// on a first machine already set up. The database machines join again,
/// to let the second in, and then the second joins.
pub fn add_steps(
    plan: &Plan,
    second: &str,
    shared: Option<&str>,
    archive: &Path,
    recordings: &Path,
    system: &System,
    port: u16,
) -> Vec<Step> {
    let mut steps = vec![
        share_check(archive),
        Step {
            title: "Add the second ScannerPulse machine to the plan".into(),
            action: Action::Plan {
                nodes: plan.nodes.clone(),
                hub: plan.hub.clone(),
                password: plan.password.clone(),
                referee: plan.referee.clone(),
                second: Some(second.into()),
                shared: shared.map(String::from),
                archive: archive.into(),
                recordings: recordings.into(),
            },
        },
    ];
    steps.extend(first_steps(system, &plan.hub, second, shared, port));
    steps
}

/// The steps that make this machine the second ScannerPulse machine of
/// `plan`, named `name`, from the first machine's `files`.
pub fn steps(
    plan: &Plan,
    files: &HubFiles,
    system: &System,
    binary: &Path,
    name: &str,
) -> Result<Vec<Step>, String> {
    let port = port_of(&files.settings)?;
    let (recordings, archive) = (Path::new(&plan.recordings), Path::new(&plan.archive));
    let mut steps = Vec::new();
    match system.family {
        Family::Apt => {
            let wait = "DPkg::Lock::Timeout=300";
            steps.push(run(
                "Read the package lists",
                "apt-get",
                &["-o", wait, "update"],
            ));
            steps.push(run(
                "Install the tool for folders' access lists",
                "apt-get",
                &["-o", wait, "install", "-y", "acl", "curl"],
            ));
        }
        Family::Dnf => steps.push(run(
            "Install the tool for folders' access lists",
            "dnf",
            &["install", "-y", "acl", "curl"],
        )),
    }
    steps.push(super::node::script(
        "Check that the archive is the first machine's share",
        &format!(
            "{SHARE_CHECK}\n[ \"$(cat \"$ARCHIVE/$MARK\" 2>/dev/null)\" = \"$ID\" ] || {{ echo \"the archive $ARCHIVE is not the first machine's: mount the first machine's archive share at that path\" >&2; exit 1; }}"
        ),
        &[
            ("ARCHIVE", &plan.archive),
            ("MARK", MARK),
            ("ID", &plan.install_id),
        ],
        None,
    ));
    steps.push(account_step());
    steps.extend(folder_steps(recordings, archive));
    steps.extend(settings_folder_steps());
    let own = format!("{ACCOUNT}:{ACCOUNT}");
    for (title, path, contents, mode, owner) in [
        (
            "Write the database's password file",
            pgpass_path(),
            files.pgpass.clone(),
            0o600,
            own.clone(),
        ),
        (
            "Write this machine's settings file",
            env_path(),
            settings_for(&files.settings, name)?,
            0o640,
            format!("root:{ACCOUNT}"),
        ),
        (
            "Copy the site's certificate",
            cert_path(),
            files.cert.clone(),
            0o644,
            own.clone(),
        ),
        (
            "Copy its key, for the scannerpulse account alone",
            key_path(),
            files.key.clone(),
            0o600,
            own.clone(),
        ),
    ] {
        steps.push(Step {
            title: title.into(),
            action: Action::Write {
                path,
                contents,
                mode,
                owner,
            },
        });
    }
    steps.extend(start_steps(
        binary, system, recordings, archive, false, false, port,
    ));
    if let Some(shared) = &plan.shared {
        steps.extend(super::keepalived::steps(
            system,
            shared,
            &plan.hub,
            false,
            super::keepalived::router_id(&plan.hub, name),
            port,
        ));
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            hub: "192.0.2.10".into(),
            hub_key: "ssh-ed25519 AAAA hub".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            database: "scannerpulse".into(),
            role: "scannerpulse".into(),
            password: "app-secret-1".into(),
            replication_password: "copy-secret-2".into(),
            referee: None,
            monitor_password: String::new(),
            second: Some("web2.example.org".into()),
            shared: Some("192.0.2.50/24".into()),
            install_id: "0123456789abcdef".into(),
            archive: "/srv/archive".into(),
            recordings: "/srv/recordings".into(),
            ready: true,
            joined: Vec::new(),
        }
    }

    fn files() -> HubFiles {
        HubFiles {
            settings: "DATABASE_URL=postgres://scannerpulse@db1.example.org:5432/scannerpulse?sslmode=require\nSCANNERPULSE_HOST=web1\nSCANNERPULSE_PREFERRED=true\nSCANNERPULSE_WEB_LISTEN=0.0.0.0:8443\n".into(),
            pgpass: "*:5432:scannerpulse:scannerpulse:app-secret-1\n".into(),
            cert: "-----BEGIN CERTIFICATE-----\n".into(),
            key: "-----BEGIN PRIVATE KEY----- key-secret\n".into(),
        }
    }

    #[test]
    fn the_second_machine_serves_the_same_site_under_its_own_name() {
        let settings = settings_for(&files().settings, "web2").unwrap();
        assert!(
            settings.contains("SCANNERPULSE_HOST=\"web2\"\n"),
            "{settings}"
        );
        assert!(
            settings.contains("SCANNERPULSE_PREFERRED=\"false\"\n"),
            "{settings}"
        );
        assert!(settings.contains("db1.example.org:5432"), "{settings}");
        assert_eq!(port_of(&files().settings), Ok(8443));
        assert!(!format!("{:?}", files()).contains("secret"));
    }

    #[test]
    fn the_second_machine_checks_the_archive_then_starts_and_shares_the_address() {
        let system =
            crate::setup::system::detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let all = steps(
            &plan(),
            &files(),
            &system,
            Path::new("/usr/local/bin/scannerpulse"),
            "web2",
        )
        .unwrap();
        let titles: Vec<&str> = all.iter().map(|s| s.title.as_str()).collect();
        let at = |title: &str| {
            titles
                .iter()
                .position(|t| *t == title)
                .unwrap_or_else(|| panic!("{title}: {titles:?}"))
        };
        let check = "Check that the archive is the first machine's share";
        assert!(at(check) < at("Make the scannerpulse account"));
        let Action::Run { args, env, .. } = &all[at(check)].action else {
            panic!()
        };
        assert!(args[1].contains("stat -f -c %T"), "{}", args[1]);
        assert!(env.contains(&("ID".into(), "0123456789abcdef".into())));
        assert!(at("Write this machine's settings file") < at("Start ScannerPulse at boot"));
        assert!(
            at("Wait for the web interface")
                < at("Write keepalived's settings for the shared address")
        );
        let Action::Write { mode, .. } =
            &all[at("Copy its key, for the scannerpulse account alone")].action
        else {
            panic!()
        };
        assert_eq!(*mode, 0o600);
        // No database of its own, no plan, no first admin.
        for title in [
            "Make the database and its role",
            "Add the first admin account",
            "Make the database's tables",
        ] {
            assert!(!titles.contains(&title), "{title}");
        }
        assert!(!format!("{all:?}").contains("secret"));
    }

    #[test]
    fn a_second_machine_added_to_an_install_set_up_checks_the_share_plans_it_and_shares_the_address()
     {
        use crate::setup::system::detect;
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let kept = Plan {
            second: None,
            shared: None,
            ..plan()
        };
        let all = add_steps(
            &kept,
            "web2.example.org",
            Some("192.0.2.50/24"),
            Path::new("/srv/archive"),
            Path::new("/srv/recordings"),
            &system,
            8443,
        );
        let titles: Vec<&str> = all.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(
            titles[0],
            "Check that the archive is a network share, for the second machine"
        );
        let Action::Plan {
            nodes,
            password,
            second,
            shared,
            archive,
            ..
        } = &all[1].action
        else {
            panic!("{titles:?}")
        };
        assert_eq!(*nodes, kept.nodes);
        assert_eq!(*password, kept.password);
        assert_eq!(second.as_deref(), Some("web2.example.org"));
        assert_eq!(shared.as_deref(), Some("192.0.2.50/24"));
        assert_eq!(archive, Path::new("/srv/archive"));
        assert!(
            titles.contains(&"Write keepalived's settings for the shared address"),
            "{titles:?}"
        );
        assert!(matches!(all.last().unwrap().action, Action::Ready));
    }
}
