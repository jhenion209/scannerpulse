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

    /// PostgreSQL 17's data directory, as the packages make it.
    pub fn postgres_data(&self) -> &'static str {
        match self.family {
            Family::Apt => "/var/lib/postgresql/17/main",
            Family::Dnf => "/var/lib/pgsql/17/data",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Family, detect};

    const DEBIAN: &str = "PRETTY_NAME=\"Debian GNU/Linux 13 (trixie)\"\nNAME=\"Debian GNU/Linux\"\nVERSION_ID=\"13\"\nID=debian\n";
    const ROCKY: &str = "NAME=\"Rocky Linux\"\nVERSION=\"9.6 (Blue Onyx)\"\nID=\"rocky\"\nID_LIKE=\"rhel centos fedora\"\nVERSION_ID=\"9.6\"\n";

    #[test]
    fn a_supported_system_is_found_with_its_package_tools() {
        let debian = detect(DEBIAN, "aarch64").unwrap();
        assert_eq!(
            (debian.id.as_str(), debian.version.as_str(), debian.family),
            ("debian", "13", Family::Apt)
        );
        assert_eq!(debian.pgdg_rpm(), None);
        assert_eq!(debian.postgres_service(), "postgresql");
        let rocky = detect(ROCKY, "x86_64").unwrap();
        assert_eq!(rocky.family, Family::Dnf);
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
}
