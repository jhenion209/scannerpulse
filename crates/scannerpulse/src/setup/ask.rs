//! The Simple questions: asked one at a time, each explained first, or read
//! from an answers file for an unattended install. Every answer is checked
//! before it is taken.

use std::path::{Path, PathBuf};

use inquire::validator::Validation;
use inquire::{Confirm, Password, PasswordDisplayMode, Select, Text};

use super::folders::{self, size};
use super::steps::{Answers, Transcription};

/// What setup proposes before asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Defaults {
    pub host: String,
    pub names: Vec<String>,
    pub time_zone: String,
    pub port: u16,
}

/// Every answer checked as the questions check it, the folders made
/// canonical as the questions make them.
fn checked(mut answers: Answers) -> Result<Answers, String> {
    let recordings = folders::check(&answers.recordings)?;
    let archive = folders::check(&answers.archive)?;
    folders::apart(&recordings, &archive)?;
    answers.recordings = recordings.path;
    answers.archive = archive.path;
    check_zone(&answers.time_zone)?;
    crate::accounts::check_login(&answers.admin).map_err(|e| e.to_string())?;
    crate::accounts::check_password(&answers.password).map_err(|e| e.to_string())?;
    if let Some(t) = &answers.transcription {
        check_server(&t.url)?;
        if t.model.trim().is_empty() {
            return Err("a transcription model is needed".into());
        }
    }
    if answers.port == 0 {
        return Err("the port is 1 to 65535".into());
    }
    if answers.names.is_empty() {
        return Err("the certificate needs this machine's name or address".into());
    }
    Ok(answers)
}

fn check_zone(zone: &str) -> Result<(), String> {
    zone.parse::<chrono_tz::Tz>().map(|_| ()).map_err(|_| {
        format!("{zone:?} is not a time zone; write one such as America/Chicago or Europe/London")
    })
}

fn check_server(url: &str) -> Result<(), String> {
    if crate::settings::server_url_ok(url) {
        Ok(())
    } else {
        Err("the server's address is a full http:// or https:// URL, such as http://192.0.2.20:8000/v1/audio/transcriptions, with no user name or password".into())
    }
}

/// The answers in `path`: `NAME=value` lines, as the settings file is
/// written. ADMIN_PASSWORD is in the file, so the file must be its owner's
/// alone.
pub fn from_file(path: &Path, defaults: &Defaults) -> Result<Answers, String> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(format!(
            "{} holds a password: make it its owner's alone (chmod 600)",
            path.display()
        ));
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let pairs =
        crate::hostfile::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    let get = |name: &str| {
        pairs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    };
    let need =
        |name: &str| get(name).ok_or_else(|| format!("{}: {name} is missing", path.display()));
    let transcription = match (get("TRANSCRIPTION_URL"), get("TRANSCRIPTION_MODEL")) {
        (Some(url), Some(model)) => Some(Transcription { url, model }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "{}: give both TRANSCRIPTION_URL and TRANSCRIPTION_MODEL, or neither",
                path.display()
            ));
        }
    };
    let port = match get("PORT") {
        Some(port) => {
            let port: u16 = port
                .parse()
                .map_err(|_| format!("PORT is {port:?}: a number"))?;
            if !free(port) {
                return Err(format!(
                    "PORT {port} is in use by {}: free it, or leave PORT out and setup takes the next free port",
                    holder(port)
                ));
            }
            port
        }
        None => defaults.port,
    };
    let mut names: Vec<String> = get("NAMES")
        .map(|n| n.split_whitespace().map(String::from).collect())
        .unwrap_or_else(|| defaults.names.clone());
    // Setup's own check at the end reaches the site at this address.
    if !names.iter().any(|n| n == "127.0.0.1") {
        names.push("127.0.0.1".into());
    }
    let answers = Answers {
        recordings: PathBuf::from(need("RECORDINGS")?),
        archive: PathBuf::from(need("ARCHIVE")?),
        time_zone: get("TIME_ZONE").unwrap_or_else(|| defaults.time_zone.clone()),
        admin: need("ADMIN")?,
        password: need("ADMIN_PASSWORD")?,
        transcription,
        port,
        host: get("HOST").unwrap_or_else(|| defaults.host.clone()),
        names,
    };
    checked(answers)
}

fn say(text: &str) {
    println!();
    for line in super::screen::wrap(text, 76) {
        println!("  {line}");
    }
    println!();
}

/// A validator from a check that gives the reason it refuses.
fn valid(
    check: impl Fn(&str) -> Result<(), String> + Clone + 'static,
) -> impl Fn(&str) -> Result<Validation, inquire::CustomUserError> + Clone + 'static {
    move |input: &str| {
        Ok(match check(input.trim()) {
            Ok(()) => Validation::Valid,
            Err(why) => Validation::Invalid(why.into()),
        })
    }
}

fn folder(
    question: &str,
    previous: Option<&Path>,
    other: Option<&folders::Folder>,
) -> Result<folders::Folder, inquire::InquireError> {
    let other = other.cloned();
    let previous = previous.map(|p| p.display().to_string());
    let mut text = Text::new(question).with_validator(valid(move |input| {
        let found = folders::check(Path::new(input))?;
        other.as_ref().map_or(Ok(()), |o| folders::apart(o, &found))
    }));
    if let Some(previous) = &previous {
        text = text.with_default(previous);
    }
    let answer = text.prompt()?;
    let found = folders::check(Path::new(answer.trim()))
        .map_err(|why| inquire::InquireError::Custom(why.into()))?;
    println!(
        "    {} free{}",
        size(found.free_bytes),
        if found.on_system_disk {
            ". This folder is on the system disk: if it fills, the system can stop working; a disk of its own is safer"
        } else {
            ""
        }
    );
    Ok(found)
}

/// The program `ss` shows listening on `port`, from its `users:(("nginx",...` part.
pub fn program_in(ss: &str) -> Option<String> {
    let (_, rest) = ss.split_once("((\"")?;
    Some(rest.split('"').next()?.to_string())
}

/// What listens on `port`, as far as `ss` can tell.
pub fn holder(port: u16) -> String {
    std::process::Command::new("ss")
        .args(["-H", "-ltnp", &format!("sport = :{port}")])
        .output()
        .ok()
        .and_then(|out| program_in(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or_else(|| "another program".into())
}

/// Asks the Simple questions, each answer of `previous` offered again;
/// None when the person leaves.
pub fn interactive(
    defaults: &Defaults,
    previous: Option<&Answers>,
) -> Result<Option<Answers>, String> {
    let result = (|| -> Result<Option<Answers>, inquire::InquireError> {
        let port = match previous {
            Some(answers) => answers.port,
            None if defaults.port != 443 => {
                let take = Confirm::new(&format!(
                    "Port 443 is in use by {}. Serve ScannerPulse on port {} instead?",
                    holder(443),
                    defaults.port
                ))
                .with_default(true)
                .with_help_message("No leaves setup, so you can free port 443 and run it again.")
                .prompt()?;
                if !take {
                    return Ok(None);
                }
                defaults.port
            }
            None => defaults.port,
        };
        say(
            "1 of 5. The recordings folder: where SDRTrunk saves its recordings. ScannerPulse reads each new file there and then moves it into the archive, so this folder stays small. It can be a folder on this machine or a network share SDRTrunk writes to; the folder must exist already.",
        );
        let recordings = folder(
            "Recordings folder:",
            previous.map(|a| a.recordings.as_path()),
            None,
        )?;
        say(
            "2 of 5. The archive: where every call is kept for good, filed as <archive>/YYYY-MM-DD/<talkgroup>/<file>.mp3. Nothing here is removed automatically, and the database's backups are kept in it too, in .scannerpulse-backups. A busy county system adds about half a gigabyte a day. Give it a disk of its own if you can; it may not be inside the recordings folder, nor the recordings folder inside it.",
        );
        let archive = folder(
            "Archive folder:",
            previous.map(|a| a.archive.as_path()),
            Some(&recordings),
        )?;
        say(
            "3 of 5. The site's time zone: the times on the pages are shown in it, and SDRTrunk's file names are read in it.",
        );
        let time_zone = Text::new("Time zone:")
            .with_default(previous.map_or(&defaults.time_zone, |a| &a.time_zone))
            .with_validator(valid(check_zone))
            .prompt()?
            .trim()
            .to_string();
        say(
            "4 of 5. The first admin account signs in to the admin page. Its password has at least 12 characters; it is asked twice and never shown.",
        );
        let admin = Text::new("Admin name:")
            .with_default(previous.map_or("admin", |a| &a.admin))
            .with_validator(valid(|n| {
                crate::accounts::check_login(n).map_err(|e| e.to_string())
            }))
            .prompt()?
            .trim()
            .to_string();
        let keep = match previous {
            Some(_) => Confirm::new("Keep the password you typed?")
                .with_default(true)
                .prompt()?,
            None => false,
        };
        let password = match previous {
            Some(answers) if keep => answers.password.clone(),
            _ => Password::new("Password:")
                .with_display_mode(PasswordDisplayMode::Masked)
                .with_custom_confirmation_message("The password again:")
                .with_validator(move |p: &str| {
                    Ok(match crate::accounts::check_password(p) {
                        Ok(()) => Validation::Valid,
                        Err(why) => Validation::Invalid(why.to_string().into()),
                    })
                })
                .prompt()?,
        };
        say(
            "5 of 5. Transcription: ScannerPulse can turn calls into text with a speech-to-text server that speaks the OpenAI audio API, such as speaches. Give its full address and model now, or later in the admin page (Settings).",
        );
        let had = previous.and_then(|a| a.transcription.as_ref());
        let choices = if had.is_some() {
            vec!["Now", "Later"]
        } else {
            vec!["Later", "Now"]
        };
        let transcription = if Select::new("Transcription:", choices).prompt()? == "Now" {
            let mut url = Text::new("Server's address:").with_validator(valid(check_server));
            if let Some(t) = had {
                url = url.with_default(&t.url);
            }
            let url = url.prompt()?.trim().to_string();
            let mut model = Text::new("Model:").with_validator(valid(|m| {
                if m.is_empty() {
                    Err("a model is needed".into())
                } else {
                    Ok(())
                }
            }));
            if let Some(t) = had {
                model = model.with_default(&t.model);
            }
            let model = model.prompt()?.trim().to_string();
            Some(Transcription { url, model })
        } else {
            None
        };
        Ok(Some(Answers {
            recordings: recordings.path,
            archive: archive.path,
            time_zone,
            admin,
            password,
            transcription,
            port,
            host: defaults.host.clone(),
            names: defaults.names.clone(),
        }))
    })();
    match result {
        Ok(answers) => Ok(answers),
        Err(
            inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted,
        ) => Ok(None),
        Err(other) => Err(other.to_string()),
    }
}

/// What the person does after the summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Go,
    Change,
    Quit,
}

/// Asks to go ahead, change an answer, or quit, after the summary.
pub fn next() -> Result<Next, String> {
    const GO: &str = "Set up this machine now";
    const CHANGE: &str = "Change an answer";
    const QUIT: &str = "Quit; nothing is written";
    let choice = Select::new("Nothing has been written yet.", vec![GO, CHANGE, QUIT])
        .prompt()
        .map_err(|error| error.to_string());
    Ok(match choice {
        Ok(GO) => Next::Go,
        Ok(CHANGE) => Next::Change,
        Ok(_) => Next::Quit,
        Err(_) => Next::Quit,
    })
}

/// What this machine suggests: its name, its outward address, its clock's
/// time zone, and port 443 or the next free one.
pub fn defaults() -> Defaults {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|h| h.trim().to_string())
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "scannerpulse".into());
    let mut names = vec![host.clone()];
    names.extend(outward_address());
    // Setup's own check at the end reaches the site at this address.
    names.push("127.0.0.1".into());
    let time_zone = std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|link| zone_from_link(&link))
        .unwrap_or_else(|| "UTC".into());
    Defaults {
        host,
        names,
        time_zone,
        port: free_port(443),
    }
}

/// The address this machine reaches the network from; the route is only
/// looked up, nothing is sent (192.0.2.1 is a documentation address).
fn outward_address() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:9").ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}

/// `/usr/share/zoneinfo/America/Chicago` gives America/Chicago.
fn zone_from_link(link: &Path) -> Option<String> {
    let (_, zone) = link.to_str()?.split_once("zoneinfo/")?;
    check_zone(zone).ok().map(|()| zone.to_string())
}

/// `from` or the next port nothing listens on.
fn free_port(from: u16) -> u16 {
    (from..=u16::MAX).find(|port| free(*port)).unwrap_or(from)
}

/// Nothing listens on `port`. Without root a low port cannot be tried (a
/// dry run), so it counts as free.
fn free(port: u16) -> bool {
    match std::net::TcpListener::bind(("0.0.0.0", port)) {
        Ok(_) => true,
        Err(error) => error.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

#[cfg(test)]
mod tests {
    use super::{Defaults, free_port, from_file, program_in, zone_from_link};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn defaults() -> Defaults {
        Defaults {
            host: "radio-a".into(),
            names: vec!["radio-a".into(), "192.0.2.10".into()],
            time_zone: "America/Chicago".into(),
            port: 443,
        }
    }

    fn answers_file(dir: &Path, text: &str, mode: u32) -> std::path::PathBuf {
        let path = dir.join("answers");
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        path
    }

    #[test]
    fn an_answers_file_gives_the_answers_and_the_defaults_fill_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (rec, arc) = (dir.path().join("rec"), dir.path().join("arc"));
        std::fs::create_dir(&rec).unwrap();
        std::fs::create_dir(&arc).unwrap();
        // Nothing listens on port 1, and the other tests' ports are far above it.
        let text = format!(
            "RECORDINGS={}\nARCHIVE={}\nADMIN=chief\nADMIN_PASSWORD=\"long enough secret\"\nPORT=1\n",
            rec.display(),
            arc.display()
        );
        let answers = from_file(&answers_file(dir.path(), &text, 0o600), &defaults()).unwrap();
        assert_eq!(answers.admin, "chief");
        assert_eq!(answers.password, "long enough secret");
        assert_eq!(answers.port, 1);
        assert_eq!(answers.time_zone, "America/Chicago");
        assert_eq!(answers.names, ["radio-a", "192.0.2.10", "127.0.0.1"]);
        assert_eq!(answers.transcription, None);
        assert_eq!(answers.recordings, rec.canonicalize().unwrap());
    }

    #[test]
    fn an_answers_file_others_can_read_or_half_a_transcription_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let text =
            "RECORDINGS=/r\nARCHIVE=/a\nADMIN=chief\nADMIN_PASSWORD=\"long enough secret\"\n";
        let error = from_file(&answers_file(dir.path(), text, 0o644), &defaults()).unwrap_err();
        assert!(error.contains("chmod 600"), "{error}");
        let half =
            format!("{text}TRANSCRIPTION_URL=http://192.0.2.20:8000/v1/audio/transcriptions\n");
        let error = from_file(&answers_file(dir.path(), &half, 0o600), &defaults()).unwrap_err();
        assert!(error.contains("or neither"), "{error}");
    }

    #[test]
    fn an_answers_files_port_must_be_free_and_its_names_always_include_127_0_0_1() {
        let dir = tempfile::tempdir().unwrap();
        let (rec, arc) = (dir.path().join("rec"), dir.path().join("arc"));
        std::fs::create_dir(&rec).unwrap();
        std::fs::create_dir(&arc).unwrap();
        let base = format!(
            "RECORDINGS={}\nARCHIVE={}\nADMIN=chief\nADMIN_PASSWORD=\"long enough secret\"\n",
            rec.display(),
            arc.display()
        );
        let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let text = format!("{base}PORT={port}\n");
        let error = from_file(&answers_file(dir.path(), &text, 0o600), &defaults()).unwrap_err();
        assert!(
            error.starts_with(&format!("PORT {port} is in use by "))
                && error
                    .ends_with(": free it, or leave PORT out and setup takes the next free port"),
            "{error}"
        );
        let text = format!("{base}NAMES=\"radio.example 192.0.2.10\"\n");
        let answers = from_file(&answers_file(dir.path(), &text, 0o600), &defaults()).unwrap();
        assert_eq!(answers.names, ["radio.example", "192.0.2.10", "127.0.0.1"]);
        let text = format!("{base}NAMES=\"127.0.0.1 radio.example\"\n");
        let answers = from_file(&answers_file(dir.path(), &text, 0o600), &defaults()).unwrap();
        assert_eq!(answers.names, ["127.0.0.1", "radio.example"]);
    }

    #[test]
    fn the_time_zone_comes_from_the_clock_link() {
        assert_eq!(
            zone_from_link(Path::new("/usr/share/zoneinfo/America/Chicago")).as_deref(),
            Some("America/Chicago")
        );
        assert_eq!(
            zone_from_link(Path::new("../usr/share/zoneinfo/Etc/UTC")).as_deref(),
            Some("Etc/UTC")
        );
        assert_eq!(zone_from_link(Path::new("/etc/nowhere")), None);
    }

    #[test]
    fn what_holds_a_port_is_read_from_ss() {
        let line = "LISTEN 0 511 0.0.0.0:443 0.0.0.0:* users:((\"nginx\",pid=812,fd=6),(\"nginx\",pid=811,fd=6))";
        assert_eq!(program_in(line).as_deref(), Some("nginx"));
        assert_eq!(program_in(""), None);
    }

    #[test]
    fn a_port_in_use_gives_the_next_free_one() {
        let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        assert!(free_port(port) > port);
    }
}
