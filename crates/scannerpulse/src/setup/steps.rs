//! What setup does, as a list of steps decided before any of them runs: the
//! summary shows them, and `apply` runs them in order. Every step can run
//! again safely, so a setup stopped half-way is finished by running it again.

use std::path::{Path, PathBuf};

use super::system::{Family, System};
use crate::backup::{PGBACKREST_CONF, STANZA};

/// Where setup keeps this machine's files.
pub const ETC: &str = "/etc/scannerpulse";
/// The account the services run as.
pub const ACCOUNT: &str = "scannerpulse";
/// The database, its owner role and its port.
pub const DATABASE: &str = "scannerpulse";
pub const PORT_PG: u16 = 5432;

/// The Simple questions' answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    /// Where the recorder saves new recordings (ingest reads, then moves them).
    pub recordings: PathBuf,
    /// The permanent archive.
    pub archive: PathBuf,
    pub time_zone: String,
    pub admin: String,
    pub password: String,
    pub transcription: Option<Transcription>,
    /// The HTTPS port.
    pub port: u16,
    /// This machine's name, for the list of open pages and the certificate.
    pub host: String,
    /// The names and addresses the certificate covers.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcription {
    /// The server's full URL, ending `/v1/audio/transcriptions`.
    pub url: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Runs `program`; `input` goes to its standard input, so a password is
    /// never in its arguments; `env` is added to its environment. Skipped when
    /// `unless` exists: the work is already done.
    Run {
        program: String,
        args: Vec<String>,
        input: Option<String>,
        env: Vec<(String, String)>,
        unless: Option<PathBuf>,
    },
    /// Writes a file with its mode and owner (`user:group`).
    Write {
        path: PathBuf,
        contents: String,
        mode: u32,
        owner: String,
    },
    /// Records the archive as the primary location and the recordings folder
    /// as the source, in the new database.
    Folders {
        recordings: PathBuf,
        archive: PathBuf,
    },
    /// Adds the first admin, or gives it this password when a database kept
    /// by `uninstall` has it already.
    Admin { login: String, password: String },
    /// Waits until `web` answers on this machine.
    WaitForWeb { port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub action: Action,
}

fn run(title: &str, program: &str, args: &[&str]) -> Step {
    Step {
        title: title.to_string(),
        action: Action::Run {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            input: None,
            env: Vec::new(),
            unless: None,
        },
    }
}

/// The paths setup writes, under `ETC`.
pub fn env_path() -> PathBuf {
    Path::new(ETC).join("scannerpulse.env")
}
pub fn pgpass_path() -> PathBuf {
    Path::new(ETC).join("pgpass")
}
pub fn cert_path() -> PathBuf {
    Path::new(ETC).join("tls/cert.pem")
}
pub fn key_path() -> PathBuf {
    Path::new(ETC).join("tls/key.pem")
}
/// The backups' folder inside the archive; the dot keeps it from ever
/// reading as a date folder.
pub fn backups_path(archive: &Path) -> PathBuf {
    archive.join(".scannerpulse-backups")
}

/// pgBackRest's settings: its repository in the backups folder, two full
/// backups kept (with the differentials and the WAL between them).
pub fn pgbackrest_conf(archive: &Path, system: &System) -> String {
    format!(
        "# Written by `scannerpulse setup`.\n\
         [global]\n\
         repo1-path={repo}\n\
         repo1-retention-full=2\n\
         start-fast=y\n\
         log-level-file=off\n\
         \n\
         [{STANZA}]\n\
         pg1-path={data}\n\
         pg1-port={PORT_PG}\n",
        repo = backups_path(archive).join("pgbackrest").display(),
        data = system.postgres_data(),
    )
}

/// The nightly backup's service and timer, in that order.
pub fn backup_units(binary: &Path, system: &System) -> (String, String) {
    (
        format!(
            "# Written by `scannerpulse setup`.\n\
             [Unit]\n\
             Description=ScannerPulse nightly backups (pgBackRest and a dump)\n\
             After={pg}.service\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             EnvironmentFile={env}\n\
             ExecStart={binary} backup\n",
            pg = system.postgres_service(),
            env = env_path().display(),
            binary = binary.display(),
        ),
        "# Written by `scannerpulse setup`.\n\
         [Unit]\n\
         Description=ScannerPulse nightly backups\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* 03:17:00\n\
         Persistent=true\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n"
            .to_string(),
    )
}

/// The settings file's variables for these answers.
pub fn env_pairs(answers: &Answers) -> Vec<(&'static str, String)> {
    vec![
        (
            "DATABASE_URL",
            format!("postgres://{ACCOUNT}@localhost:{PORT_PG}/{DATABASE}"),
        ),
        ("PGPASSFILE", pgpass_path().display().to_string()),
        ("SCANNERPULSE_HOST", answers.host.clone()),
        ("SCANNERPULSE_PREFERRED", "true".into()),
        (
            "SCANNERPULSE_WEB_LISTEN",
            format!("0.0.0.0:{}", answers.port),
        ),
        ("SCANNERPULSE_TLS_CERT", cert_path().display().to_string()),
        ("SCANNERPULSE_TLS_KEY", key_path().display().to_string()),
        (
            "SCANNERPULSE_BACKUPS",
            backups_path(&answers.archive).display().to_string(),
        ),
    ]
}

/// The systemd unit of `command` (`web` or `ingest`).
pub fn unit(command: &str, binary: &Path, system: &System) -> String {
    let (what, extra) = match command {
        "web" => (
            "the web interface",
            // Port 443 without root.
            "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n",
        ),
        // Ingest releases the processing lease before it stops.
        _ => ("ingest: filing and transcription", "TimeoutStopSec=150\n"),
    };
    let pg = system.postgres_service();
    format!(
        "# Written by `scannerpulse setup`.\n\
         [Unit]\n\
         Description=ScannerPulse {what}\n\
         Wants=network-online.target\n\
         After=network-online.target {pg}.service\n\
         \n\
         [Service]\n\
         User={ACCOUNT}\n\
         Group={ACCOUNT}\n\
         EnvironmentFile={env}\n\
         ExecStart={binary} {command}\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         NoNewPrivileges=true\n\
         ProtectSystem=full\n\
         {extra}\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n",
        env = env_path().display(),
        binary = binary.display(),
    )
}

/// The SQL that makes the role and the database, run by the `postgres`
/// account; it does nothing that is already done.
fn database_sql(password: &str) -> String {
    let quoted = password.replace('\'', "''");
    format!(
        "SELECT format('CREATE ROLE {ACCOUNT} LOGIN PASSWORD %L', '{quoted}') \
         WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '{ACCOUNT}') \\gexec\n\
         ALTER ROLE {ACCOUNT} PASSWORD '{quoted}';\n\
         SELECT 'CREATE DATABASE {DATABASE} OWNER {ACCOUNT} TEMPLATE template0 ENCODING ''UTF8'' \
         LC_COLLATE ''C.UTF-8'' LC_CTYPE ''C.UTF-8''' \
         WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '{DATABASE}') \\gexec\n"
    )
}

/// Every step, in order, for `answers` on `system`. `binary` is this
/// program; `db_password` is the database role's new password.
pub fn steps(answers: &Answers, system: &System, binary: &Path, db_password: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    match system.family {
        Family::Apt => {
            steps.push(run("Read the package lists", "apt-get", &["update"]));
            steps.push(run(
                "Install the PostgreSQL project's repository tool",
                "apt-get",
                &[
                    "install",
                    "-y",
                    "postgresql-common",
                    "ca-certificates",
                    "acl",
                ],
            ));
            steps.push(run(
                "Add the PostgreSQL project's repository",
                "/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh",
                &["-y"],
            ));
            steps.push(run(
                "Install PostgreSQL 17 and pgBackRest",
                "apt-get",
                &["install", "-y", "postgresql-17", "pgbackrest"],
            ));
        }
        Family::Dnf => {
            let rpm = system
                .pgdg_rpm()
                .expect("the Red Hat family has the repository's package");
            steps.push(run(
                "Add the PostgreSQL project's repository",
                "dnf",
                &["install", "-y", &rpm],
            ));
            if system.id != "fedora" {
                // The system's own PostgreSQL module would shadow version 17.
                steps.push(run(
                    "Turn off the system's own PostgreSQL module",
                    "dnf",
                    &["-qy", "module", "disable", "postgresql"],
                ));
            }
            steps.push(run(
                "Install PostgreSQL 17 and pgBackRest",
                "dnf",
                &["install", "-y", "postgresql17-server", "pgbackrest", "acl"],
            ));
            steps.push(Step {
                title: "Make PostgreSQL 17's data directory".into(),
                action: Action::Run {
                    program: "/usr/pgsql-17/bin/postgresql-17-setup".into(),
                    args: vec!["initdb".into()],
                    input: None,
                    env: vec![(
                        "PGSETUP_INITDB_OPTIONS".into(),
                        "--auth-host=scram-sha-256 --auth-local=peer".into(),
                    )],
                    unless: Some(Path::new(system.postgres_data()).join("PG_VERSION")),
                },
            });
        }
    }
    steps.push(run(
        "Start PostgreSQL 17, now and at boot",
        "systemctl",
        &["enable", "--now", system.postgres_service()],
    ));
    steps.push(run(
        "Make the scannerpulse account",
        "/bin/sh",
        &[
            "-c",
            "id -u scannerpulse >/dev/null 2>&1 || useradd --system --user-group \
             --home-dir /var/lib/scannerpulse --create-home --shell /usr/sbin/nologin scannerpulse",
        ],
    ));
    steps.push(Step {
        title: "Make the database and its role".into(),
        action: Action::Run {
            program: "runuser".into(),
            args: [
                "-u",
                "postgres",
                "--",
                "psql",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
            ]
            .map(String::from)
            .to_vec(),
            input: Some(database_sql(db_password)),
            env: Vec::new(),
            unless: None,
        },
    });
    for (folder, role) in [
        (&answers.recordings, "recordings"),
        (&answers.archive, "archive"),
    ] {
        let path = folder.display().to_string();
        steps.push(run(
            &format!("Let the scannerpulse account read and write the {role} folder"),
            "setfacl",
            &[
                "-R",
                "-m",
                "u:scannerpulse:rwX",
                "-m",
                "d:u:scannerpulse:rwX",
                &path,
            ],
        ));
    }
    steps.push(run(
        "Make the settings folder",
        "install",
        &["-d", "-m", "0750", "-o", "root", "-g", ACCOUNT, ETC],
    ));
    steps.push(run(
        "Make the certificate folder",
        "install",
        &[
            "-d",
            "-m",
            "0750",
            "-o",
            ACCOUNT,
            "-g",
            ACCOUNT,
            &format!("{ETC}/tls"),
        ],
    ));
    steps.push(Step {
        title: "Write the database's password file".into(),
        action: Action::Write {
            path: pgpass_path(),
            contents: format!("localhost:{PORT_PG}:{DATABASE}:{ACCOUNT}:{db_password}\n"),
            mode: 0o600,
            owner: format!("{ACCOUNT}:{ACCOUNT}"),
        },
    });
    let pairs = env_pairs(answers);
    let rendered: Vec<(&str, &str)> = pairs.iter().map(|(n, v)| (*n, v.as_str())).collect();
    steps.push(Step {
        title: "Write this machine's settings file".into(),
        action: Action::Write {
            path: env_path(),
            contents: crate::hostfile::render(&rendered)
                .expect("setup's own values hold no line break"),
            mode: 0o640,
            owner: format!("root:{ACCOUNT}"),
        },
    });
    let mut cert_args = vec![
        "make".to_string(),
        "--cert".into(),
        cert_path().display().to_string(),
        "--key".into(),
        key_path().display().to_string(),
    ];
    cert_args.extend(answers.names.iter().cloned());
    let me = binary.display().to_string();
    steps.push(Step {
        title: "Make this machine's certificate".into(),
        action: Action::Run {
            program: me.clone(),
            args: std::iter::once("cert".to_string())
                .chain(cert_args)
                .collect(),
            input: None,
            env: Vec::new(),
            // Kept on a second run: a new one would make browsers warn again.
            unless: Some(key_path()),
        },
    });
    let key = key_path().display().to_string();
    steps.push(run(
        "Give the certificate's key to the scannerpulse account",
        "chown",
        &[&format!("{ACCOUNT}:{ACCOUNT}"), &key],
    ));
    let with_db: Vec<(String, String)> = pairs
        .iter()
        .map(|(n, v)| (n.to_string(), v.clone()))
        .collect();
    let bin = |title: &str, args: &[&str], input: Option<String>| Step {
        title: title.to_string(),
        action: Action::Run {
            program: me.clone(),
            args: args.iter().map(|a| a.to_string()).collect(),
            input,
            env: with_db.clone(),
            unless: None,
        },
    };
    let mut more = vec![
        bin("Make the database's tables", &["db-update"], None),
        bin(
            "Set the site's time zone",
            &[
                "settings",
                "set",
                "time_zone",
                &serde_json::json!(answers.time_zone).to_string(),
            ],
            None,
        ),
    ];
    if let Some(t) = &answers.transcription {
        let servers = serde_json::json!([{ "url": t.url, "weight": 1 }]).to_string();
        let model = serde_json::json!(t.model).to_string();
        more.push(bin(
            "Set the transcription server",
            &["settings", "set", "transcription.servers", &servers],
            None,
        ));
        more.push(bin(
            "Set the transcription model",
            &["settings", "set", "transcription.model", &model],
            None,
        ));
        more.push(bin(
            "Turn transcription on",
            &["settings", "set", "transcription.enabled", "true"],
            None,
        ));
    }
    steps.extend(more);
    steps.push(Step {
        title: "Add the first admin account".into(),
        action: Action::Admin {
            login: answers.admin.clone(),
            password: answers.password.clone(),
        },
    });
    steps.push(Step {
        title: "Record the archive and the recordings folder".into(),
        action: Action::Folders {
            recordings: answers.recordings.clone(),
            archive: answers.archive.clone(),
        },
    });
    let backups = backups_path(&answers.archive);
    steps.push(run(
        "Make the folder of pgBackRest's backups",
        "install",
        &[
            "-d",
            "-m",
            "0750",
            "-o",
            "postgres",
            "-g",
            "postgres",
            &backups.join("pgbackrest").display().to_string(),
        ],
    ));
    steps.push(run(
        "Make the folder of the nightly dumps",
        "install",
        &[
            "-d",
            "-m",
            "0700",
            &backups.join("dump").display().to_string(),
        ],
    ));
    steps.push(run(
        "Make pgBackRest's settings folder",
        "install",
        &["-d", "-m", "0755", "/etc/pgbackrest"],
    ));
    steps.push(Step {
        title: "Write pgBackRest's settings".into(),
        action: Action::Write {
            path: PathBuf::from(PGBACKREST_CONF),
            contents: pgbackrest_conf(&answers.archive, system),
            mode: 0o640,
            owner: "root:postgres".into(),
        },
    });
    let postgres = |title: &str, args: &[&str], input: Option<String>| Step {
        title: title.to_string(),
        action: Action::Run {
            program: "runuser".into(),
            args: ["-u", "postgres", "--"]
                .iter()
                .chain(args)
                .map(|a| a.to_string())
                .collect(),
            input,
            env: Vec::new(),
            unless: None,
        },
    };
    let config = format!("--config={PGBACKREST_CONF}");
    let stanza = format!("--stanza={STANZA}");
    steps.push(postgres(
        "Send PostgreSQL's WAL to pgBackRest",
        &["psql", "-q", "-v", "ON_ERROR_STOP=1"],
        Some(format!(
            "ALTER SYSTEM SET archive_mode = 'on';\n\
             ALTER SYSTEM SET archive_command = 'pgbackrest {config} {stanza} archive-push %p';\n"
        )),
    ));
    steps.push(run(
        "Restart PostgreSQL 17, to start sending its WAL",
        "systemctl",
        &["restart", system.postgres_service()],
    ));
    steps.push(postgres(
        "Make pgBackRest's place for this database",
        &["pgbackrest", &config, &stanza, "stanza-create"],
        None,
    ));
    steps.push(postgres(
        "Take the first full backup",
        &["pgbackrest", &config, &stanza, "backup", "--type=full"],
        None,
    ));
    let (service, timer) = backup_units(binary, system);
    for (name, contents) in [
        ("scannerpulse-backup.service", service),
        ("scannerpulse-backup.timer", timer),
    ] {
        steps.push(Step {
            title: format!("Write {name}"),
            action: Action::Write {
                path: Path::new("/etc/systemd/system").join(name),
                contents,
                mode: 0o644,
                owner: "root:root".into(),
            },
        });
    }
    for command in ["web", "ingest"] {
        steps.push(Step {
            title: format!("Write the {command} service"),
            action: Action::Write {
                path: PathBuf::from(format!(
                    "/etc/systemd/system/scannerpulse-{command}.service"
                )),
                contents: unit(command, binary, system),
                mode: 0o644,
                owner: "root:root".into(),
            },
        });
    }
    steps.push(run(
        "Read the new services",
        "systemctl",
        &["daemon-reload"],
    ));
    steps.push(run(
        "Start ScannerPulse, now and at boot",
        "systemctl",
        &[
            "enable",
            "--now",
            "scannerpulse-web",
            "scannerpulse-ingest",
            "scannerpulse-backup.timer",
        ],
    ));
    steps.push(Step {
        title: "Wait for the web interface".into(),
        action: Action::WaitForWeb { port: answers.port },
    });
    steps
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Answers, Transcription, backup_units, env_pairs, pgbackrest_conf, steps, unit,
    };
    use crate::setup::system::detect;
    use std::path::{Path, PathBuf};

    fn answers() -> Answers {
        Answers {
            recordings: PathBuf::from("/srv/recordings"),
            archive: PathBuf::from("/srv/archive"),
            time_zone: "America/Chicago".into(),
            admin: "ann".into(),
            password: "a long pass phrase".into(),
            transcription: Some(Transcription {
                url: "http://192.0.2.20:8000/v1/audio/transcriptions".into(),
                model: "Systran/faster-whisper-medium.en".into(),
            }),
            port: 443,
            host: "radio-a".into(),
            names: vec!["radio-a".into(), "192.0.2.10".into()],
        }
    }

    const DEBIAN: &str = "ID=debian\nVERSION_ID=\"13\"\n";
    const ROCKY: &str = "ID=\"rocky\"\nVERSION_ID=\"9.6\"\n";

    #[test]
    fn the_steps_install_postgresql_the_way_the_system_does_then_build_the_site() {
        let bin = Path::new("/usr/local/bin/scannerpulse");
        let titles = |s: &str| -> Vec<String> {
            steps(&answers(), &detect(s, "x86_64").unwrap(), bin, "pw")
                .into_iter()
                .map(|s| s.title)
                .collect()
        };
        let debian = titles(DEBIAN);
        assert_eq!(debian[0], "Read the package lists");
        assert!(debian.contains(&"Add the PostgreSQL project's repository".to_string()));
        let rocky = titles(ROCKY);
        assert!(rocky.contains(&"Turn off the system's own PostgreSQL module".to_string()));
        assert!(rocky.contains(&"Make PostgreSQL 17's data directory".to_string()));
        assert!(
            !titles("ID=fedora\nVERSION_ID=42\n")
                .contains(&"Turn off the system's own PostgreSQL module".to_string())
        );
        for list in [&debian, &rocky] {
            let tail: Vec<&str> = list
                .iter()
                .rev()
                .take(3)
                .rev()
                .map(String::as_str)
                .collect();
            assert_eq!(
                tail,
                [
                    "Read the new services",
                    "Start ScannerPulse, now and at boot",
                    "Wait for the web interface"
                ]
            );
            let db = list
                .iter()
                .position(|t| t == "Make the database and its role")
                .unwrap();
            let tables = list
                .iter()
                .position(|t| t == "Make the database's tables")
                .unwrap();
            let admin = list
                .iter()
                .position(|t| t == "Add the first admin account")
                .unwrap();
            assert!(db < tables && tables < admin);
        }
    }

    #[test]
    fn a_password_travels_only_on_standard_input_or_into_its_private_file() {
        let all = steps(
            &answers(),
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/b"),
            "db secret",
        );
        for step in &all {
            if let Action::Run { args, .. } = &step.action {
                let line = args.join(" ");
                assert!(
                    !line.contains("a long pass phrase") && !line.contains("db secret"),
                    "{}: {line}",
                    step.title
                );
            }
        }
        let admin = all
            .iter()
            .find(|s| s.title == "Add the first admin account")
            .unwrap();
        let Action::Admin { password, .. } = &admin.action else {
            panic!()
        };
        assert_eq!(password, "a long pass phrase");
        let pgpass = all
            .iter()
            .find(|s| s.title == "Write the database's password file")
            .unwrap();
        let Action::Write { contents, mode, .. } = &pgpass.action else {
            panic!()
        };
        assert_eq!(
            (contents.as_str(), *mode),
            (
                "localhost:5432:scannerpulse:scannerpulse:db secret\n",
                0o600
            )
        );
    }

    #[test]
    fn without_transcription_no_transcription_setting_is_made() {
        let mut a = answers();
        a.transcription = None;
        let all = steps(
            &a,
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/b"),
            "pw",
        );
        assert!(!all.iter().any(|s| s.title.contains("transcription")));
    }

    #[test]
    fn the_settings_file_and_the_services_say_where_everything_is() {
        let pairs = env_pairs(&answers());
        let get = |n: &str| pairs.iter().find(|(k, _)| *k == n).map(|(_, v)| v.as_str());
        assert_eq!(get("SCANNERPULSE_WEB_LISTEN"), Some("0.0.0.0:443"));
        assert_eq!(
            get("SCANNERPULSE_TLS_KEY"),
            Some("/etc/scannerpulse/tls/key.pem")
        );
        assert_eq!(
            get("DATABASE_URL"),
            Some("postgres://scannerpulse@localhost:5432/scannerpulse")
        );
        let system = detect(ROCKY, "x86_64").unwrap();
        let web = unit("web", Path::new("/usr/local/bin/scannerpulse"), &system);
        assert!(web.contains("User=scannerpulse\n"));
        assert!(web.contains("ExecStart=/usr/local/bin/scannerpulse web\n"));
        assert!(web.contains("AmbientCapabilities=CAP_NET_BIND_SERVICE\n"));
        assert!(web.contains("After=network-online.target postgresql-17.service\n"));
        assert!(web.contains("EnvironmentFile=/etc/scannerpulse/scannerpulse.env\n"));
        let ingest = unit("ingest", Path::new("/usr/local/bin/scannerpulse"), &system);
        assert!(
            ingest.contains("TimeoutStopSec=150\n") && !ingest.contains("CAP_NET_BIND_SERVICE")
        );
    }

    #[test]
    fn the_backups_go_inside_the_archive_and_postgresql_sends_its_wal_to_pgbackrest() {
        let pairs = env_pairs(&answers());
        assert!(pairs.contains(&(
            "SCANNERPULSE_BACKUPS",
            "/srv/archive/.scannerpulse-backups".to_string()
        )));
        let debian = detect(DEBIAN, "aarch64").unwrap();
        let conf = pgbackrest_conf(Path::new("/srv/archive"), &debian);
        assert!(conf.contains("repo1-path=/srv/archive/.scannerpulse-backups/pgbackrest\n"));
        assert!(conf.contains("[scannerpulse]\npg1-path=/var/lib/postgresql/17/main\n"));
        let rocky = detect(ROCKY, "x86_64").unwrap();
        assert!(
            pgbackrest_conf(Path::new("/srv/archive"), &rocky)
                .contains("pg1-path=/var/lib/pgsql/17/data\n")
        );
        let (service, timer) = backup_units(Path::new("/usr/local/bin/scannerpulse"), &rocky);
        assert!(service.contains("ExecStart=/usr/local/bin/scannerpulse backup\n"));
        assert!(
            !service.contains("User="),
            "the backup runs as root, for pgBackRest's runuser"
        );
        assert!(timer.contains("OnCalendar=*-*-* 03:17:00\nPersistent=true\n"));
        let all = steps(
            &answers(),
            &debian,
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let at = |title: &str| all.iter().position(|s| s.title == title).unwrap();
        let wal = at("Send PostgreSQL's WAL to pgBackRest");
        let Action::Run {
            input: Some(sql), ..
        } = &all[wal].action
        else {
            panic!()
        };
        assert!(sql.contains(
            "archive_command = 'pgbackrest --config=/etc/pgbackrest/scannerpulse.conf --stanza=scannerpulse archive-push %p'"
        ));
        assert!(wal < at("Restart PostgreSQL 17, to start sending its WAL"));
        assert!(
            at("Restart PostgreSQL 17, to start sending its WAL")
                < at("Make pgBackRest's place for this database")
        );
        assert!(at("Make pgBackRest's place for this database") < at("Take the first full backup"));
        let Action::Run { args, .. } = &all[at("Start ScannerPulse, now and at boot")].action
        else {
            panic!()
        };
        assert!(args.contains(&"scannerpulse-backup.timer".to_string()));
    }
}
