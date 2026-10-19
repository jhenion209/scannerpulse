//! `scannerpulse update`: the newest release, checked against the key that
//! signs every release, put on each machine of the install in turn. The
//! database machines and the referee, which serve no visitors, go first,
//! then the second ScannerPulse machine, then this one: a pair stays up
//! throughout. The schema changes once, first, and stays additive, so the
//! machines not yet updated keep running meanwhile.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::io::Read;
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

/// Puts `bytes` in place of `binary`, once they match `sha256`, run, and
/// give a version no older than `current`, and returns that version. The old
/// program stays until then, and the new one is on disk before the rename,
/// so a power cut leaves one or the other.
pub fn install(binary: &Path, bytes: &[u8], sha256: &str, current: &str) -> Result<String, String> {
    if !sha256.eq_ignore_ascii_case(&sha256_hex(bytes)) {
        return Err("the program received does not match its checksum: nothing was changed".into());
    }
    // Beside the old one, so that the rename is one step.
    let new = binary.with_file_name(".scannerpulse.update");
    let fail = |error: std::io::Error| format!("{}: {error}", new.display());
    {
        let mut file = std::fs::File::create(&new).map_err(fail)?;
        std::io::Write::write_all(&mut file, bytes).map_err(fail)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o755))
            .map_err(fail)?;
        file.sync_all().map_err(fail)?;
    }
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
    let refused = match &version {
        None => Some("the program received does not run here: nothing was changed".to_string()),
        Some(version) if newer(version, current) => Some(format!(
            "the program received ({version}) is older than {current}, which runs here: nothing was changed"
        )),
        Some(_) => None,
    };
    if let Some(refused) = refused {
        let _ = std::fs::remove_file(&new);
        return Err(refused);
    }
    std::fs::rename(&new, binary).map_err(|error| format!("{}: {error}", binary.display()))?;
    if let Some(folder) = binary.parent() {
        let _ = std::fs::File::open(folder).and_then(|folder| folder.sync_all());
    }
    Ok(version.unwrap_or_default())
}

/// The most `node serve update` reads of the checksums or the signature,
/// and of the program.
const LIST_MOST: usize = 64 * 1024;
const PROGRAM_MOST: u64 = 256 * 1024 * 1024;

/// What the first machine sends with `update NAME`: the release's checksums
/// and their signature, each after its length on a line of its own, then
/// the program.
pub fn bundle(sums: &[u8], sig: &[u8], program: &[u8]) -> Vec<u8> {
    let mut sent = Vec::with_capacity(sums.len() + sig.len() + program.len() + 16);
    for part in [sums, sig] {
        sent.extend_from_slice(format!("{}\n", part.len()).as_bytes());
        sent.extend_from_slice(part);
    }
    sent.extend_from_slice(program);
    sent
}

/// What a bundle holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Sent {
    pub sums: Vec<u8>,
    pub sig: Vec<u8>,
    pub program: Vec<u8>,
}

/// A bundle's checksums, signature and program, refusing a part that is cut
/// off or longer than its limit.
pub fn unbundle(input: &mut impl Read) -> Result<Sent, String> {
    let mut list = || -> Result<Vec<u8>, String> {
        let mut digits = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            input
                .read_exact(&mut byte)
                .map_err(|_| "the program sent was cut off".to_string())?;
            match byte[0] {
                b'\n' => break,
                digit @ b'0'..=b'9' if digits.len() < 10 => digits.push(digit),
                _ => return Err("the program sent is not laid out as `update` sends it".into()),
            }
        }
        let length: usize = String::from_utf8_lossy(&digits)
            .parse()
            .map_err(|_| "the program sent is not laid out as `update` sends it".to_string())?;
        if length > LIST_MOST {
            return Err("the checksums or signature sent are too long".into());
        }
        let mut part = vec![0u8; length];
        input
            .read_exact(&mut part)
            .map_err(|_| "the program sent was cut off".to_string())?;
        Ok(part)
    };
    let (sums, sig) = (list()?, list()?);
    let mut program = Vec::new();
    input
        .take(PROGRAM_MOST + 1)
        .read_to_end(&mut program)
        .map_err(|error| format!("cannot read the program sent: {error}"))?;
    if program.len() as u64 > PROGRAM_MOST {
        return Err("the program sent is too long".into());
    }
    Ok(Sent { sums, sig, program })
}

/// `node serve update NAME`, on the far side: the release's signature is
/// checked with this machine's own `key`, and the program `name` against its
/// checksum, so that the first machine's key puts nothing in place that the
/// release key did not sign; then it goes in place unless it is older than
/// `current`. `work` holds the checksums meanwhile.
pub fn receive(
    binary: &Path,
    name: &str,
    input: &mut impl Read,
    key: &[u8],
    work: &Path,
    current: &str,
) -> Result<String, String> {
    let Sent { sums, sig, program } = unbundle(input)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(work)
        .map_err(|error| format!("{}: {error}", work.display()))?;
    let result = (|| {
        std::fs::write(work.join("SHA256SUMS"), &sums).map_err(|error| error.to_string())?;
        std::fs::write(work.join("SHA256SUMS.sig"), &sig).map_err(|error| error.to_string())?;
        signed(work, key)?;
        checked(&String::from_utf8_lossy(&sums), name, &program)?;
        install(binary, &program, &sha256_hex(&program), current)
    })();
    let _ = std::fs::remove_dir_all(work);
    result
}

/// A program's file name in a release: `scannerpulse-<processor>-linux`.
pub fn is_program(name: &str) -> bool {
    name.strip_prefix("scannerpulse-")
        .and_then(|rest| rest.strip_suffix("-linux"))
        .is_some_and(|arch| {
            !arch.is_empty()
                && arch
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        })
}

/// The folder `node serve update` works in on this machine.
pub fn work_folder() -> PathBuf {
    PathBuf::from(format!("{UPDATES}/receive-{}", std::process::id()))
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
            "{name} does not let this machine's key in: if it runs a version from before 0.8.0, run the install line there first; then run `sudo /usr/local/bin/scannerpulse setup` there and \"join an existing install\" again"
        ),
        Err(error) => format!("{name} cannot be reached ({error}): check that it is up"),
        Ok(answer) => format!("{name} gave an answer that is not a version ({answer:?})"),
    }
}

/// The machines behind `target`, to update, and those ahead of it, named
/// with their version: an update never moves a machine back.
pub fn sort_out<'a>(
    found: &'a [(Other, String, String)],
    target: &str,
) -> (Vec<&'a (Other, String, String)>, Vec<String>) {
    let pending = found
        .iter()
        .filter(|(_, version, _)| newer(version, target))
        .collect();
    let ahead = found
        .iter()
        .filter(|(_, version, _)| newer(target, version))
        .map(|(other, version, _)| format!("{} ({version})", other.name))
        .collect();
    (pending, ahead)
}

/// Both ScannerPulse machines answer, so that each carries the site while
/// the other restarts: `second` is the second machine's name and whether it
/// answers, when there is one.
pub fn both_answer(here: bool, second: Option<(&str, bool)>) -> Result<(), String> {
    let mut silent = Vec::new();
    if !here {
        silent.push("this machine".to_string());
    }
    if let Some((name, false)) = second {
        silent.push(name.to_string());
    }
    if silent.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "nothing was changed: the site does not answer on {} (`/healthz`); the update restarts one ScannerPulse machine at a time while the other carries the site, so both must answer first",
            silent.join(" or ")
        ))
    }
}

fn health_url(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("https://[{host}]:{port}/healthz")
    } else {
        format!("https://{host}:{port}/healthz")
    }
}

fn health_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|error| error.to_string())
}

/// `https://<host>:<port>/healthz` answers 200 now.
async fn answers(host: &str, port: u16) -> bool {
    match health_client() {
        Ok(client) => client
            .get(health_url(host, port))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success()),
        Err(_) => false,
    }
}

/// Waits until `https://<host>:<port>/healthz` answers 200. Its certificate
/// is not checked: this asks only whether the site is up.
async fn healthy(host: &str, port: u16) -> Result<(), String> {
    let client = health_client()?;
    let url = health_url(host, port);
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
    let (pending, ahead) = sort_out(&found, &target);
    let mine = newer(VERSION, &target);
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
        return Ok(if !ahead.is_empty() || newer(&target, VERSION) {
            "A machine runs a version newer than the newest release: update leaves the install as it is.".into()
        } else if pending.is_empty() && !mine {
            "Every machine runs the newest release.".into()
        } else {
            "An update is available: `sudo /usr/local/bin/scannerpulse update` installs it.".into()
        });
    }
    // An update never moves a machine back: the newest release may be one
    // pulled back to an older version.
    let mut ahead = ahead;
    if newer(&target, VERSION) {
        ahead.push(format!("this machine ({VERSION})"));
    }
    if !ahead.is_empty() {
        return Err(format!(
            "{} runs a version newer than the newest release ({target}): nothing was changed",
            ahead.join(", ")
        ));
    }
    if pending.is_empty() && !mine {
        return Ok(format!(
            "Every machine runs ScannerPulse {target}: nothing to do."
        ));
    }
    let second = others.iter().find(|other| other.second);
    let second_answers = match second {
        Some(second) => Some((second.name.as_str(), answers(&second.name, port).await)),
        None => None,
    };
    if second.is_some() {
        both_answer(answers("127.0.0.1", port).await, second_answers)?;
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
        // The signature covers the checksums, not the version: the program
        // must say it is the release it was downloaded as.
        let output = Command::new(&here)
            .arg("--version")
            .output()
            .map_err(|error| format!("cannot run the new program: {error}"))?;
        let says = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if says != format!("scannerpulse {target}") {
            return Err(format!(
                "the release {} holds a program that says {says:?}: nothing was changed",
                release.tag
            ));
        }
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
        let read = |file: &str| {
            std::fs::read(dir.join(file)).map_err(|error| format!("{file}: {error}"))
        };
        let (sums, sig) = (read("SHA256SUMS")?, read("SHA256SUMS.sig")?);
        for (other, version, arch) in &pending {
            if other.second && !answers("127.0.0.1", port).await {
                return Err(format!(
                    "this machine's site does not answer, so {} was not restarted: it carries the site meanwhile",
                    other.name
                ));
            }
            say(&format!("Updating {} ({version} to {target})...", other.name));
            let name = program(arch);
            let answer = crate::setup::node::ask(
                &other.name,
                &format!("update {name}"),
                &bundle(&sums, &sig, &read(&name)?),
            )?;
            if answer != format!("now runs {target}") {
                return Err(format!(
                    "{} answered {answer:?}, not that it runs {target}",
                    other.name
                ));
            }
            say(&format!("  {}: {answer}", other.name));
            if other.second {
                healthy(&other.name, port).await?;
                say(&format!("  {} answers again.", other.name));
            }
        }
        // The second carries the site while this machine restarts, whether
        // or not this run updated it.
        if let Some(second) = second {
            healthy(&second.name, port).await.map_err(|error| {
                format!("{error}: this machine was not restarted, as {} carries the site meanwhile", second.name)
            })?;
        }
        if mine {
            say(&format!("Updating this machine ({VERSION} to {target})..."));
            let bytes = std::fs::read(&here).map_err(|error| format!("{}: {error}", here.display()))?;
            install(
                Path::new(crate::setup::BINARY),
                &bytes,
                &sha256_hex(&bytes),
                VERSION,
            )?;
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
        let wrong = install(&binary, new, &sha256_hex(b"other"), "9.9.8").unwrap_err();
        assert!(wrong.contains("does not match"), "{wrong}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        let broken = b"#!/bin/sh\nexit 1\n";
        let error = install(&binary, broken, &sha256_hex(broken), "9.9.8").unwrap_err();
        assert!(error.contains("does not run"), "{error}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        assert!(!dir.path().join(".scannerpulse.update").exists());
        assert_eq!(
            install(&binary, new, &sha256_hex(new), "9.9.8").unwrap(),
            "9.9.9"
        );
        assert_eq!(std::fs::read(&binary).unwrap(), new);
    }

    #[test]
    fn an_older_program_is_not_put_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("scannerpulse");
        std::fs::write(&binary, "old").unwrap();
        let new = b"#!/bin/sh\necho 'scannerpulse 9.9.9'\n";
        let error = install(&binary, new, &sha256_hex(new), "9.9.10").unwrap_err();
        assert!(error.contains("older than 9.9.10"), "{error}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        // The same version again is allowed: a machine put right by hand.
        assert_eq!(
            install(&binary, new, &sha256_hex(new), "9.9.9").unwrap(),
            "9.9.9"
        );
    }

    #[test]
    fn a_bundle_is_read_back_whole_and_a_cut_or_oversized_one_is_refused() {
        let whole = bundle(b"sums", b"signature", b"program");
        assert_eq!(
            unbundle(&mut whole.as_slice()).unwrap(),
            Sent {
                sums: b"sums".to_vec(),
                sig: b"signature".to_vec(),
                program: b"program".to_vec()
            }
        );
        let cut = &whole[..whole.len() - 12];
        assert!(unbundle(&mut &cut[..]).is_err());
        let huge = format!("{}\n", 1024 * 1024).into_bytes();
        let error = unbundle(&mut huge.as_slice()).unwrap_err();
        assert!(error.contains("too long"), "{error}");
        assert!(unbundle(&mut &b"x\n"[..]).is_err());
    }

    #[test]
    fn a_program_is_put_in_place_only_with_the_release_signature() {
        let read = |file: &str| std::fs::read(fixtures().join(file)).unwrap();
        let (sums, sig) = (read("SHA256SUMS"), read("SHA256SUMS.sig"));
        let name = "scannerpulse-x86_64-linux";
        let program = read(name);
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("scannerpulse");
        let work = dir.path().join("work");
        let receive_with = |key: &[u8], program: &[u8], name: &str| {
            std::fs::write(&binary, "old").unwrap();
            let sent = bundle(&sums, &sig, program);
            receive(&binary, name, &mut sent.as_slice(), key, &work, "9.9.8")
        };
        // Signed by another key: the far side's own key decides.
        let error = receive_with(&release_key().unwrap(), &program, name).unwrap_err();
        assert!(error.contains("does not check out"), "{error}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        // A program the checksums do not name, or not this one.
        let other = b"#!/bin/sh\necho 'scannerpulse 9.9.9'\n# changed\n";
        let error = receive_with(&fixture_key(), other, name).unwrap_err();
        assert!(error.contains("does not match"), "{error}");
        let error =
            receive_with(&fixture_key(), &program, "scannerpulse-riscv64-linux").unwrap_err();
        assert!(error.contains("name no"), "{error}");
        assert_eq!(std::fs::read_to_string(&binary).unwrap(), "old");
        assert!(!work.exists());
        // Signed, and named: in place.
        assert_eq!(
            receive_with(&fixture_key(), &program, name).unwrap(),
            "9.9.9"
        );
        assert_eq!(std::fs::read(&binary).unwrap(), program);
        assert!(!work.exists());
    }

    #[test]
    fn machines_behind_are_updated_and_those_ahead_are_named() {
        let at = |name: &str, version: &str| {
            (
                Other {
                    name: name.into(),
                    second: false,
                },
                version.to_string(),
                "x86_64".to_string(),
            )
        };
        let found = vec![at("db1", "0.8.0"), at("db2", "0.9.0"), at("ref", "0.9.1")];
        let (pending, ahead) = sort_out(&found, "0.9.0");
        let names: Vec<&str> = pending
            .iter()
            .map(|(other, _, _)| other.name.as_str())
            .collect();
        assert_eq!(names, vec!["db1"]);
        assert_eq!(ahead, vec!["ref (0.9.1)".to_string()]);
    }

    #[test]
    fn both_scannerpulse_machines_answer_before_either_restarts() {
        assert!(both_answer(true, None).is_ok());
        assert!(both_answer(true, Some(("web2", true))).is_ok());
        let second = both_answer(true, Some(("web2", false))).unwrap_err();
        assert!(second.contains("web2"), "{second}");
        let here = both_answer(false, Some(("web2", true))).unwrap_err();
        assert!(here.contains("this machine"), "{here}");
        assert!(both_answer(false, None).is_err());
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
        // A second still before 0.8.0 joins with its old program, which
        // cannot let the key in: the install line comes first.
        assert!(shut.contains("install line"), "{shut}");
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
