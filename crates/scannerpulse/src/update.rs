//! `scannerpulse update`: the newest release, checked against the key that
//! signs every release, put on each machine of the install in turn. The
//! database machines and the referee, which serve no visitors, go first,
//! then the second ScannerPulse machine, then this one: a pair stays up
//! throughout. The schema changes once, first, and stays additive, so the
//! machines not yet updated keep running meanwhile.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// This program's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where an update's downloads wait, root's alone.
const UPDATES: &str = "/usr/local/lib/scannerpulse/update";

/// How long a restarted machine may take to answer `/healthz`.
const HEALTHY_WAIT: Duration = Duration::from_secs(120);

/// Where releases are found: the newest one's description, and its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub latest: String,
    pub download: String,
}

impl Source {
    /// The project's releases, or SCANNERPULSE_RELEASES's (a test's own
    /// server), whose files are checked the same way.
    pub fn from_env() -> Source {
        match std::env::var("SCANNERPULSE_RELEASES") {
            Ok(base) if !base.is_empty() => Source::at(&base),
            _ => Source {
                latest: "https://api.github.com/repos/jhenion209/scannerpulse/releases/latest"
                    .into(),
                download: "https://github.com/jhenion209/scannerpulse/releases/download".into(),
            },
        }
    }

    /// `<base>/latest` describes the newest release, as GitHub's API does;
    /// `<base>/download/<tag>/<file>` serves its files.
    pub fn at(base: &str) -> Source {
        let base = base.trim_end_matches('/');
        Source {
            latest: format!("{base}/latest"),
            download: format!("{base}/download"),
        }
    }
}

/// A release: its tag, its version and its notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub notes: String,
}

/// The release that GitHub's `releases/latest` answer describes.
pub fn release_from(json: &serde_json::Value) -> Result<Release, String> {
    let tag = json["tag_name"]
        .as_str()
        .ok_or("the newest release names no tag")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if parse_version(version).is_none() {
        return Err(format!("the newest release's tag {tag:?} is not a version"));
    }
    Ok(Release {
        tag: tag.to_string(),
        version: version.to_string(),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
    })
}

fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.split('.').map(|part| part.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

/// `offered` is a later version than `current`.
pub fn newer(current: &str, offered: &str) -> bool {
    match (parse_version(current), parse_version(offered)) {
        (Some(current), Some(offered)) => offered > current,
        _ => false,
    }
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        // GitHub's API refuses a request without one.
        .user_agent(concat!("scannerpulse/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())
}

/// The newest release.
pub async fn latest(source: &Source) -> Result<Release, String> {
    let text = client()?
        .get(&source.latest)
        .header("accept", "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| {
            format!(
                "cannot read the newest release at {}: {error}",
                source.latest
            )
        })?
        .text()
        .await
        .map_err(|error| {
            format!(
                "cannot read the newest release at {}: {error}",
                source.latest
            )
        })?;
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| format!("the newest release's description cannot be read: {error}"))?;
    release_from(&json)
}

/// The program's file in a release, for a processor (`x86_64`, `aarch64`).
pub fn program(arch: &str) -> String {
    format!("scannerpulse-{arch}-linux")
}

/// The public key that signs every release: the install script's own, or,
/// in a test build, the one SCANNERPULSE_RELEASE_KEY named at build time.
pub fn release_key() -> Result<Vec<u8>, String> {
    let text = option_env!("SCANNERPULSE_RELEASE_KEY")
        .map(str::to_string)
        .or_else(|| key_in(include_str!("../../../install.sh")))
        .ok_or("this program carries no release key")?;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|error| format!("the release key cannot be read: {error}"))
}

/// The install script's RELEASE_KEY, so the two never differ.
fn key_in(script: &str) -> Option<String> {
    script.lines().find_map(|line| {
        line.strip_prefix("RELEASE_KEY='")?
            .strip_suffix('\'')
            .map(str::to_string)
    })
}

/// Downloads the release's checksums and the programs for `arches` into
/// `dir`, then checks the checksums' signature against `key` and each
/// program against its checksum.
pub async fn fetch(
    source: &Source,
    release: &Release,
    arches: &[String],
    dir: &Path,
    key: &[u8],
) -> Result<(), String> {
    let client = client()?;
    let get = |file: String| {
        let url = format!("{}/{}/{file}", source.download, release.tag);
        let client = client.clone();
        async move {
            client
                .get(&url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|error| format!("cannot download {url}: {error}"))?
                .bytes()
                .await
                .map_err(|error| format!("cannot download {url}: {error}"))
        }
    };
    for file in ["SHA256SUMS", "SHA256SUMS.sig"] {
        let bytes = get(file.to_string()).await?;
        std::fs::write(dir.join(file), &bytes).map_err(|error| format!("{file}: {error}"))?;
    }
    signed(dir, key)?;
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS"))
        .map_err(|error| format!("SHA256SUMS: {error}"))?;
    for arch in arches {
        let name = program(arch);
        let bytes = get(name.clone()).await?;
        checked(&sums, &name, &bytes)?;
        let path = dir.join(&name);
        std::fs::write(&path, &bytes).map_err(|error| format!("{name}: {error}"))?;
        // This machine's runs the schema change before it is put in place.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(())
}

/// The checksums in `dir` carry the release key's signature, by gpgv or,
/// without it, sqv: the install script's two.
pub fn signed(dir: &Path, key: &[u8]) -> Result<(), String> {
    let keyring = dir.join("release-key.gpg");
    std::fs::write(&keyring, key).map_err(|error| format!("the release key: {error}"))?;
    let (sig, sums) = (dir.join("SHA256SUMS.sig"), dir.join("SHA256SUMS"));
    let run = |program: &str| {
        let mut command = Command::new(program);
        if program == "gpgv" {
            command.env("GNUPGHOME", dir);
        }
        command
            .arg("--keyring")
            .arg(&keyring)
            .args([&sig, &sums])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    let status = match run("gpgv") {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => run("sqv"),
        other => other,
    };
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err("the release's signature does not check out: nothing was changed".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(
            "gpgv checks a release's signature: install it first (Debian and Ubuntu: apt-get install gpgv; Fedora, Rocky and AlmaLinux: dnf install gnupg2)".into(),
        ),
        Err(error) => Err(format!("cannot check the release's signature: {error}")),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `bytes` match `program`'s line in SHA256SUMS.
pub fn checked(sums: &str, program: &str, bytes: &[u8]) -> Result<(), String> {
    let want = sums
        .lines()
        .find_map(|line| {
            let (hash, name) = line.split_once("  ")?;
            (name == program).then_some(hash)
        })
        .ok_or_else(|| format!("the release's checksums name no {program}"))?;
    if want.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        Ok(())
    } else {
        Err(format!(
            "{program} does not match its signed checksum: nothing was changed"
        ))
    }
}

/// Puts `bytes` in place of `binary`, once they match `sha256` and run, and
/// returns the version they give. The old program stays until then.
pub fn install(binary: &Path, bytes: &[u8], sha256: &str) -> Result<String, String> {
    if !sha256.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        return Err("the program received does not match its checksum: nothing was changed".into());
    }
    // Beside the old one, so that the rename is one step.
    let new = binary.with_file_name(".scannerpulse.update");
    std::fs::write(&new, bytes).map_err(|error| format!("{}: {error}", new.display()))?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("{}: {error}", new.display()))?;
    let version = Command::new(&new)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .strip_prefix("scannerpulse ")
                .map(str::to_string)
        });
    let Some(version) = version else {
        let _ = std::fs::remove_file(&new);
        return Err("the program received does not run here: nothing was changed".into());
    };
    std::fs::rename(&new, binary).map_err(|error| format!("{}: {error}", binary.display()))?;
    Ok(version)
}

/// The services that run the program: those setup made on this machine
/// (none on a database machine or the referee, whose timers start the
/// program afresh each time).
pub fn services_here(units: &Path) -> Vec<&'static str> {
    ["scannerpulse-web.service", "scannerpulse-ingest.service"]
        .into_iter()
        .filter(|unit| units.join(unit).exists())
        .collect()
}

/// Restarts this machine's ScannerPulse services that run, so that they run
/// the program now in place.
pub fn restart() -> Result<(), String> {
    let services = services_here(Path::new("/etc/systemd/system"));
    if services.is_empty() {
        return Ok(());
    }
    let output = Command::new("systemctl")
        .arg("try-restart")
        .args(&services)
        .output()
        .map_err(|error| format!("cannot run systemctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the services did not restart: {}",
            crate::setup::apply::said(&output)
        ))
    }
}

/// What `node serve version` gives: this program's version and processor.
pub fn running_here() -> String {
    format!("scannerpulse {VERSION} {}", std::env::consts::ARCH)
}

/// A machine's answer to `version`: its version and processor.
pub fn running(answer: &str) -> Option<(String, String)> {
    match answer.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["scannerpulse", version, arch] => Some((version.to_string(), arch.to_string())),
        _ => None,
    }
}

/// Another machine of the install, in the order they are updated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Other {
    pub name: String,
    /// The second ScannerPulse machine, which serves the site: it must
    /// answer again before the next machine restarts.
    pub second: bool,
}

/// The plan's other machines that have joined: the referee and the
/// database machines, which serve no visitors, then the second ScannerPulse
/// machine.
pub fn others(plan: &crate::setup::plan::Plan) -> Vec<Other> {
    let joined = |name: &str| plan.joined.iter().any(|joined| joined == name);
    plan.referee
        .iter()
        .chain(plan.nodes.iter())
        .filter(|name| joined(name))
        .map(|name| Other {
            name: name.clone(),
            second: false,
        })
        .chain(
            plan.second
                .iter()
                .filter(|name| joined(name))
                .map(|name| Other {
                    name: name.clone(),
                    second: true,
                }),
        )
        .collect()
}

/// Why `name` did not say which version it runs: a program from before
/// 0.8.0 does not know the question, and the rest is ssh's answer.
fn why_not(name: &str, answer: Result<String, String>) -> String {
    match answer {
        Err(error) if error.contains("is not a database machine's command") => format!(
            "{name} runs a version from before 0.8.0, which cannot be updated from here: run the install line there once (on the second ScannerPulse machine, then also `sudo /usr/local/bin/scannerpulse setup` and \"join an existing install\" again)"
        ),
        // A second machine that joined before 0.8.0 never let the key in.
        Err(error) if error.contains("Permission denied") => format!(
            "{name} does not let this machine's key in: run `sudo /usr/local/bin/scannerpulse setup` there and \"join an existing install\" again"
        ),
        Err(error) => format!("{name} cannot be reached ({error}): check that it is up"),
        Ok(answer) => format!("{name} gave an answer that is not a version ({answer:?})"),
    }
}

/// Waits until `https://<host>:<port>/healthz` answers 200. Its certificate
/// is not checked: this asks only whether the site is up.
async fn healthy(host: &str, port: u16) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())?;
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let url = format!("https://{host}:{port}/healthz");
    let started = std::time::Instant::now();
    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status().is_success()
        {
            return Ok(());
        }
        if started.elapsed() > HEALTHY_WAIT {
            return Err(format!(
                "{url} did not answer within {} s; see `journalctl -u scannerpulse-web` there",
                HEALTHY_WAIT.as_secs()
            ));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// `scannerpulse update [--check] [--yes]`, as root on the first ScannerPulse
/// machine (or on one machine alone): `say` prints each step, `confirm`
/// asks before anything changes.
pub async fn run(
    check_only: bool,
    say: impl Fn(&str),
    confirm: impl Fn(&str) -> bool,
) -> Result<String, String> {
    use crate::setup::plan::{PLAN, Plan};
    let settings = std::fs::read_to_string(crate::setup::steps::env_path()).map_err(|_| {
        "this machine runs no ScannerPulse site: run `sudo /usr/local/bin/scannerpulse update` on the first ScannerPulse machine, which updates every machine of its install (a machine not yet set up takes the install line)".to_string()
    })?;
    let plan_here = Path::new(PLAN).exists();
    if crate::setup::second::is_second(&settings, plan_here) {
        let first = crate::layout::Layout::read(Path::new(crate::layout::PATH))
            .map(|layout| layout.hub)
            .unwrap_or_else(|| "the first machine".into());
        return Err(format!(
            "this is the second ScannerPulse machine: run `sudo /usr/local/bin/scannerpulse update` on {first}, which updates every machine"
        ));
    }
    let plan = if plan_here {
        Some(Plan::read(Path::new(PLAN))?)
    } else {
        None
    };
    let port = crate::setup::second::port_of(&settings)?;
    let source = Source::from_env();
    say(&format!(
        "This machine runs ScannerPulse {VERSION}. Reading the newest release..."
    ));
    let release = latest(&source).await?;
    let target = release.version.clone();
    // Each other machine's version and processor, before anything changes:
    // one that cannot be reached stops the update here.
    let others = plan.as_ref().map(others).unwrap_or_default();
    let mut found = Vec::new();
    let mut unreachable = Vec::new();
    for other in &others {
        let answer = crate::setup::node::ask(&other.name, "version", &[]);
        match answer.as_deref().ok().and_then(running) {
            Some((version, arch)) => found.push((other.clone(), version, arch)),
            None => unreachable.push(why_not(&other.name, answer)),
        }
    }
    if !unreachable.is_empty() {
        return Err(format!(
            "nothing was changed: {}; then run update here again",
            unreachable.join("; ")
        ));
    }
    let pending: Vec<_> = found
        .iter()
        .filter(|(_, version, _)| version != &target)
        .collect();
    let mine = VERSION != target;
    let mut lines = vec![format!("The newest release is {target}.")];
    for (other, version, _) in &found {
        lines.push(format!("  {}: {version}", other.name));
    }
    lines.push(format!("  this machine: {VERSION}"));
    say(&lines.join("\n"));
    if check_only {
        if !release.notes.trim().is_empty() {
            say(&format!("Its notes:\n{}", release.notes.trim()));
        }
        return Ok(if pending.is_empty() && !mine {
            "Every machine runs the newest release.".into()
        } else {
            "An update is available: `sudo /usr/local/bin/scannerpulse update` installs it.".into()
        });
    }
    if pending.is_empty() && !mine {
        return Ok(format!(
            "Every machine runs ScannerPulse {target}: nothing to do."
        ));
    }
    if !newer(VERSION, &target) && mine {
        return Err(format!(
            "this machine runs {VERSION}, newer than the newest release ({target}): nothing was changed"
        ));
    }
    let order: Vec<String> = pending
        .iter()
        .map(|(other, _, _)| other.name.clone())
        .chain(mine.then(|| "this machine".to_string()))
        .collect();
    if !confirm(&format!(
        "Update {} to {target}, one at a time? Each machine's ScannerPulse services restart.",
        order.join(", then ")
    )) {
        return Ok("Nothing was changed.".into());
    }
    let mut arches: Vec<String> = pending.iter().map(|(_, _, arch)| arch.clone()).collect();
    arches.push(std::env::consts::ARCH.to_string());
    arches.sort();
    arches.dedup();
    // Not under /tmp or /var/tmp, which a hardened system mounts noexec:
    // the new program runs from here once, for the schema.
    let dir = PathBuf::from(format!("{UPDATES}/{}", std::process::id()));
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    let result = async {
        say(&format!("Downloading {target} and checking its signature..."));
        fetch(&source, &release, &arches, &dir, &release_key()?).await?;
        let here = dir.join(program(std::env::consts::ARCH));
        say("Bringing the database's tables up to date...");
        let output = Command::new(&here)
            .args(["db-update", "--quiet"])
            .output()
            .map_err(|error| format!("cannot run the new program: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "the database's tables could not be brought up to date, so no machine was changed: {}",
                crate::setup::apply::said(&output)
            ));
        }
        for (other, version, arch) in &pending {
            say(&format!("Updating {} ({version} to {target})...", other.name));
            let bytes = std::fs::read(dir.join(program(arch)))
                .map_err(|error| format!("{}: {error}", program(arch)))?;
            let answer = crate::setup::node::ask(
                &other.name,
                &format!("update {}", sha256_hex(&bytes)),
                &bytes,
            )?;
            say(&format!("  {}: {answer}", other.name));
            if other.second {
                healthy(&other.name, port).await?;
                say(&format!("  {} answers again.", other.name));
            }
        }
        if mine {
            say(&format!("Updating this machine ({VERSION} to {target})..."));
            let bytes = std::fs::read(&here).map_err(|error| format!("{}: {error}", here.display()))?;
            install(Path::new(crate::setup::BINARY), &bytes, &sha256_hex(&bytes))?;
            restart()?;
            healthy("127.0.0.1", port).await?;
            say("  This machine answers again.");
        }
        Ok(if others.is_empty() {
            format!("ScannerPulse {target} runs here.")
        } else {
            format!("ScannerPulse {target} runs on every machine.")
        })
    }
    .await;
    let _ = std::fs::remove_dir_all(&dir);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_version_is_newer_and_an_unreadable_one_never_is() {
        assert!(newer("0.7.0", "0.8.0"));
        assert!(newer("0.7.9", "0.7.10"));
        assert!(newer("0.9.0", "1.0.0"));
        assert!(!newer("0.8.0", "0.8.0"));
        assert!(!newer("0.8.1", "0.8.0"));
        assert!(!newer("0.8.0", "0.8"));
        assert!(!newer("0.8.0", "0.9.0-rc1"));
    }

    #[test]
    fn the_newest_release_is_read_from_its_tag_and_notes() {
        let release =
            release_from(&serde_json::json!({"tag_name": "v0.8.0", "body": "Updates."})).unwrap();
        assert_eq!(
            release,
            Release {
                tag: "v0.8.0".into(),
                version: "0.8.0".into(),
                notes: "Updates.".into()
            }
        );
        assert!(release_from(&serde_json::json!({"tag_name": "nightly"})).is_err());
        assert!(release_from(&serde_json::json!({})).is_err());
    }

    #[test]
    fn the_release_key_is_the_install_scripts() {
        let key = release_key().unwrap();
        // An OpenPGP public key packet (old format, tag 6).
        assert_eq!(key[0] & 0xfc, 0x98, "{:02x}", key[0]);
        assert_eq!(key_in("x\nRELEASE_KEY='abc'\n"), Some("abc".into()));
        assert_eq!(key_in("RELEASE_KEY=abc\n"), None);
    }

    #[test]
    fn a_program_matches_only_its_own_signed_checksum() {
        let sums = format!(
            "{}  scannerpulse-x86_64-linux\n{}  scannerpulse-aarch64-linux\n",
            sha256_hex(b"x86"),
            sha256_hex(b"arm")
        );
        assert!(checked(&sums, "scannerpulse-x86_64-linux", b"x86").is_ok());
        let wrong = checked(&sums, "scannerpulse-x86_64-linux", b"arm").unwrap_err();
        assert!(wrong.contains("does not match"), "{wrong}");
        let missing = checked(&sums, "scannerpulse-riscv64-linux", b"x86").unwrap_err();
        assert!(missing.contains("name no"), "{missing}");
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update")
    }

    fn fixture_key() -> Vec<u8> {
        let text = std::fs::read_to_string(fixtures().join("key.b64")).unwrap();
        base64::engine::general_purpose::STANDARD
            .decode(text.trim())
            .unwrap()
    }

    #[test]
    fn checksums_signed_by_the_key_pass_and_changed_ones_do_not() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["SHA256SUMS", "SHA256SUMS.sig"] {
            std::fs::copy(fixtures().join(file), dir.path().join(file)).unwrap();
        }
        signed(dir.path(), &fixture_key()).unwrap();
        // The project's own key did not sign the test's checksums.
        assert!(signed(dir.path(), &release_key().unwrap()).is_err());
        let sums = dir.path().join("SHA256SUMS");
        let text = std::fs::read_to_string(&sums).unwrap();
        std::fs::write(&sums, text.replacen('6', "7", 1)).unwrap();
        let error = signed(dir.path(), &fixture_key()).unwrap_err();
        assert!(error.contains("does not check out"), "{error}");
    }

    #[test]
    fn a_program_is_put_in_place_only_when_it_matches_and_runs() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("scannerpulse");
        std::fs::write(&binary, "old").unwrap();
        let new = b"#!/bin/sh\necho 'scannerpulse 9.9.9'\n";
        let wrong = install(&binary, new, &sha256_hex(b"other")).unwrap_err();
        assert!(wrong.contains("does not match"), "{wrong}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        let broken = b"#!/bin/sh\nexit 1\n";
        let error = install(&binary, broken, &sha256_hex(broken)).unwrap_err();
        assert!(error.contains("does not run"), "{error}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        assert!(!dir.path().join(".scannerpulse.update").exists());
        assert_eq!(install(&binary, new, &sha256_hex(new)).unwrap(), "9.9.9");
        assert_eq!(std::fs::read(&binary).unwrap(), new);
    }

    #[test]
    fn only_the_services_setup_made_here_are_restarted() {
        let units = tempfile::tempdir().unwrap();
        // A database machine or the referee: nothing to restart.
        assert!(services_here(units.path()).is_empty());
        for unit in ["scannerpulse-web.service", "scannerpulse-ingest.service"] {
            std::fs::write(units.path().join(unit), "").unwrap();
        }
        assert_eq!(
            services_here(units.path()),
            vec!["scannerpulse-web.service", "scannerpulse-ingest.service"]
        );
    }

    #[test]
    fn a_machine_that_cannot_say_its_version_is_told_apart_by_why() {
        let old = why_not(
            "db1",
            Err("db1: \"version\" is not a database machine's command: status, stop, start".into()),
        );
        assert!(
            old.contains("before 0.8.0") && old.contains("install line"),
            "{old}"
        );
        let down = why_not(
            "db2",
            Err("db2: ssh: connect to host db2 port 22: No route to host".into()),
        );
        assert!(
            down.contains("cannot be reached") && down.contains("No route to host"),
            "{down}"
        );
        assert!(!down.contains("install line"), "{down}");
        let shut = why_not(
            "web2",
            Err("web2: root@web2: Permission denied (publickey).".into()),
        );
        assert!(shut.contains("join an existing install"), "{shut}");
    }

    #[test]
    fn a_machines_answer_gives_its_version_and_processor() {
        assert_eq!(
            running("scannerpulse 0.8.0 aarch64\n"),
            Some(("0.8.0".into(), "aarch64".into()))
        );
        assert_eq!(
            running("\"version\" is not a database machine's command"),
            None
        );
        assert_eq!(running(&running_here()).unwrap().0, VERSION);
    }

    #[test]
    fn machines_that_serve_no_visitors_go_first_and_the_second_last() {
        let mut plan: crate::setup::plan::Plan = serde_json::from_value(serde_json::json!({
            "hub": "192.0.2.10", "hub_key": "ssh-ed25519 AAAAhub", "database": "scannerpulse",
            "role": "scannerpulse", "password": "x", "replication_password": "y",
            "nodes": ["db1.example.org", "db2.example.org"],
            "referee": "ref.example.org", "second": "web2.example.org",
            "joined": ["web2.example.org", "db2.example.org", "ref.example.org", "db1.example.org"],
        }))
        .unwrap();
        let names: Vec<(String, bool)> = others(&plan)
            .into_iter()
            .map(|other| (other.name, other.second))
            .collect();
        assert_eq!(
            names,
            vec![
                ("ref.example.org".into(), false),
                ("db1.example.org".into(), false),
                ("db2.example.org".into(), false),
                ("web2.example.org".into(), true),
            ]
        );
        // A machine that has not joined is not reached.
        plan.joined.retain(|name| name != "web2.example.org");
        assert!(others(&plan).iter().all(|other| !other.second));
    }
}
