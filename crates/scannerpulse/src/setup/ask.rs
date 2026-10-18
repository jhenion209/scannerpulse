//! Setup's questions, Simple's and Advanced's: asked one at a time, each
//! explained first, or read from an answers file for an unattended install.
//! Every answer is checked before it is taken.

use std::path::{Path, PathBuf};

use inquire::validator::Validation;
use inquire::{Confirm, Password, PasswordDisplayMode, Select, Text};

use super::folders::{self, size};
use super::steps::{Answers, Certificate, Database, Remote, Transcription};

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
    if let Database::Theirs(remote) = &answers.database {
        check_remote_form(remote)?;
    }
    if let Certificate::Installed { cert, key } = &answers.certificate {
        crate::cert::check(cert, key)?;
    }
    for proxy in &answers.proxies {
        check_proxy(proxy)?;
    }
    if let Some(server) = &answers.syslog {
        check_syslog(server)?;
    }
    if answers.database != Database::Here
        && (answers.backups.is_some() || !answers.pushes.is_empty())
    {
        return Err(
            "a database you run keeps its own backups: leave the backup folder and push addresses out"
                .into(),
        );
    }
    if let Some(backups) = &answers.backups {
        answers.backups = Some(check_backups(backups, &answers.recordings, &answers.archive)?.path);
    }
    for (kind, url) in &answers.pushes {
        if crate::backup::push_var(kind).is_none() {
            return Err(format!("{kind:?} is not a kind of backup"));
        }
        check_push(url)?;
    }
    Ok(answers)
}

fn check_remote_form(remote: &Remote) -> Result<(), String> {
    if remote.host.trim().is_empty() || remote.host.contains(char::is_whitespace) {
        return Err("the database's host is a name or an address".into());
    }
    if remote.port == 0 {
        return Err("the database's port is 1 to 65535".into());
    }
    if remote.name.trim().is_empty() || remote.role.trim().is_empty() {
        return Err("the database and its role each need a name".into());
    }
    if remote.password.is_empty() {
        return Err("the role's password is needed".into());
    }
    Ok(())
}

fn check_proxy(proxy: &str) -> Result<(), String> {
    proxy
        .parse::<std::net::IpAddr>()
        .map(drop)
        .map_err(|_| format!("{proxy:?} is not an address such as 192.0.2.10"))
}

fn check_syslog(server: &str) -> Result<(), String> {
    crate::log::target(server)
        .map(drop)
        .ok_or_else(|| format!("{server:?} is not udp://host:port or tcp://host:port"))
}

/// A separate backup folder: a folder as the others are checked, inside
/// neither the recordings folder nor the archive (whose own backup folder is
/// the default), nor holding them.
pub(super) fn check_backups(
    path: &Path,
    recordings: &Path,
    archive: &Path,
) -> Result<folders::Folder, String> {
    let found = folders::check(path)?;
    backups_apart(
        &found.path,
        &[
            (recordings, "the recordings folder"),
            (archive, "the archive"),
        ],
    )?;
    Ok(found)
}

/// A backups `folder` may be inside none of `others` (each with what it
/// is), nor hold one.
pub(super) fn backups_apart(folder: &Path, others: &[(&Path, &str)]) -> Result<(), String> {
    for (other, what) in others {
        if folder.starts_with(other) || other.starts_with(folder) {
            return Err(format!(
                "{} and {what} ({}) may not be inside each other; for backups inside the archive, keep the default",
                folder.display(),
                other.display()
            ));
        }
    }
    Ok(())
}

/// A push (heartbeat) address: http or https, such as an Uptime Kuma push
/// monitor's, whose token may be in its path or its query.
pub(super) fn check_push(url: &str) -> Result<(), String> {
    let ok = reqwest::Url::parse(url).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    });
    if ok {
        Ok(())
    } else {
        // The address carries a token: it is not repeated.
        Err("the push address is not a full http:// or https:// address".into())
    }
}

/// The checks a database the person runs must pass, once connected with
/// `options`: PostgreSQL 17, and its role may make tables in the public
/// schema. Gives the server's version.
pub async fn check_database(options: sqlx::postgres::PgConnectOptions) -> Result<String, String> {
    use sqlx::ConnectOptions;
    let place = format!(
        "the database {} on {}:{} as {}",
        options.get_database().unwrap_or("?"),
        options.get_host(),
        options.get_port(),
        options.get_username()
    );
    let mut conn = tokio::time::timeout(std::time::Duration::from_secs(10), options.connect())
        .await
        .map_err(|_| format!("{place} did not answer in 10 s"))?
        .map_err(|error| format!("cannot sign in to {place}: {error}"))?;
    let version: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
        .fetch_one(&mut conn)
        .await
        .map_err(|error| format!("{place}: {error}"))?;
    if !(170_000..180_000).contains(&version) {
        return Err(format!(
            "{place} is PostgreSQL {}; ScannerPulse needs PostgreSQL 17",
            version / 10_000
        ));
    }
    let may: bool = sqlx::query_scalar("SELECT has_schema_privilege('public', 'CREATE')")
        .fetch_one(&mut conn)
        .await
        .map_err(|error| format!("{place}: {error}"))?;
    if !may {
        return Err(format!(
            "the role {role} may not make tables in {place}'s public schema: make it the database's owner, or GRANT CREATE ON SCHEMA public TO {role}",
            role = options.get_username()
        ));
    }
    Ok(format!(
        "PostgreSQL {}.{}",
        version / 10_000,
        version % 10_000
    ))
}

/// `check_database` for a database the person names.
pub async fn check_remote(remote: &Remote) -> Result<String, String> {
    check_database(
        sqlx::postgres::PgConnectOptions::new()
            .host(&remote.host)
            .port(remote.port)
            .database(&remote.name)
            .username(&remote.role)
            .password(&remote.password),
    )
    .await
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
    let database = match get("DATABASE").as_deref() {
        None | Some("here") => Database::Here,
        Some("theirs") => Database::Theirs(Remote {
            host: need("DB_HOST")?,
            port: match get("DB_PORT") {
                Some(port) => port
                    .parse()
                    .map_err(|_| format!("DB_PORT is {port:?}: a number"))?,
                None => 5432,
            },
            name: need("DB_NAME")?,
            role: need("DB_ROLE")?,
            password: need("DB_PASSWORD")?,
        }),
        Some(other) => return Err(format!("DATABASE is {other:?}: here or theirs")),
    };
    let certificate = match (get("CERT_FILE"), get("KEY_FILE")) {
        (Some(cert), Some(key)) => Certificate::Installed {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        },
        (None, None) => Certificate::Own,
        _ => {
            return Err(format!(
                "{}: give both CERT_FILE and KEY_FILE, or neither",
                path.display()
            ));
        }
    };
    let pushes = crate::backup::KINDS
        .iter()
        .filter_map(|kind| {
            Some((
                kind.to_string(),
                get(&format!("PUSH_{}", kind.to_ascii_uppercase()))?,
            ))
        })
        .collect();
    let advanced = pairs.iter().any(|(n, _)| {
        matches!(
            n.as_str(),
            "DATABASE"
                | "CERT_FILE"
                | "KEY_FILE"
                | "PROXIES"
                | "SYSLOG"
                | "BACKUPS"
                | "PUSH_PGBACKREST"
                | "PUSH_DUMP"
        )
    });
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
        advanced,
        database,
        certificate,
        proxies: get("PROXIES")
            .map(|p| p.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        syslog: get("SYSLOG"),
        backups: get("BACKUPS").map(PathBuf::from),
        pushes,
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

/// Asks the Simple questions, and with `advanced` Advanced's too, each
/// answer of `previous` offered again; None when the person leaves.
pub fn interactive(
    defaults: &Defaults,
    previous: Option<&Answers>,
    advanced: bool,
) -> Result<Option<Answers>, String> {
    let result = (|| -> Result<Option<Answers>, inquire::InquireError> {
        let mut database = Database::Here;
        let mut certificate = Certificate::Own;
        let mut proxies = Vec::new();
        let mut syslog = None;
        let port = match previous {
            _ if advanced => {
                say(
                    "Advanced 1 of 4. Where the database runs: PostgreSQL 17 on this machine, installed and backed up by setup; or a PostgreSQL 17 database you run elsewhere, whose failover and backups stay yours. ScannerPulse needs a database and a role that may make tables in it.",
                );
                database = ask_database(previous.map(|a| &a.database))?;
                say(
                    "Advanced 2 of 4. Web access. ScannerPulse serves HTTPS only. Its certificate is one setup makes for this machine (the browser warns once), or one you have (a PEM certificate and its key). Let's Encrypt comes in a later version; the admin page will offer it. If a reverse proxy sits in front of the site, give its addresses, so the visitors' own addresses are read from it.",
                );
                certificate = ask_certificate(previous.map(|a| &a.certificate))?;
                proxies = ask_proxies(previous.map(|a| a.proxies.as_slice()))?;
                say(
                    "Advanced 3 of 4. Logs go to the journal (journalctl -u scannerpulse-web). They can also go to a syslog server, as RFC 5424 over UDP or TCP.",
                );
                syslog = ask_syslog(previous.and_then(|a| a.syslog.as_deref()))?;
                ask_port(previous.map_or(defaults.port, |a| a.port))?
            }
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
        let (mut backups, mut pushes) = (None, Vec::new());
        if advanced && database == Database::Here {
            say(
                "Advanced 4 of 4. The backups: every night a pgBackRest backup (with the database's changes kept between them, for recovery to any moment) and a dump. They go in the archive's .scannerpulse-backups folder, or a folder you name; a disk other than the database's is safer. A monitor such as Uptime Kuma can be told of each backup that succeeds: give a push monitor's address for each kind, or none.",
            );
            backups = ask_backups(
                previous.and_then(|a| a.backups.as_deref()),
                &recordings.path,
                &archive.path,
            )?;
            pushes = ask_pushes(previous.map(|a| a.pushes.as_slice()))?;
        }
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
            advanced,
            database,
            certificate,
            proxies,
            syslog,
            backups,
            pushes,
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

/// A text answer checked by `check`, offering `previous`.
fn text(
    question: &str,
    previous: Option<&str>,
    check: impl Fn(&str) -> Result<(), String> + Clone + 'static,
) -> Result<String, inquire::InquireError> {
    let mut prompt = Text::new(question).with_validator(valid(check));
    if let Some(previous) = previous {
        prompt = prompt.with_default(previous);
    }
    Ok(prompt.prompt()?.trim().to_string())
}

fn ask_database(previous: Option<&Database>) -> Result<Database, inquire::InquireError> {
    const HERE: &str = "On this machine";
    const THEIRS: &str = "One I run";
    let had = match previous {
        Some(Database::Theirs(remote)) => Some(remote),
        _ => None,
    };
    let choices = if had.is_some() {
        vec![THEIRS, HERE]
    } else {
        vec![HERE, THEIRS]
    };
    if Select::new("Database:", choices).prompt()? == HERE {
        return Ok(Database::Here);
    }
    let host = text(
        "Its host (name or address):",
        had.map(|r| r.host.as_str()),
        |h| {
            if h.is_empty() || h.contains(char::is_whitespace) {
                Err("a name or an address".into())
            } else {
                Ok(())
            }
        },
    )?;
    let port = text(
        "Its port:",
        Some(&had.map_or(5432, |r| r.port).to_string()),
        |p| {
            p.parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .map(drop)
                .ok_or_else(|| "a port, 1 to 65535".into())
        },
    )?
    .parse()
    .expect("checked");
    let named = |what: &'static str| {
        move |n: &str| {
            if n.is_empty() {
                Err(format!("{what} needs a name"))
            } else {
                Ok(())
            }
        }
    };
    let name = text(
        "The database's name:",
        Some(had.map_or("scannerpulse", |r| r.name.as_str())),
        named("the database"),
    )?;
    let role = text(
        "The role ScannerPulse signs in as:",
        Some(had.map_or("scannerpulse", |r| r.role.as_str())),
        named("the role"),
    )?;
    let password = match had {
        Some(r)
            if Confirm::new("Keep the role's password you typed?")
                .with_default(true)
                .prompt()? =>
        {
            r.password.clone()
        }
        _ => Password::new("The role's password:")
            .with_display_mode(PasswordDisplayMode::Masked)
            .without_confirmation()
            .prompt()?,
    };
    Ok(Database::Theirs(Remote {
        host,
        port,
        name,
        role,
        password,
    }))
}

fn ask_certificate(previous: Option<&Certificate>) -> Result<Certificate, inquire::InquireError> {
    const OWN: &str = "Made by setup";
    const MINE: &str = "One I have";
    let had = match previous {
        Some(Certificate::Installed { cert, key }) => {
            Some((cert.display().to_string(), key.display().to_string()))
        }
        _ => None,
    };
    let choices = if had.is_some() {
        vec![MINE, OWN]
    } else {
        vec![OWN, MINE]
    };
    if Select::new("Certificate:", choices).prompt()? == OWN {
        return Ok(Certificate::Own);
    }
    let exists = |f: &str| {
        if Path::new(f).is_file() {
            Ok(())
        } else {
            Err(format!("{f} is not a file"))
        }
    };
    let cert = text(
        "The certificate (PEM, with its chain):",
        had.as_ref().map(|h| h.0.as_str()),
        exists,
    )?;
    let cert_for_key = cert.clone();
    let key = text(
        "Its private key (PEM):",
        had.as_ref().map(|h| h.1.as_str()),
        move |k| crate::cert::check(Path::new(&cert_for_key), Path::new(k)).map(drop),
    )?;
    let found = crate::cert::check(Path::new(&cert), Path::new(&key))
        .map_err(|why| inquire::InquireError::Custom(why.into()))?;
    println!(
        "    for {}, good until {}",
        found.names.join(", "),
        found.not_after.format("%Y-%m-%d")
    );
    Ok(Certificate::Installed {
        cert: PathBuf::from(cert),
        key: PathBuf::from(key),
    })
}

fn ask_proxies(previous: Option<&[String]>) -> Result<Vec<String>, inquire::InquireError> {
    let had = previous.map(|p| p.join(" "));
    let answer = text(
        "Reverse proxies' addresses (spaces between; none: leave empty):",
        had.as_deref(),
        |list| list.split_whitespace().try_for_each(check_proxy),
    )?;
    Ok(answer.split_whitespace().map(String::from).collect())
}

fn ask_syslog(previous: Option<&str>) -> Result<Option<String>, inquire::InquireError> {
    let answer = text(
        "Syslog server (udp://host:port or tcp://host:port; none: leave empty):",
        previous,
        |s| {
            if s.is_empty() {
                Ok(())
            } else {
                check_syslog(s)
            }
        },
    )?;
    Ok(Some(answer).filter(|a| !a.is_empty()))
}

fn ask_port(previous: u16) -> Result<u16, inquire::InquireError> {
    let answer = text("HTTPS port:", Some(&previous.to_string()), move |p| {
        let port: u16 = p
            .parse()
            .ok()
            .filter(|p| *p > 0)
            .ok_or("a port, 1 to 65535")?;
        if port == previous || free(port) {
            Ok(())
        } else {
            Err(format!("port {port} is in use by {}", holder(port)))
        }
    })?;
    Ok(answer.parse().expect("checked"))
}

fn ask_backups(
    previous: Option<&Path>,
    recordings: &Path,
    archive: &Path,
) -> Result<Option<PathBuf>, inquire::InquireError> {
    const INSIDE: &str = "In the archive";
    const ELSEWHERE: &str = "In a folder I name";
    let choices = if previous.is_some() {
        vec![ELSEWHERE, INSIDE]
    } else {
        vec![INSIDE, ELSEWHERE]
    };
    if Select::new("Backups:", choices).prompt()? == INSIDE {
        return Ok(None);
    }
    let (recordings, archive) = (recordings.to_path_buf(), archive.to_path_buf());
    let answer = text(
        "Backup folder:",
        previous.map(|p| p.to_str().unwrap_or_default()),
        move |b| check_backups(Path::new(b), &recordings, &archive).map(drop),
    )?;
    Ok(Some(
        folders::check(Path::new(&answer))
            .map_err(|why| inquire::InquireError::Custom(why.into()))?
            .path,
    ))
}

fn ask_pushes(
    previous: Option<&[(String, String)]>,
) -> Result<Vec<(String, String)>, inquire::InquireError> {
    let mut pushes = Vec::new();
    for (kind, what) in [("pgbackrest", "pgBackRest backup"), ("dump", "dump")] {
        let had = previous
            .and_then(|p| p.iter().find(|(k, _)| k == kind))
            .map(|(_, url)| url.as_str());
        let url = text(
            &format!("Push address after each {what} (none: leave empty):"),
            had,
            |u| {
                if u.is_empty() { Ok(()) } else { check_push(u) }
            },
        )?;
        if !url.is_empty() {
            pushes.push((kind.to_string(), url));
        }
    }
    Ok(pushes)
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

    #[test]
    fn an_answers_file_gives_advanced_answers_each_checked() {
        use crate::setup::steps::{Certificate, Database, Remote};
        let dir = tempfile::tempdir().unwrap();
        let [rec, arc, bak] = ["rec", "arc", "bak"].map(|n| dir.path().join(n));
        for d in [&rec, &arc, &bak] {
            std::fs::create_dir(d).unwrap();
        }
        let (cert, key) = (dir.path().join("site.pem"), dir.path().join("site.key"));
        crate::cert::make(&["radio-a.example.org".into()], &cert, &key).unwrap();
        // No PORT: checking one binds it, which races the other test that
        // checks port 1 wherever low ports may be bound (macOS, root).
        let base = format!(
            "RECORDINGS={}\nARCHIVE={}\nADMIN=chief\nADMIN_PASSWORD=\"long enough secret\"\n",
            rec.display(),
            arc.display()
        );
        let theirs = format!(
            "{base}DATABASE=theirs\nDB_HOST=db.example.org\nDB_PORT=5433\nDB_NAME=calls\nDB_ROLE=sp\nDB_PASSWORD=\"se:cret\"\n\
             CERT_FILE={}\nKEY_FILE={}\nPROXIES=\"192.0.2.5 2001:db8::5\"\nSYSLOG=udp://192.0.2.30:514\n",
            cert.display(),
            key.display()
        );
        let answers = from_file(&answers_file(dir.path(), &theirs, 0o600), &defaults()).unwrap();
        assert!(answers.advanced);
        assert_eq!(
            answers.database,
            Database::Theirs(Remote {
                host: "db.example.org".into(),
                port: 5433,
                name: "calls".into(),
                role: "sp".into(),
                password: "se:cret".into(),
            })
        );
        assert_eq!(
            answers.certificate,
            Certificate::Installed {
                cert: cert.clone(),
                key: key.clone()
            }
        );
        assert_eq!(answers.proxies, ["192.0.2.5", "2001:db8::5"]);
        assert_eq!(answers.syslog.as_deref(), Some("udp://192.0.2.30:514"));
        let refused = |extra: &str, why: &str| {
            let error = from_file(
                &answers_file(dir.path(), &format!("{base}{extra}"), 0o600),
                &defaults(),
            )
            .unwrap_err();
            assert!(error.contains(why), "{extra}: {error}");
        };
        refused(
            &format!(
                "DATABASE=theirs\nDB_HOST=h\nDB_NAME=n\nDB_ROLE=r\nDB_PASSWORD=p\nBACKUPS={}\n",
                bak.display()
            ),
            "keeps its own backups",
        );
        refused(
            "DATABASE=theirs\nDB_HOST=h\nDB_NAME=n\nDB_ROLE=r\nDB_PASSWORD=p\nPUSH_DUMP=https://kuma.example.org/api/push/x\n",
            "keeps its own backups",
        );
        refused(
            &format!("BACKUPS={}\n", arc.join("inner").display()),
            "does not exist",
        );
        std::fs::create_dir(arc.join("inner")).unwrap();
        refused(
            &format!("BACKUPS={}\n", arc.join("inner").display()),
            "may not be inside each other",
        );
        refused("PROXIES=proxy.lan\n", "is not an address");
        refused("SYSLOG=192.0.2.30\n", "udp://host:port");
        refused("PUSH_DUMP=kuma\n", "http:// or https://");
        refused(
            &format!(
                "CERT_FILE={}\nKEY_FILE={}\n",
                cert.display(),
                dir.path().join("none.key").display()
            ),
            "private key",
        );
        // Named through a link, the folder is kept as its real path, as the
        // question keeps it (pgBackRest's file and the units name it).
        let link = dir.path().join("bak-link");
        std::os::unix::fs::symlink(&bak, &link).unwrap();
        let here = format!(
            "{base}BACKUPS={}\nPUSH_DUMP=\"https://kuma.example.org/api/push/abc?status=up&msg=OK\"\n",
            link.display()
        );
        let answers = from_file(&answers_file(dir.path(), &here, 0o600), &defaults()).unwrap();
        assert_eq!(answers.database, Database::Here);
        assert_eq!(answers.backups, Some(bak.canonicalize().unwrap()));
        assert_eq!(
            answers.pushes,
            [(
                "dump".to_string(),
                "https://kuma.example.org/api/push/abc?status=up&msg=OK".to_string()
            )]
        );
    }
}
