//! Which Linux this is, from `/etc/os-release`, and how setup installs
//! PostgreSQL 17 on it: from the PostgreSQL project's own repositories, so
//! every supported system gets the same version.

/// The package tools a supported system uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Debian and Ubuntu.
    Apt,
    /// Fedora and the Red Hat family.
    Dnf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct System {
    /// `ID` from os-release: debian, ubuntu, fedora, rocky, almalinux.
    pub id: String,
    /// `VERSION_ID`.
    pub version: String,
    pub family: Family,
    /// `x86_64` or `aarch64`.
    pub arch: String,
}

/// The systems setup supports, as the README lists them.
pub const SUPPORTED: &str = "Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora, \
    Rocky Linux and AlmaLinux 8 to 10, on x86_64 or aarch64 (64-bit ARM, such as a Raspberry Pi 4 or 5)";

/// The system `os_release` (the file's text) describes, on `arch`, or why
/// setup cannot install on it.
pub fn detect(os_release: &str, arch: &str) -> Result<System, String> {
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().trim_matches('"').to_string())
        })
    };
    let id = field("ID").unwrap_or_default();
    let version = field("VERSION_ID").unwrap_or_default();
    let major = version.split('.').next().unwrap_or("");
    let family = match (id.as_str(), version.as_str()) {
        ("debian", "12" | "13") | ("ubuntu", "22.04" | "24.04") => Family::Apt,
        ("fedora", _) if !version.is_empty() => Family::Dnf,
        ("rocky" | "almalinux", _) if matches!(major, "8" | "9" | "10") => Family::Dnf,
        _ => {
            let name = field("PRETTY_NAME").unwrap_or_else(|| format!("{id} {version}"));
            return Err(format!(
                "setup does not support {name}; it supports {SUPPORTED}"
            ));
        }
    };
    if !matches!(arch, "x86_64" | "aarch64") {
        return Err(format!(
            "setup does not support the {arch} processor; it supports {SUPPORTED}"
        ));
    }
    Ok(System {
        id,
        version,
        family,
        arch: arch.to_string(),
    })
}

impl System {
    /// The package that adds the PostgreSQL project's repository, on the
    /// Red Hat family and Fedora.
    pub fn pgdg_rpm(&self) -> Option<String> {
        let base = "https://download.postgresql.org/pub/repos/yum/reporpms";
        match self.family {
            Family::Apt => None,
            Family::Dnf if self.id == "fedora" => Some(format!(
                "{base}/F-{}-{}/pgdg-fedora-repo-latest.noarch.rpm",
                self.version, self.arch
            )),
            Family::Dnf => Some(format!(
                "{base}/EL-{}-{}/pgdg-redhat-repo-latest.noarch.rpm",
                self.version.split('.').next().unwrap_or(&self.version),
                self.arch
            )),
        }
    }

    /// PostgreSQL 17's service.
    pub fn postgres_service(&self) -> &'static str {
        match self.family {
            Family::Apt => "postgresql",
            Family::Dnf => "postgresql-17",
        }
    }

    /// The service of PostgreSQL 17's cluster itself: stopping it returns
    /// once PostgreSQL has stopped (Debian's `postgresql` returns at once).
    pub fn postgres_cluster(&self) -> &'static str {
        match self.family {
            Family::Apt => "postgresql@17-main",
            Family::Dnf => "postgresql-17",
        }
    }

    /// PostgreSQL 17's data directory, as the packages make it.
    pub fn postgres_data(&self) -> &'static str {
        match self.family {
            Family::Apt => "/var/lib/postgresql/17/main",
            Family::Dnf => "/var/lib/pgsql/17/data",
        }
    }

    /// PostgreSQL 17's programs (pg_basebackup, pg_rewind).
    pub fn postgres_bin(&self) -> &'static str {
        match self.family {
            Family::Apt => "/usr/lib/postgresql/17/bin",
            Family::Dnf => "/usr/pgsql-17/bin",
        }
    }

    /// Its access rules.
    pub fn postgres_hba(&self) -> &'static str {
        match self.family {
            Family::Apt => "/etc/postgresql/17/main/pg_hba.conf",
            Family::Dnf => "/var/lib/pgsql/17/data/pg_hba.conf",
        }
    }

    /// The postgres account's home, where its password file goes.
    pub fn postgres_home(&self) -> &'static str {
        match self.family {
            Family::Apt => "/var/lib/postgresql",
            Family::Dnf => "/var/lib/pgsql",
        }
    }

    /// The data directories of PostgreSQL 17 that setup makes: the
    /// package's, and pg_autoctl's own on a database machine or the referee.
    pub fn our_data(&self) -> Vec<String> {
        vec![
            self.postgres_data().to_string(),
            format!("{}/17/sp", self.postgres_home()),
            format!("{}/17/monitor", self.postgres_home()),
        ]
    }

    /// What this machine has of PostgreSQL before setup changes anything.
    pub fn seen(&self) -> Seen {
        use std::net::{SocketAddr, TcpStream};
        use std::path::Path;
        use std::time::Duration;
        let folders = |dir: &str| -> Vec<String> {
            let mut names: Vec<String> = std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect();
            names.sort();
            names
        };
        let version = |path: &str| {
            std::fs::read_to_string(path)
                .ok()
                .map(|v| v.trim().to_string())
        };
        Seen {
            etc_versions: folders("/etc/postgresql"),
            system_data: version("/var/lib/pgsql/data/PG_VERSION"),
            pgsql_versions: folders("/var/lib/pgsql")
                .into_iter()
                .filter(|v| Path::new(&format!("/var/lib/pgsql/{v}/data/PG_VERSION")).exists())
                .collect(),
            port_taken: ["127.0.0.1:5432", "[::1]:5432"].iter().any(|a| {
                a.parse::<SocketAddr>().is_ok_and(|addr| {
                    TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok()
                })
            }),
            ours: self
                .our_data()
                .iter()
                .any(|data| Path::new(data).join("PG_VERSION").exists()),
        }
    }
}

/// What a machine has of PostgreSQL, seen before setup starts.
#[derive(Debug, Default)]
pub struct Seen {
    /// The version folders in `/etc/postgresql` (Debian and Ubuntu).
    pub etc_versions: Vec<String>,
    /// The version in `/var/lib/pgsql/data/PG_VERSION`: the system's own
    /// PostgreSQL, on the Red Hat family and Fedora.
    pub system_data: Option<String>,
    /// Each version `v` with `/var/lib/pgsql/<v>/data/PG_VERSION`.
    pub pgsql_versions: Vec<String>,
    /// Something listens on port 5432 of this machine.
    pub port_taken: bool,
    /// PostgreSQL 17's data directory has its PG_VERSION: an earlier setup's.
    pub ours: bool,
}

/// Another PostgreSQL, or a program on its port, that this machine has.
fn other_found(seen: &Seen) -> Option<String> {
    let other = |v: &&String| *v != "17";
    if let Some(v) = seen.etc_versions.iter().find(other) {
        Some(format!("PostgreSQL {v} (/etc/postgresql/{v})"))
    } else if let Some(v) = &seen.system_data {
        Some(format!("PostgreSQL {v} (/var/lib/pgsql/data)"))
    } else if let Some(v) = seen.pgsql_versions.iter().find(other) {
        Some(format!("PostgreSQL {v} (/var/lib/pgsql/{v}/data)"))
    } else if seen.port_taken && !seen.ours {
        Some("a program on port 5432".into())
    } else {
        None
    }
}

/// `other_postgres` for a machine joining as a database machine or the
/// referee.
pub fn other_postgres_joining(seen: &Seen) -> Result<(), String> {
    match other_found(seen) {
        None => Ok(()),
        Some(found) => Err(format!(
            "this machine already has {found}, which would clash with the PostgreSQL 17 it needs on port 5432: remove it, or join another machine"
        )),
    }
}

/// Refuses a machine that already runs a PostgreSQL other than the 17 setup
/// installs: setup would turn on archiving in it and restart it.
pub fn other_postgres(seen: &Seen) -> Result<(), String> {
    let Some(found) = other_found(seen) else {
        return Ok(());
    };
    Err(format!(
        "this machine already has {found}. Simple setup installs its own PostgreSQL 17 on port 5432 and sets it up for ScannerPulse, which would change the one you have. Remove it first, or choose Advanced in setup and give it as a database you run (it must be PostgreSQL 17)."
    ))
}

#[cfg(test)]
mod tests {
    use super::{Family, Seen, detect, other_postgres, other_postgres_joining};

    const DEBIAN: &str = "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"13\"\nID=debian\n";
    const ROCKY: &str = "NAME=\"Rocky Linux\"\nVERSION=\"9.6 (Blue Onyx)\"\nID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\nVERSION_ID=\"9.6\"\n";

    #[test]
    fn a_cluster_of_ours_is_not_another_postgresql() {
        // The package's cluster, and pg_autoctl's own on a database
        // machine or the referee.
        assert_eq!(
            detect(DEBIAN, "x86_64").unwrap().our_data(),
            [
                "/var/lib/postgresql/17/main",
                "/var/lib/postgresql/17/sp",
                "/var/lib/postgresql/17/monitor"
            ]
        );
        assert_eq!(
            detect(ROCKY, "x86_64").unwrap().our_data(),
            [
                "/var/lib/pgsql/17/data",
                "/var/lib/pgsql/17/sp",
                "/var/lib/pgsql/17/monitor"
            ]
        );
    }

    #[test]
    fn a_supported_system_is_found_with_its_package_tools() {
        let debian = detect(DEBIAN, "aarch64").unwrap();
        assert_eq!(
            (debian.id.as_str(), debian.version.as_str(), debian.family),
            ("debian", "13", Family::Apt)
        );
        assert_eq!(debian.pgdg_rpm(), None);
        assert_eq!(debian.postgres_service(), "postgresql");
        assert_eq!(debian.postgres_cluster(), "postgresql@17-main");
        let rocky = detect(ROCKY, "x86_64").unwrap();
        assert_eq!(rocky.family, Family::Dnf);
        assert_eq!(rocky.postgres_cluster(), "postgresql-17");
        assert_eq!(
            rocky.pgdg_rpm().unwrap(),
            "https://download.postgresql.org/pub/repos/yum/reporpms/EL-9-x86_64/pgdg-redhat-repo-latest.noarch.rpm"
        );
        let fedora = detect("ID=fedora\nVERSION_ID=42\n", "aarch64").unwrap();
        assert!(
            fedora
                .pgdg_rpm()
                .unwrap()
                .ends_with("/F-42-aarch64/pgdg-fedora-repo-latest.noarch.rpm")
        );
        assert_eq!(fedora.postgres_data(), "/var/lib/pgsql/17/data");
    }

    #[test]
    fn an_unsupported_system_or_processor_is_refused_by_name() {
        let old = detect(
            "PRETTY_NAME=\"Debian GNU/Linux 11 (bullseye)\"\nID=debian\nVERSION_ID=\"11\"\n",
            "x86_64",
        );
        let message = old.unwrap_err();
        assert!(
            message.contains("Debian GNU/Linux 11 (bullseye)")
                && message.contains("Debian 12 and 13"),
            "{message}"
        );
        assert!(detect("ID=alpine\nVERSION_ID=3.20\n", "x86_64").is_err());
        assert!(detect(DEBIAN, "armv7l").unwrap_err().contains("armv7l"));
    }

    #[test]
    fn another_postgresql_on_the_machine_is_refused_by_what_was_seen() {
        let refused = |seen: Seen| other_postgres(&seen).unwrap_err();
        let debian = refused(Seen {
            etc_versions: vec!["15".into()],
            port_taken: true,
            ..Seen::default()
        });
        assert_eq!(
            debian,
            "this machine already has PostgreSQL 15 (/etc/postgresql/15). Simple setup installs its own PostgreSQL 17 on port 5432 and sets it up for ScannerPulse, which would change the one you have. Remove it first, or choose Advanced in setup and give it as a database you run (it must be PostgreSQL 17)."
        );
        assert!(
            refused(Seen {
                etc_versions: vec!["17".into(), "16".into()],
                ours: true,
                ..Seen::default()
            })
            .contains("has PostgreSQL 16 (/etc/postgresql/16).")
        );
        assert!(
            refused(Seen {
                system_data: Some("13".into()),
                ..Seen::default()
            })
            .contains("has PostgreSQL 13 (/var/lib/pgsql/data).")
        );
        assert!(
            refused(Seen {
                pgsql_versions: vec!["16".into()],
                ..Seen::default()
            })
            .contains("has PostgreSQL 16 (/var/lib/pgsql/16/data).")
        );
        assert!(
            refused(Seen {
                port_taken: true,
                ..Seen::default()
            })
            .starts_with("this machine already has a program on port 5432. Simple setup")
        );
        // Nothing there; only 17; a second run, with 17's data and port.
        assert_eq!(other_postgres(&Seen::default()), Ok(()));
        // A joining machine is told in its own words.
        let error = other_postgres_joining(&Seen {
            port_taken: true,
            ..Seen::default()
        })
        .unwrap_err();
        assert!(error.contains("a program on port 5432"), "{error}");
        assert!(!error.contains("Simple setup"), "{error}");
        let only_17 = Seen {
            etc_versions: vec!["17".into()],
            ours: true,
            port_taken: true,
            ..Seen::default()
        };
        assert_eq!(other_postgres(&only_17), Ok(()));
        let rerun = Seen {
            pgsql_versions: vec!["17".into()],
            ours: true,
            port_taken: true,
            ..Seen::default()
        };
        assert_eq!(other_postgres(&rerun), Ok(()));
    }
}
