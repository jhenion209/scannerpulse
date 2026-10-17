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
    let answers = Answers {
        recordings: PathBuf::from(need("RECORDINGS")?),
        archive: PathBuf::from(need("ARCHIVE")?),
        time_zone: get("TIME_ZONE").unwrap_or_else(|| defaults.time_zone.clone()),
        admin: need("ADMIN")?,
        password: need("ADMIN_PASSWORD")?,
        transcription,
        port: match get("PORT") {
            Some(port) => port
                .parse()
                .map_err(|_| format!("PORT is {port:?}: a number"))?,
            None => defaults.port,
        },
        host: get("HOST").unwrap_or_else(|| defaults.host.clone()),
        names: get("NAMES")
            .map(|n| n.split_whitespace().map(String::from).collect())
            .unwrap_or_else(|| defaults.names.clone()),
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

fn folder(
    question: &str,
    other: Option<&folders::Folder>,
) -> Result<folders::Folder, inquire::InquireError> {
    let other = other.cloned();
    let answer = Text::new(question)
        .with_validator(move |input: &str| {
            Ok(match folders::check(Path::new(input.trim())) {
                Err(why) => Validation::Invalid(why.into()),
                Ok(found) => match other.as_ref().map(|o| folders::apart(o, &found)) {
                    Some(Err(why)) => Validation::Invalid(why.into()),
                    _ => Validation::Valid,
                },
            })
        })
        .prompt()?;
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

/// Asks the Simple questions; None when the person leaves.
pub fn interactive(defaults: &Defaults) -> Result<Option<Answers>, String> {
    let left = |error: inquire::InquireError| match error {
        inquire::InquireError::OperationCanceled | inquire::InquireError::OperationInterrupted => {
            None
        }
        other => Some(other.to_string()),
    };
    let result = (|| -> Result<Answers, inquire::InquireError> {
        say(
            "1 of 5. The recordings folder: where SDRTrunk saves its recordings. ScannerPulse reads each new file there and then moves it into the archive, so this folder stays small. It can be a folder on this machine or a network share SDRTrunk writes to; the folder must exist already.",
        );
        let recordings = folder("Recordings folder:", None)?;
        say(
            "2 of 5. The archive: where every call is kept for good, filed as <archive>/YYYY-MM-DD/<talkgroup>/<file>.mp3. Nothing here is removed automatically, and the database's backups are kept in it too, in .scannerpulse-backups. A busy county system adds about half a gigabyte a day. Give it a disk of its own if you can; it may not be inside the recordings folder, nor the recordings folder inside it.",
        );
        let archive = folder("Archive folder:", Some(&recordings))?;
        say(
            "3 of 5. The site's time zone: the times on the pages are shown in it, and SDRTrunk's file names are read in it.",
        );
        let time_zone = Text::new("Time zone:")
            .with_default(&defaults.time_zone)
            .with_validator(|z: &str| {
                Ok(match check_zone(z.trim()) {
                    Ok(()) => Validation::Valid,
                    Err(why) => Validation::Invalid(why.into()),
                })
            })
            .prompt()?
            .trim()
            .to_string();
        say(
            "4 of 5. The first admin account signs in to the admin page. Its password has at least 12 characters; it is asked twice and never shown.",
        );
        let admin = Text::new("Admin name:")
            .with_default("admin")
            .with_validator(|n: &str| {
                Ok(match crate::accounts::check_login(n.trim()) {
                    Ok(()) => Validation::Valid,
                    Err(why) => Validation::Invalid(why.to_string().into()),
                })
            })
            .prompt()?
            .trim()
            .to_string();
        let password = Password::new("Password:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .with_custom_confirmation_message("The password again:")
            .with_validator(|p: &str| {
                Ok(match crate::accounts::check_password(p) {
                    Ok(()) => Validation::Valid,
                    Err(why) => Validation::Invalid(why.to_string().into()),
                })
            })
            .prompt()?;
        say(
            "5 of 5. Transcription: ScannerPulse can turn calls into text with a speech-to-text server that speaks the OpenAI audio API, such as speaches. Give its full address and model now, or later in the admin page (Settings).",
        );
        let now = Select::new("Transcription:", vec!["Later", "Now"]).prompt()? == "Now";
        let transcription = if now {
            let url = Text::new("Server's address:")
                .with_validator(|u: &str| {
                    Ok(match check_server(u.trim()) {
                        Ok(()) => Validation::Valid,
                        Err(why) => Validation::Invalid(why.into()),
                    })
                })
                .prompt()?
                .trim()
                .to_string();
            let model = Text::new("Model:")
                .with_validator(|m: &str| {
                    Ok(if m.trim().is_empty() {
                        Validation::Invalid("a model is needed".into())
                    } else {
                        Validation::Valid
                    })
                })
                .prompt()?
                .trim()
                .to_string();
            Some(Transcription { url, model })
        } else {
            None
        };
        Ok(Answers {
            recordings: recordings.path,
            archive: archive.path,
            time_zone,
            admin,
            password,
            transcription,
            port: defaults.port,
            host: defaults.host.clone(),
            names: defaults.names.clone(),
        })
    })();
    match result {
        Ok(answers) => Ok(Some(answers)),
        Err(error) => match left(error) {
            None => Ok(None),
            Some(why) => Err(why),
        },
    }
}

/// Asks to go ahead after the summary.
pub fn confirm() -> Result<bool, String> {
    Confirm::new("Set up this machine now?")
        .with_default(false)
        .with_help_message("Nothing has been written yet. No leaves setup with nothing changed.")
        .prompt()
        .map_err(|error| error.to_string())
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

/// `from` or the next port nothing listens on. Without root a low port
/// cannot be tried (a dry run), so it counts as free.
fn free_port(from: u16) -> u16 {
    (from..=u16::MAX)
        .find(
            |port| match std::net::TcpListener::bind(("0.0.0.0", *port)) {
                Ok(_) => true,
                Err(error) => error.kind() == std::io::ErrorKind::PermissionDenied,
            },
        )
        .unwrap_or(from)
}

#[cfg(test)]
mod tests {
    use super::{Defaults, free_port, from_file, zone_from_link};
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
        let text = format!(
            "RECORDINGS={}\nARCHIVE={}\nADMIN=chief\nADMIN_PASSWORD=\"long enough secret\"\nPORT=8443\n",
            rec.display(),
            arc.display()
        );
        let answers = from_file(&answers_file(dir.path(), &text, 0o600), &defaults()).unwrap();
        assert_eq!(answers.admin, "chief");
        assert_eq!(answers.password, "long enough secret");
        assert_eq!(answers.port, 8443);
        assert_eq!(answers.time_zone, "America/Chicago");
        assert_eq!(answers.names, ["radio-a", "192.0.2.10"]);
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
    fn a_port_in_use_gives_the_next_free_one() {
        let taken = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        assert!(free_port(port) > port);
    }
}
