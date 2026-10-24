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

/// This machine is a second ScannerPulse machine: its settings say it is not
/// the preferred one, and it holds no plan (the first machine does).
pub fn is_second(settings: &str, plan_here: bool) -> bool {
    !plan_here
        && crate::hostfile::parse(settings).is_ok_and(|pairs| {
            pairs
                .iter()
                .any(|(name, value)| name == "SCANNERPULSE_PREFERRED" && value == "false")
        })
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
    match shared {
        Some(shared) => steps.extend(super::keepalived::steps(
            system,
            shared,
            second,
            true,
            super::keepalived::router_id(hub, second),
            port,
        )),
        None => steps.push(super::keepalived::drop_step()),
    }
    steps.push(Step {
        title: "Let the second ScannerPulse machine join".into(),
        action: Action::Ready,
    });
    steps.push(Step {
        title: "Wait for the second ScannerPulse machine to join".into(),
        action: Action::WaitForSecond,
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
                referee_port: plan.referee_port,
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

/// The settings file's database address.
fn database_url(settings: &str) -> Option<String> {
    crate::hostfile::parse(settings)
        .ok()?
        .into_iter()
        .find(|(name, _)| name == "DATABASE_URL")
        .map(|(_, url)| url)
}

/// The steps that make this machine the second ScannerPulse machine of
/// `plan`, named `name`, from the first machine's `files`.
pub fn steps(
    plan: &Plan,
    files: &HubFiles,
    system: &System,
    binary: &Path,
    name: &str,
    here: Option<&str>,
    hub: &[std::net::IpAddr],
) -> Result<Vec<Step>, String> {
    // `here`: this machine's settings, when it already runs a site. Joining
    // again keeps it; a site of another install's is not taken over.
    if let Some(here) = here
        && database_url(here) != database_url(&files.settings)
    {
        return Err("this machine runs a site of its own, with another database: uninstall it first (`sudo /usr/local/bin/scannerpulse uninstall`), then join".into());
    }
    let port = port_of(&files.settings)?;
    let (recordings, archive) = (Path::new(&plan.recordings), Path::new(&plan.archive));
    // Checked before anything is installed or written: a refusal leaves
    // nothing behind. The mark is read as root, which a share that squashes
    // root reads as nobody.
    let mut steps = vec![super::node::script(
        "Check that the archive is the first machine's share",
        &format!(
            "{SHARE_CHECK}\nmark=$(cat \"$ARCHIVE/$MARK\" 2>/dev/null) || {{ echo \"cannot read $ARCHIVE/$MARK here as root: mount the first machine's archive share at that path (a share that squashes root must let others read the archive's top folder)\" >&2; exit 1; }}\n[ \"$mark\" = \"$ID\" ] || {{ echo \"the archive $ARCHIVE is another install's (its mark is not this install's): mount the first machine's archive share at that path\" >&2; exit 1; }}"
        ),
        &[
            ("ARCHIVE", &plan.archive),
            ("MARK", MARK),
            ("ID", &plan.install_id),
        ],
        None,
    )];
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
            &["install", "-y", "acl", "/usr/bin/curl"],
        )),
    }
    steps.push(account_step());
    steps.extend(folder_steps(recordings, archive, true));
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
        (
            "Write the install's layout, for the admin page",
            std::path::PathBuf::from(crate::layout::PATH),
            serde_json::to_string_pretty(&plan.layout()).map_err(|e| e.to_string())?,
            0o644,
            "root:root".into(),
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
    // `scannerpulse update` on the first machine reaches this one through
    // its key, which may run only `node serve`.
    let mut key = super::node::hub_key_step(plan, hub, binary);
    key.title = "Let the first machine update this machine's program".into();
    steps.push(key);
    // The root helper runs on the first machine only, which holds the plan.
    steps.extend(start_steps(
        binary, system, recordings, archive, false, false, false, port,
    ));
    match &plan.shared {
        Some(shared) => steps.extend(super::keepalived::steps(
            system,
            shared,
            &plan.hub,
            false,
            super::keepalived::router_id(&plan.hub, name),
            port,
        )),
        None => steps.push(super::keepalived::drop_step()),
    }
    Ok(steps)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::setup) fn plan() -> Plan {
        Plan {
            hub: "192.0.2.10".into(),
            hub_key: "ssh-ed25519 AAAA hub".into(),
            nodes: vec!["db1.example.org".into(), "db2.example.org".into()],
            database: "scannerpulse".into(),
            role: "scannerpulse".into(),
            password: "app-secret-1".into(),
            replication_password: "copy-secret-2".into(),
            referee: None,
            referee_port: None,
            monitor_password: String::new(),
            second: Some("web2.example.org".into()),
            shared: Some("192.0.2.50/24".into()),
            install_id: "0123456789abcdef".into(),
            archive: "/srv/archive".into(),
            recordings: "/srv/recordings".into(),
            ready: true,
            joined: Vec::new(),
            moving: false,
            retired: false,
        }
    }

    pub(in crate::setup) fn files() -> HubFiles {
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
            None,
            &["192.0.2.10".parse().unwrap()],
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
        // The first machine's key may then run `node serve` here, from its
        // own address only: `scannerpulse update` reaches this machine so.
        // The root helper runs on the first machine, never here.
        assert!(
            !all.iter().any(|s| matches!(&s.action, Action::Write { path, .. } if path == Path::new(crate::agent::UNIT))),
            "{titles:?}"
        );
        let key = at("Let the first machine update this machine's program");
        let Action::Run { input, .. } = &all[key].action else {
            panic!()
        };
        assert_eq!(
            input.as_deref(),
            Some(
                "restrict,from=\"192.0.2.10\",command=\"/usr/local/bin/scannerpulse node serve\" ssh-ed25519 AAAA hub\n"
            )
        );
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
        assert!(matches!(all[all.len() - 2].action, Action::Ready));
        assert!(matches!(all.last().unwrap().action, Action::WaitForSecond));
    }

    #[test]
    fn without_a_shared_address_each_machine_drops_setups_keepalived() {
        use crate::setup::system::detect;
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let dropped = |steps: &[Step]| {
            steps
                .iter()
                .any(|s| s.title == super::super::keepalived::drop_step().title)
        };
        let first = |shared| first_steps(&system, "192.0.2.10", "web2.example.org", shared, 443);
        assert!(dropped(&first(None)));
        assert!(!dropped(&first(Some("192.0.2.50/24"))));
        let second = |plan: &Plan| {
            steps(
                plan,
                &files(),
                &system,
                Path::new("/b"),
                "web2.example.org",
                None,
                &["192.0.2.10".parse().unwrap()],
            )
            .unwrap()
        };
        assert!(dropped(&second(&Plan {
            shared: None,
            ..plan()
        })));
        assert!(!dropped(&second(&plan())));
    }

    #[test]
    fn the_second_checks_the_archive_and_its_mark_before_anything_else() {
        use crate::setup::system::detect;
        use crate::setup::test_script::run;
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let all = steps(
            &plan(),
            &files(),
            &system,
            Path::new("/b"),
            "web2.example.org",
            None,
            &["192.0.2.10".parse().unwrap()],
        )
        .unwrap();
        // Before any package or account: a refusal leaves nothing behind.
        let check = &all[0];
        assert_eq!(
            check.title,
            "Check that the archive is the first machine's share"
        );
        let share = ("stat", "echo smb2");
        for (mark, ok, said) in [
            (Some("0123456789abcdef"), true, ""),
            (Some("fedcba9876543210"), false, "another install's"),
            (None, false, "cannot read"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let archive = dir.path().join("archive");
            std::fs::create_dir(&archive).unwrap();
            if let Some(mark) = mark {
                std::fs::write(archive.join(MARK), mark).unwrap();
            }
            let archive = archive.display().to_string();
            let ran = run(check, &[share], &[("ARCHIVE", &archive)], dir.path(), "");
            assert_eq!(ran.ok, ok, "{:?}: {}", mark, ran.stderr);
            assert!(ran.stderr.contains(said), "{:?}: {}", mark, ran.stderr);
        }
    }

    #[test]
    fn the_second_knows_itself_by_its_settings_and_no_plan() {
        let second = settings_for(&files().settings, "web2").unwrap();
        assert!(is_second(&second, false));
        // The first machine's settings, or any machine holding the plan.
        assert!(!is_second(&files().settings, false));
        assert!(!is_second(&second, true));
        assert!(!is_second("SCANNERPULSE_HOST=\"radio-a\"\n", false));
    }

    #[test]
    fn a_machine_with_a_site_of_its_own_is_not_made_the_second() {
        use crate::setup::system::detect;
        let system = detect("ID=debian\nVERSION_ID=\"13\"\n", "x86_64").unwrap();
        let join = |here: Option<&str>| {
            steps(
                &plan(),
                &files(),
                &system,
                Path::new("/b"),
                "web2.example.org",
                here,
                &["192.0.2.10".parse().unwrap()],
            )
        };
        // Joining again: its settings are this install's.
        let again = settings_for(&files().settings, "web2.example.org").unwrap();
        assert!(join(Some(&again)).is_ok());
        assert!(join(None).is_ok());
        let other = "DATABASE_URL=postgres://scannerpulse@other.example.org:5432/scannerpulse\n";
        let error = join(Some(other)).err().unwrap();
        assert!(error.contains("a site of its own"), "{error}");
        // It keeps the install's layout, for its admin page.
        let all = join(None).unwrap();
        let written = all
            .iter()
            .find_map(|s| match &s.action {
                Action::Write {
                    path,
                    contents,
                    mode,
                    ..
                } if s.title == "Write the install's layout, for the admin page" => {
                    Some((path.clone(), contents.clone(), *mode))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(written.0, Path::new(crate::layout::PATH));
        assert_eq!(written.2, 0o644);
        assert!(written.1.contains("web2.example.org") && !written.1.contains("secret"));
    }
}
