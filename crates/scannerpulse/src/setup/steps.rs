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
/// The web service's unit; setup writes it near the end, so it marks a
/// machine set up.
pub const WEB_UNIT: &str = "/etc/systemd/system/scannerpulse-web.service";

/// The answers to setup's questions: Simple's, and Advanced's, which
/// Simple leaves at their defaults.
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
    /// Advanced was chosen, so changing an answer asks its questions again.
    pub advanced: bool,
    /// Where the database runs.
    pub database: Database,
    /// The certificate the site serves.
    pub certificate: Certificate,
    /// The reverse proxies in front of the site (`web.trusted_proxies`).
    pub proxies: Vec<String>,
    /// The syslog server lines also go to (`logs.syslog`), if any.
    pub syslog: Option<String>,
    /// Where the backups go; `None` is the archive's `.scannerpulse-backups`.
    pub backups: Option<PathBuf>,
    /// Each backup kind's push (heartbeat) address, by `backup::KINDS` name.
    pub pushes: Vec<(String, String)>,
}

/// Where the database runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Database {
    /// PostgreSQL 17 on this machine, installed and backed up by setup.
    Here,
    /// One the person runs; its failover and backups stay theirs.
    Theirs(Remote),
}

/// A database the person runs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Remote {
    pub host: String,
    pub port: u16,
    pub name: String,
    pub role: String,
    pub password: String,
    pub sslmode: SslMode,
    /// The CA certificate (PEM) that signed the server's, for `VerifyFull`
    /// when no public authority did; copied to `db_ca_path()`.
    pub root_cert: Option<PathBuf>,
}

/// How the connection to a database the person runs is protected, by
/// PostgreSQL's names for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SslMode {
    /// Encrypted when the server offers it.
    #[default]
    Prefer,
    /// Always encrypted; the server's certificate is not checked.
    Require,
    /// Always encrypted, and the server's certificate checked against its
    /// name.
    VerifyFull,
}

impl SslMode {
    pub fn parse(name: &str) -> Option<SslMode> {
        match name {
            "prefer" => Some(SslMode::Prefer),
            "require" => Some(SslMode::Require),
            "verify-full" => Some(SslMode::VerifyFull),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            SslMode::Prefer => "prefer",
            SslMode::Require => "require",
            SslMode::VerifyFull => "verify-full",
        }
    }
}

impl Remote {
    /// Its address, without the password (which goes in the password file).
    pub fn url(&self) -> String {
        let mut url = reqwest::Url::parse("postgres://localhost/").expect("a URL");
        // A host that is an IPv6 address goes in brackets.
        let host = if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let _ = url.set_host(Some(&host));
        let _ = url.set_port(Some(self.port));
        let _ = url.set_username(&self.role);
        url.set_path(&self.name);
        if self.sslmode != SslMode::Prefer {
            url.query_pairs_mut()
                .append_pair("sslmode", self.sslmode.name());
        }
        if self.root_cert.is_some() {
            url.query_pairs_mut()
                .append_pair("sslrootcert", &db_ca_path().display().to_string());
        }
        url.to_string()
    }

    /// Its line in the password file, as the services' reader (sqlx) takes
    /// it: any host (the file serves this one database), the name and the
    /// role with `:` and `\` escaped, and the password as typed, to the end
    /// of the line.
    pub fn pgpass_line(&self) -> String {
        let esc = |s: &str| s.replace('\\', "\\\\").replace(':', "\\:");
        format!(
            "*:{}:{}:{}:{}\n",
            self.port,
            esc(&self.name),
            esc(&self.role),
            self.password
        )
    }
}

/// The certificate the site serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Certificate {
    /// Made by setup for this machine's names; the browser warns once.
    Own,
    /// Files the person gives (PEM), copied into the settings folder.
    Installed { cert: PathBuf, key: PathBuf },
    /// Let's Encrypt's for `names`, proved as `proof` says; the site serves
    /// the one setup makes until it arrives.
    LetsEncrypt {
        proof: crate::acme::Proof,
        names: Vec<String>,
    },
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

pub fn run(title: &str, program: &str, args: &[&str]) -> Step {
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

/// Opens (or, not `open`, closes) `port` in firewalld, when it runs.
pub fn firewall(open: bool, port: u16) -> Step {
    let (title, verb) = if open {
        ("Open", "add")
    } else {
        ("Close", "remove")
    };
    run(
        &format!("{title} port {port} in the firewall, if firewalld runs"),
        "/bin/sh",
        &[
            "-c",
            &format!(
                "if systemctl is-active --quiet firewalld; then firewall-cmd --quiet --permanent --{verb}-port={port}/tcp && firewall-cmd --quiet --reload; fi"
            ),
        ],
    )
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
pub fn db_ca_path() -> PathBuf {
    Path::new(ETC).join("db-ca.pem")
}
/// The backups' folder inside the archive; the dot keeps it from ever
/// reading as a date folder.
pub fn backups_path(archive: &Path) -> PathBuf {
    archive.join(".scannerpulse-backups")
}

/// Where these answers' backups go: none with a database the person runs.
pub fn backups_folder(answers: &Answers) -> Option<PathBuf> {
    match answers.database {
        Database::Here => Some(
            answers
                .backups
                .clone()
                .unwrap_or_else(|| backups_path(&answers.archive)),
        ),
        Database::Theirs(_) => None,
    }
}

/// pgBackRest's settings: its repository in the backups folder, two full
/// backups kept (with the differentials and the WAL between them).
pub fn pgbackrest_conf(backups: &Path, system: &System) -> String {
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
        repo = backups.join("pgbackrest").display(),
        data = system.postgres_data(),
    )
}

/// A unit's line that makes it wait for the mounts `folders` are on (a
/// share mounted late at boot), each path quoted as systemd reads it.
fn requires_mounts(folders: &[&Path]) -> String {
    let quoted: Vec<String> = folders
        .iter()
        .map(|folder| {
            let path = folder.display().to_string();
            let escaped = path
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%");
            format!("\"{escaped}\"")
        })
        .collect();
    format!("RequiresMountsFor={}\n", quoted.join(" "))
}

/// The nightly backup's service and timer, in that order; the backups are
/// in `backups`.
pub fn backup_units(binary: &Path, system: &System, backups: &Path) -> (String, String) {
    (
        format!(
            "# Written by `scannerpulse setup`.\n\
             [Unit]\n\
             Description=ScannerPulse nightly backups (pgBackRest and a dump)\n\
             After={pg}.service\n\
             {mounts}\
             \n\
             [Service]\n\
             Type=oneshot\n\
             EnvironmentFile={env}\n\
             ExecStart={binary} backup\n",
            pg = system.postgres_service(),
            mounts = requires_mounts(&[backups]),
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
    let url = match &answers.database {
        Database::Here => format!("postgres://{ACCOUNT}@localhost:{PORT_PG}/{DATABASE}"),
        Database::Theirs(remote) => remote.url(),
    };
    let mut pairs = vec![
        ("DATABASE_URL", url),
        ("PGPASSFILE", pgpass_path().display().to_string()),
        ("SCANNERPULSE_HOST", answers.host.clone()),
        ("SCANNERPULSE_PREFERRED", "true".into()),
        (
            "SCANNERPULSE_WEB_LISTEN",
            format!("0.0.0.0:{}", answers.port),
        ),
        ("SCANNERPULSE_TLS_CERT", cert_path().display().to_string()),
        ("SCANNERPULSE_TLS_KEY", key_path().display().to_string()),
    ];
    if let Certificate::LetsEncrypt { proof, names } = &answers.certificate {
        pairs.push(("SCANNERPULSE_ACME", proof.name().to_string()));
        pairs.push(("SCANNERPULSE_ACME_NAMES", names.join(" ")));
        if let crate::acme::Proof::Cloudflare { token } = proof {
            pairs.push(("SCANNERPULSE_CLOUDFLARE_TOKEN", token.clone()));
        }
    }
    if let Some(backups) = backups_folder(answers) {
        pairs.push(("SCANNERPULSE_BACKUPS", backups.display().to_string()));
        for (kind, url) in &answers.pushes {
            pairs.extend(crate::backup::push_var(kind).map(|var| (var, url.clone())));
        }
    }
    pairs
}

/// The systemd unit of `command` (`web` or `ingest`), which uses `folders`.
/// With `local_db`, it starts after this machine's PostgreSQL.
pub fn unit(
    command: &str,
    binary: &Path,
    system: &System,
    folders: &[&Path],
    local_db: bool,
) -> String {
    let (what, extra) = match command {
        "web" => (
            "the web interface",
            // Port 443 without root.
            "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n",
        ),
        // Ingest releases the processing lease before it stops.
        _ => ("ingest: filing and transcription", "TimeoutStopSec=150\n"),
    };
    let after = if local_db {
        format!(" {}.service", system.postgres_service())
    } else {
        String::new()
    };
    format!(
        "# Written by `scannerpulse setup`.\n\
         [Unit]\n\
         Description=ScannerPulse {what}\n\
         Wants=network-online.target\n\
         After=network-online.target{after}\n\
         {mounts}\
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
        mounts = requires_mounts(folders),
        env = env_path().display(),
        binary = binary.display(),
    )
}

/// Run as `account` with a folder as `$1`: fails, saying why and what to
/// do, unless the account can enter the folder and write a file in it (a
/// share that squashes the account can say it may write, then refuse).
fn reach(account: &str) -> String {
    format!(
        "cd -- \"$1\" 2>/dev/null && touch .scannerpulse-reach 2>/dev/null && rm -f .scannerpulse-reach || {{ echo \"the {account} account cannot reach or write $1: a folder above it may be closed to other accounts, as a home folder often is, or a network share may not let the account write. Open each such folder to the account (setfacl -m u:{account}:x FOLDER), let the account write on the share, or choose another folder, then run setup again.\"; exit 1; }}"
    )
}

/// `setfacl` with `args`, except that a filesystem without POSIX ACLs, as
/// an NFSv4 or CIFS share, is let through: its own permissions decide, and
/// setup's reach check proves them. Any other error still stops.
pub(super) fn setfacl(title: &str, args: &[&str]) -> Step {
    const LET_THROUGH: &str = "said=$(LC_ALL=C setfacl \"$@\" 2>&1) && exit 0; printf '%s\\n' \"$said\" | grep -qv 'Operation not supported' || exit 0; printf '%s\\n' \"$said\" >&2; exit 1";
    run(
        title,
        "sh",
        &[&["-c", LET_THROUGH, "setfacl"], args].concat(),
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
    let here = answers.database == Database::Here;
    match system.family {
        // A database the person runs: only the tool for folders' access lists.
        Family::Apt if !here => {
            steps.push(run(
                "Read the package lists",
                "apt-get",
                &["-o", "DPkg::Lock::Timeout=300", "update"],
            ));
            steps.push(run(
                "Install the tool for folders' access lists",
                "apt-get",
                &["-o", "DPkg::Lock::Timeout=300", "install", "-y", "acl"],
            ));
        }
        Family::Dnf if !here => steps.push(run(
            "Install the tool for folders' access lists",
            "dnf",
            &["install", "-y", "acl"],
        )),
        Family::Apt => {
            // A fresh machine's unattended upgrades may hold apt's lock a while.
            let wait = "DPkg::Lock::Timeout=300";
            steps.push(run(
                "Read the package lists",
                "apt-get",
                &["-o", wait, "update"],
            ));
            steps.push(run(
                "Install the PostgreSQL project's repository tool",
                "apt-get",
                &[
                    "-o",
                    wait,
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
                &["-o", wait, "install", "-y", "postgresql-17", "pgbackrest"],
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
                // The system's own PostgreSQL module would shadow version 17;
                // version 10 has no modules.
                if matches!(system.version.split('.').next(), Some("8" | "9")) {
                    steps.push(run(
                        "Turn off the system's own PostgreSQL module",
                        "dnf",
                        &["-qy", "module", "disable", "postgresql"],
                    ));
                }
                // pgBackRest needs libssh2, which only EPEL carries here.
                steps.push(run(
                    "Add EPEL, for pgBackRest's libssh2",
                    "dnf",
                    &["install", "-y", "epel-release"],
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
    if here {
        steps.push(run(
            "Start PostgreSQL 17, now and at boot",
            "systemctl",
            &["enable", "--now", system.postgres_service()],
        ));
    }
    steps.push(run(
        "Make the scannerpulse account",
        "/bin/sh",
        &[
            "-c",
            "id -u scannerpulse >/dev/null 2>&1 || useradd --system --user-group \
             --home-dir /var/lib/scannerpulse --create-home --shell /usr/sbin/nologin scannerpulse",
        ],
    ));
    if here {
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
    }
    for (folder, role) in [
        (&answers.recordings, "recordings"),
        (&answers.archive, "archive"),
    ] {
        let path = folder.display().to_string();
        steps.push(setfacl(
            &format!("Let the scannerpulse account read and write the {role} folder"),
            &[
                "-R",
                "-m",
                "u:scannerpulse:rwX",
                "-m",
                "d:u:scannerpulse:rwX",
                &path,
            ],
        ));
        // The folder's own access is not enough: every folder above it must
        // let the account through, and a home folder often does not.
        steps.push(run(
            &format!("Check that the scannerpulse account reaches the {role} folder"),
            "runuser",
            &[
                "-u",
                ACCOUNT,
                "--",
                "sh",
                "-c",
                &reach(ACCOUNT),
                "reach",
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
            contents: match &answers.database {
                Database::Here => {
                    format!("localhost:{PORT_PG}:{DATABASE}:{ACCOUNT}:{db_password}\n")
                }
                Database::Theirs(remote) => remote.pgpass_line(),
            },
            mode: 0o600,
            owner: format!("{ACCOUNT}:{ACCOUNT}"),
        },
    });
    if let Database::Theirs(Remote {
        root_cert: Some(ca),
        ..
    }) = &answers.database
    {
        steps.push(run(
            "Copy the database's CA certificate",
            "install",
            &[
                "-m",
                "0644",
                "-o",
                "root",
                "-g",
                ACCOUNT,
                &ca.display().to_string(),
                &db_ca_path().display().to_string(),
            ],
        ));
    }
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
    let me = binary.display().to_string();
    match &answers.certificate {
        Certificate::Own | Certificate::LetsEncrypt { .. } => {
            let mut cert_args = vec![
                "make".to_string(),
                "--cert".into(),
                cert_path().display().to_string(),
                "--key".into(),
                key_path().display().to_string(),
            ];
            cert_args.extend(answers.names.iter().cloned());
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
        }
        Certificate::Installed { cert, key } => {
            let (from_cert, from_key) = (cert.display().to_string(), key.display().to_string());
            let (to_cert, to_key) = (
                cert_path().display().to_string(),
                key_path().display().to_string(),
            );
            steps.push(run(
                "Copy the site's certificate",
                "install",
                &[
                    "-m", "0644", "-o", ACCOUNT, "-g", ACCOUNT, &from_cert, &to_cert,
                ],
            ));
            steps.push(run(
                "Copy its key, for the scannerpulse account alone",
                "install",
                &[
                    "-m", "0600", "-o", ACCOUNT, "-g", ACCOUNT, &from_key, &to_key,
                ],
            ));
        }
    }
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
    if !answers.proxies.is_empty() {
        more.push(bin(
            "Set the reverse proxies' addresses",
            &[
                "settings",
                "set",
                "web.trusted_proxies",
                &serde_json::json!(answers.proxies).to_string(),
            ],
            None,
        ));
    }
    if let Some(server) = &answers.syslog {
        more.push(bin(
            "Send the logs to the syslog server too",
            &[
                "settings",
                "set",
                "logs.syslog",
                &serde_json::json!(server).to_string(),
            ],
            None,
        ));
    }
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
    let backups = backups_folder(answers);
    if let Some(backups) = &backups {
        backup_steps(&mut steps, backups, binary, system);
    }
    for command in ["web", "ingest"] {
        steps.push(Step {
            title: format!("Write the {command} service"),
            action: Action::Write {
                path: PathBuf::from(format!(
                    "/etc/systemd/system/scannerpulse-{command}.service"
                )),
                contents: unit(
                    command,
                    binary,
                    system,
                    &[&answers.recordings, &answers.archive],
                    answers.database == Database::Here,
                ),
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
    let mut units = vec!["scannerpulse-web", "scannerpulse-ingest"];
    if backups.is_some() {
        units.push("scannerpulse-backup.timer");
    }
    steps.push(run(
        "Start ScannerPulse at boot",
        "systemctl",
        &[&["enable"], &units[..]].concat(),
    ));
    // A rerun gives the database role a new password: a service still
    // running from before would keep the old one and fail.
    steps.push(run(
        "Start or restart ScannerPulse, to read the new settings",
        "systemctl",
        &[&["restart"], &units[..]].concat(),
    ));
    // Rocky, Alma and Fedora run firewalld, which would shut the site out.
    steps.push(firewall(true, answers.port));
    steps.push(Step {
        title: "Wait for the web interface".into(),
        action: Action::WaitForWeb { port: answers.port },
    });
    steps
}

/// The backups' steps, into `backups`: its folders kept to PostgreSQL and
/// root, pgBackRest's settings, the WAL sent to it, the first full backup,
/// and the nightly timer.
pub(super) fn backup_steps(steps: &mut Vec<Step>, backups: &Path, binary: &Path, system: &System) {
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
    // The archive's default ACL would give the service account its way into
    // the backups; they are PostgreSQL's and root's alone. Only setup's own
    // folders are stripped: a folder the person names may hold other things.
    let own = |sub: &str| backups.join(sub).display().to_string();
    if backups
        .file_name()
        .is_some_and(|name| name == ".scannerpulse-backups")
    {
        steps.push(setfacl(
            "Keep the archive's access off its backups folder",
            &["-b", &backups.display().to_string()],
        ));
    }
    steps.push(setfacl(
        "Keep the backups to PostgreSQL and root",
        &["-R", "-b", &own("pgbackrest"), &own("dump")],
    ));
    // PostgreSQL archives its WAL there: every folder above must let it in.
    steps.push(run(
        "Check that the postgres account reaches the backups folder",
        "runuser",
        &[
            "-u",
            "postgres",
            "--",
            "sh",
            "-c",
            &reach("postgres"),
            "reach",
            &backups.join("pgbackrest").display().to_string(),
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
            contents: pgbackrest_conf(backups, system),
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
    let (service, timer) = backup_units(binary, system, backups);
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
}

#[cfg(test)]
mod tests {
    use super::{
        Action, Answers, Certificate, Database, Transcription, backup_units, env_pairs,
        pgbackrest_conf, reach, steps, unit,
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
            advanced: false,
            database: Database::Here,
            certificate: Certificate::Own,
            proxies: Vec::new(),
            syslog: None,
            backups: None,
            pushes: Vec::new(),
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
        let epel = rocky
            .iter()
            .position(|t| t == "Add EPEL, for pgBackRest's libssh2")
            .unwrap();
        assert!(
            epel < rocky
                .iter()
                .position(|t| t == "Install PostgreSQL 17 and pgBackRest")
                .unwrap()
        );
        let fedora = titles("ID=fedora\nVERSION_ID=42\n");
        assert!(!fedora.contains(&"Turn off the system's own PostgreSQL module".to_string()));
        assert!(!fedora.contains(&"Add EPEL, for pgBackRest's libssh2".to_string()));
        // Version 10 of the Red Hat family has no modules to turn off.
        for ten in [
            "ID=\"rocky\"\nVERSION_ID=\"10.0\"\n",
            "ID=\"almalinux\"\nVERSION_ID=\"10.0\"\n",
        ] {
            let list = titles(ten);
            assert!(!list.contains(&"Turn off the system's own PostgreSQL module".to_string()));
            assert!(list.contains(&"Add EPEL, for pgBackRest's libssh2".to_string()));
        }
        assert!(
            titles("ID=\"almalinux\"\nVERSION_ID=\"8.10\"\n")
                .contains(&"Turn off the system's own PostgreSQL module".to_string())
        );
        for list in [&debian, &rocky] {
            let tail: Vec<&str> = list
                .iter()
                .rev()
                .take(5)
                .rev()
                .map(String::as_str)
                .collect();
            assert_eq!(
                tail,
                [
                    "Read the new services",
                    "Start ScannerPulse at boot",
                    "Start or restart ScannerPulse, to read the new settings",
                    "Open port 443 in the firewall, if firewalld runs",
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
    fn firewalld_when_it_runs_opens_the_sites_port_before_the_last_check() {
        let mut a = answers();
        a.port = 8443;
        let all = steps(&a, &detect(ROCKY, "x86_64").unwrap(), Path::new("/b"), "pw");
        let at = all
            .iter()
            .position(|s| s.title == "Open port 8443 in the firewall, if firewalld runs")
            .unwrap();
        assert_eq!(all[at + 1].title, "Wait for the web interface");
        let Action::Run { program, args, .. } = &all[at].action else {
            panic!()
        };
        assert_eq!(program, "/bin/sh");
        assert_eq!(
            args,
            &[
                "-c",
                "if systemctl is-active --quiet firewalld; then firewall-cmd --quiet --permanent --add-port=8443/tcp && firewall-cmd --quiet --reload; fi"
            ]
        );
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
        let folders = [Path::new("/srv/recordings"), Path::new("/srv/archive")];
        let web = unit(
            "web",
            Path::new("/usr/local/bin/scannerpulse"),
            &system,
            &folders,
            true,
        );
        assert!(web.contains("User=scannerpulse\n"));
        assert!(web.contains("ExecStart=/usr/local/bin/scannerpulse web\n"));
        assert!(web.contains("AmbientCapabilities=CAP_NET_BIND_SERVICE\n"));
        assert!(web.contains("After=network-online.target postgresql-17.service\n"));
        assert!(web.contains("EnvironmentFile=/etc/scannerpulse/scannerpulse.env\n"));
        let ingest = unit(
            "ingest",
            Path::new("/usr/local/bin/scannerpulse"),
            &system,
            &folders,
            true,
        );
        assert!(
            ingest.contains("TimeoutStopSec=150\n") && !ingest.contains("CAP_NET_BIND_SERVICE")
        );
        // Both wait for the folders' mounts (a share mounted late at boot).
        for text in [&web, &ingest] {
            assert!(
                text.contains("RequiresMountsFor=\"/srv/recordings\" \"/srv/archive\"\n"),
                "{text}"
            );
        }
        let odd = unit(
            "ingest",
            Path::new("/usr/local/bin/scannerpulse"),
            &system,
            &[Path::new("/srv/my calls"), Path::new("/srv/100%/a\"b\\c")],
            true,
        );
        assert!(
            odd.contains("RequiresMountsFor=\"/srv/my calls\" \"/srv/100%%/a\\\"b\\\\c\"\n"),
            "{odd}"
        );
    }

    #[test]
    fn the_account_must_reach_each_folder_through_the_folders_above_it() {
        let system = detect(DEBIAN, "x86_64").unwrap();
        let all = steps(
            &answers(),
            &system,
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let at = |title: &str| all.iter().position(|s| s.title == title).unwrap();
        let check = at("Check that the scannerpulse account reaches the recordings folder");
        assert!(at("Make the scannerpulse account") < check);
        assert_eq!(
            check,
            at("Let the scannerpulse account read and write the recordings folder") + 1
        );
        let Action::Run { program, args, .. } = &all[check].action else {
            panic!()
        };
        assert_eq!(program, "runuser");
        assert_eq!(&args[..4], ["-u", "scannerpulse", "--", "sh"]);
        assert_eq!(args.last().unwrap(), "/srv/recordings");
        assert_eq!(
            args[5],
            "cd -- \"$1\" 2>/dev/null && touch .scannerpulse-reach 2>/dev/null && rm -f .scannerpulse-reach || { echo \"the scannerpulse account cannot reach or write $1: a folder above it may be closed to other accounts, as a home folder often is, or a network share may not let the account write. Open each such folder to the account (setfacl -m u:scannerpulse:x FOLDER), let the account write on the share, or choose another folder, then run setup again.\"; exit 1; }"
        );
        assert!(at("Check that the scannerpulse account reaches the archive folder") > check);
    }

    /// A folder holding a stand-in for `name` that ends with `code`, after
    /// printing `$SAID` to its errors.
    fn stand_in(name: &str, code: i32) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            format!("#!/bin/sh\necho \"$SAID\" >&2\nexit {code}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        dir
    }

    fn path_with(dir: &Path) -> String {
        format!("{}:{}", dir.display(), std::env::var("PATH").unwrap())
    }

    #[test]
    fn a_filesystem_without_posix_acls_is_left_to_the_reach_check() {
        let all = steps(
            &answers(),
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let step = all
            .iter()
            .find(|s| {
                s.title == "Let the scannerpulse account read and write the recordings folder"
            })
            .unwrap();
        let Action::Run { program, args, .. } = &step.action else {
            panic!()
        };
        assert_eq!(
            &args[2..],
            [
                "setfacl",
                "-R",
                "-m",
                "u:scannerpulse:rwX",
                "-m",
                "d:u:scannerpulse:rwX",
                "/srv/recordings"
            ]
        );
        let unsupported = "setfacl: /srv/recordings: Operation not supported";
        let denied = "setfacl: /srv/recordings/a: Permission denied";
        for (code, said, passes) in [
            (0, "", true),
            (1, unsupported, true),
            (1, denied, false),
            (1, &format!("{unsupported}\n{denied}"), false),
        ] {
            let fake = stand_in("setfacl", code);
            let output = std::process::Command::new(program)
                .args(args)
                .env("PATH", path_with(fake.path()))
                .env("SAID", said)
                .output()
                .unwrap();
            assert_eq!(output.status.success(), passes, "{said}");
            if !passes {
                assert!(String::from_utf8_lossy(&output.stderr).contains(denied));
            }
        }
    }

    #[test]
    fn the_reach_check_writes_a_file_and_leaves_none() {
        let folder = tempfile::tempdir().unwrap();
        let check = |path: &str| {
            std::process::Command::new("sh")
                .args(["-c", &reach("scannerpulse"), "reach"])
                .arg(folder.path())
                .env("PATH", path)
                .output()
                .unwrap()
        };
        let path = std::env::var("PATH").unwrap();
        assert!(check(&path).status.success());
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
        // A share that squashes the account says it may write, then refuses.
        let refused = stand_in("touch", 1);
        let output = check(&path_with(refused.path()));
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("cannot reach or write"));
    }

    #[test]
    fn postgresql_must_reach_the_backups_folder_before_it_archives_into_it() {
        let all = steps(
            &answers(),
            &detect(ROCKY, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let at = |title: &str| all.iter().position(|s| s.title == title).unwrap();
        let check = at("Check that the postgres account reaches the backups folder");
        assert_eq!(check, at("Keep the backups to PostgreSQL and root") + 1);
        assert!(check < at("Send PostgreSQL's WAL to pgBackRest"));
        let Action::Run { program, args, .. } = &all[check].action else {
            panic!()
        };
        assert_eq!(program, "runuser");
        assert_eq!(&args[..4], ["-u", "postgres", "--", "sh"]);
        assert_eq!(
            args.last().unwrap(),
            "/srv/archive/.scannerpulse-backups/pgbackrest"
        );
        assert!(
            args[5].contains("echo \"the postgres account cannot reach or write $1:")
                && args[5].contains("(setfacl -m u:postgres:x FOLDER)"),
            "{}",
            args[5]
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
        let conf = pgbackrest_conf(Path::new("/srv/archive/.scannerpulse-backups"), &debian);
        assert!(conf.contains("repo1-path=/srv/archive/.scannerpulse-backups/pgbackrest\n"));
        assert!(conf.contains("[scannerpulse]\npg1-path=/var/lib/postgresql/17/main\n"));
        let rocky = detect(ROCKY, "x86_64").unwrap();
        assert!(
            pgbackrest_conf(Path::new("/srv/archive/.scannerpulse-backups"), &rocky)
                .contains("pg1-path=/var/lib/pgsql/17/data\n")
        );
        let (service, timer) = backup_units(
            Path::new("/usr/local/bin/scannerpulse"),
            &rocky,
            Path::new("/srv/archive"),
        );
        assert!(service.contains("ExecStart=/usr/local/bin/scannerpulse backup\n"));
        assert!(service.contains("RequiresMountsFor=\"/srv/archive\"\n"));
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
        let Action::Run { args, .. } = &all[at("Keep the backups to PostgreSQL and root")].action
        else {
            panic!()
        };
        assert_eq!(
            &args[2..],
            [
                "setfacl",
                "-R",
                "-b",
                "/srv/archive/.scannerpulse-backups/pgbackrest",
                "/srv/archive/.scannerpulse-backups/dump"
            ]
        );
        let Action::Run { args, .. } =
            &all[at("Keep the archive's access off its backups folder")].action
        else {
            panic!()
        };
        assert_eq!(
            &args[2..],
            ["setfacl", "-b", "/srv/archive/.scannerpulse-backups"]
        );
        assert!(
            at("Make the folder of the nightly dumps")
                < at("Keep the backups to PostgreSQL and root")
        );
        assert!(
            at("Restart PostgreSQL 17, to start sending its WAL")
                < at("Make pgBackRest's place for this database")
        );
        assert!(at("Make pgBackRest's place for this database") < at("Take the first full backup"));
        let Action::Run { args, .. } = &all[at("Start ScannerPulse at boot")].action else {
            panic!()
        };
        assert!(args.contains(&"scannerpulse-backup.timer".to_string()));
    }

    #[test]
    fn a_rerun_restarts_the_services_so_they_read_the_new_database_password() {
        let all = steps(
            &answers(),
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let at = |title: &str| all.iter().position(|s| s.title == title).unwrap();
        let units = [
            "scannerpulse-web",
            "scannerpulse-ingest",
            "scannerpulse-backup.timer",
        ];
        for (title, verb) in [
            ("Start ScannerPulse at boot", "enable"),
            (
                "Start or restart ScannerPulse, to read the new settings",
                "restart",
            ),
        ] {
            let Action::Run { program, args, .. } = &all[at(title)].action else {
                panic!()
            };
            assert_eq!(program, "systemctl");
            assert_eq!(args[0], verb);
            assert_eq!(&args[1..], units);
        }
        assert_eq!(
            at("Start or restart ScannerPulse, to read the new settings"),
            at("Start ScannerPulse at boot") + 1
        );
    }

    fn theirs() -> Answers {
        Answers {
            advanced: true,
            database: Database::Theirs(super::Remote {
                host: "db.example.org".into(),
                port: 5433,
                name: "calls".into(),
                role: "sp".into(),
                password: "se:cret".into(),
                ..Default::default()
            }),
            ..answers()
        }
    }

    #[test]
    fn a_database_you_run_gets_no_postgresql_here_and_no_backups() {
        let all = steps(
            &theirs(),
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "unused",
        );
        let titles: Vec<&str> = all.iter().map(|s| s.title.as_str()).collect();
        for gone in [
            "Install PostgreSQL 17 and pgBackRest",
            "Start PostgreSQL 17, now and at boot",
            "Make the database and its role",
            "Take the first full backup",
            "Write scannerpulse-backup.timer",
        ] {
            assert!(!titles.contains(&gone), "{gone}");
        }
        assert!(titles.contains(&"Install the tool for folders' access lists"));
        let pairs = env_pairs(&theirs());
        assert!(pairs.contains(&(
            "DATABASE_URL",
            "postgres://sp@db.example.org:5433/calls".to_string()
        )));
        assert!(!pairs.iter().any(|(n, _)| *n == "SCANNERPULSE_BACKUPS"));
        let Some(Action::Write { contents, .. }) = all
            .iter()
            .find(|s| s.title == "Write the database's password file")
            .map(|s| &s.action)
        else {
            panic!()
        };
        assert_eq!(contents, "*:5433:calls:sp:se:cret\n");
        let Some(Action::Write { contents: web, .. }) = all
            .iter()
            .find(|s| s.title == "Write the web service")
            .map(|s| &s.action)
        else {
            panic!()
        };
        assert!(web.contains("After=network-online.target\n"), "{web}");
        let Action::Run { args, .. } = &all
            .iter()
            .find(|s| s.title == "Start ScannerPulse at boot")
            .unwrap()
            .action
        else {
            panic!()
        };
        assert_eq!(args, &["enable", "scannerpulse-web", "scannerpulse-ingest"]);
        let rocky = steps(
            &theirs(),
            &detect(ROCKY, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "unused",
        );
        assert!(
            !rocky
                .iter()
                .any(|s| s.title.contains("PostgreSQL") || s.title.contains("EPEL"))
        );
    }

    #[test]
    fn a_database_you_run_can_require_tls_or_a_checked_certificate() {
        let mut remote = super::Remote {
            host: "db.example.org".into(),
            port: 5432,
            name: "calls".into(),
            role: "sp".into(),
            password: "p".into(),
            ..Default::default()
        };
        assert_eq!(remote.url(), "postgres://sp@db.example.org:5432/calls");
        remote.sslmode = super::SslMode::Require;
        assert_eq!(
            remote.url(),
            "postgres://sp@db.example.org:5432/calls?sslmode=require"
        );
        remote.sslmode = super::SslMode::VerifyFull;
        remote.root_cert = Some("/root/db-ca.pem".into());
        assert_eq!(
            remote.url(),
            "postgres://sp@db.example.org:5432/calls?sslmode=verify-full&sslrootcert=%2Fetc%2Fscannerpulse%2Fdb-ca.pem"
        );
        remote
            .url()
            .parse::<sqlx::postgres::PgConnectOptions>()
            .unwrap();
        let answers = Answers {
            database: Database::Theirs(remote),
            ..theirs()
        };
        let all = steps(
            &answers,
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "unused",
        );
        let at = |title: &str| all.iter().position(|s| s.title == title).unwrap();
        let Action::Run { program, args, .. } =
            &all[at("Copy the database's CA certificate")].action
        else {
            panic!()
        };
        assert_eq!(program, "install");
        assert_eq!(
            args,
            &[
                "-m",
                "0644",
                "-o",
                "root",
                "-g",
                "scannerpulse",
                "/root/db-ca.pem",
                "/etc/scannerpulse/db-ca.pem"
            ]
        );
        assert!(at("Copy the database's CA certificate") < at("Make the database's tables"));
    }

    #[test]
    fn an_address_with_odd_characters_stays_one_address() {
        let remote = super::Remote {
            host: "2001:db8::40".into(),
            port: 5432,
            name: "radio calls".into(),
            role: "sp@site".into(),
            password: "a\\b:c".into(),
            ..Default::default()
        };
        assert_eq!(
            remote.url(),
            "postgres://sp%40site@[2001:db8::40]:5432/radio%20calls"
        );
        assert_eq!(remote.pgpass_line(), "*:5432:radio calls:sp@site:a\\b:c\n");
    }

    #[test]
    fn with_lets_encrypt_setup_makes_the_certificate_served_until_it_arrives() {
        let dns = Answers {
            advanced: true,
            certificate: Certificate::LetsEncrypt {
                proof: crate::acme::Proof::Cloudflare {
                    token: "cf-token-1".into(),
                },
                names: vec!["radio.example.org".into(), "www.example.org".into()],
            },
            ..answers()
        };
        let all = steps(
            &dns,
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        assert!(
            all.iter()
                .any(|s| s.title == "Make this machine's certificate")
        );
        let pairs = env_pairs(&dns);
        for want in [
            ("SCANNERPULSE_ACME", "dns-cloudflare"),
            (
                "SCANNERPULSE_ACME_NAMES",
                "radio.example.org www.example.org",
            ),
            ("SCANNERPULSE_CLOUDFLARE_TOKEN", "cf-token-1"),
        ] {
            assert!(
                pairs.contains(&(want.0, want.1.to_string())),
                "{want:?}: {pairs:?}"
            );
        }
        let web = env_pairs(&Answers {
            certificate: Certificate::LetsEncrypt {
                proof: crate::acme::Proof::Web,
                names: vec!["radio.example.org".into()],
            },
            ..answers()
        });
        assert!(web.contains(&("SCANNERPULSE_ACME", "web".to_string())));
        assert!(
            !web.iter()
                .any(|(n, _)| *n == "SCANNERPULSE_CLOUDFLARE_TOKEN")
        );
        assert!(
            !env_pairs(&answers())
                .iter()
                .any(|(n, _)| n.contains("ACME"))
        );
    }

    #[test]
    fn your_certificate_is_copied_and_the_advanced_settings_written() {
        let answers = Answers {
            advanced: true,
            certificate: Certificate::Installed {
                cert: "/root/site.pem".into(),
                key: "/root/site.key".into(),
            },
            proxies: vec!["192.0.2.5".into()],
            syslog: Some("tcp://192.0.2.30:6514".into()),
            backups: Some("/srv/backups".into()),
            pushes: vec![(
                "pgbackrest".into(),
                "https://kuma.example.org/api/push/x".into(),
            )],
            ..answers()
        };
        let all = steps(
            &answers,
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let run = |title: &str| match &all
            .iter()
            .find(|s| s.title == title)
            .unwrap_or_else(|| panic!("{title}"))
            .action
        {
            Action::Run { args, .. } => args.clone(),
            other => panic!("{other:?}"),
        };
        assert!(
            !all.iter()
                .any(|s| s.title == "Make this machine's certificate")
        );
        assert_eq!(
            run("Copy the site's certificate"),
            [
                "-m",
                "0644",
                "-o",
                "scannerpulse",
                "-g",
                "scannerpulse",
                "/root/site.pem",
                "/etc/scannerpulse/tls/cert.pem"
            ]
        );
        assert_eq!(
            run("Copy its key, for the scannerpulse account alone"),
            [
                "-m",
                "0600",
                "-o",
                "scannerpulse",
                "-g",
                "scannerpulse",
                "/root/site.key",
                "/etc/scannerpulse/tls/key.pem"
            ]
        );
        assert_eq!(
            run("Set the reverse proxies' addresses")[3],
            "[\"192.0.2.5\"]"
        );
        assert_eq!(
            run("Send the logs to the syslog server too")[3],
            "\"tcp://192.0.2.30:6514\""
        );
        let pairs = env_pairs(&answers);
        assert!(pairs.contains(&("SCANNERPULSE_BACKUPS", "/srv/backups".to_string())));
        assert!(pairs.contains(&(
            "SCANNERPULSE_BACKUP_PUSH_PGBACKREST",
            "https://kuma.example.org/api/push/x".to_string()
        )));
        let Some(Action::Write { contents, .. }) = all
            .iter()
            .find(|s| s.title == "Write pgBackRest's settings")
            .map(|s| &s.action)
        else {
            panic!()
        };
        assert!(
            contents.contains("repo1-path=/srv/backups/pgbackrest\n"),
            "{contents}"
        );
    }

    #[test]
    fn a_backups_folder_you_name_keeps_whatever_else_it_holds() {
        let answers = Answers {
            backups: Some("/srv/backups".into()),
            ..answers()
        };
        let all = steps(
            &answers,
            &detect(DEBIAN, "x86_64").unwrap(),
            Path::new("/usr/local/bin/scannerpulse"),
            "db secret",
        );
        let touched: Vec<&String> = all
            .iter()
            .filter_map(|s| match &s.action {
                Action::Run { args, .. } if args.get(2).map(String::as_str) == Some("setfacl") => {
                    Some(args)
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert!(!touched.iter().any(|a| *a == "/srv/backups"), "{touched:?}");
        assert!(touched.iter().any(|a| *a == "/srv/backups/pgbackrest"));
        assert!(touched.iter().any(|a| *a == "/srv/backups/dump"));
    }
}
