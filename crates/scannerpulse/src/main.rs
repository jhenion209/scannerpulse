use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use scannerpulse::setup::change::Change;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use chrono::{NaiveDate, SecondsFormat, Utc};
use scannerpulse::MIGRATOR;
use scannerpulse::accuracy::{self, Candidate, Run};
use scannerpulse::backfill::{self, Ask};
use scannerpulse::evaluation::{self, Draw};
use scannerpulse::localtime::TimeHow;
use scannerpulse::reader::{self, FileCall, ReadError};
use scannerpulse::{accounts, favorites, ingest, jobs, lease, settings, talkgroups, web};
use serde_json::json;
use sqlx::Row;
use sqlx::postgres::PgConnection;

/// How long a command waits for the database server to accept the connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

const USAGE: &str = "\
usage: scannerpulse db-update [--quiet]
       scannerpulse ingest
       scannerpulse lease
       scannerpulse web
       scannerpulse read --zone ZONE FILE...
       scannerpulse transcribe --talkgroup SYSTEM/NUMBER --from DAY --to DAY [--yes]
       scannerpulse asr sample --set NAME --talkgroups FILE --calls N --days N
                               [--min-seconds S] [--seed N] [--yes]
       scannerpulse asr todo --set NAME
       scannerpulse asr report --set NAME
       scannerpulse asr sets
       scannerpulse asr run --set NAME --server URL --model MODEL [--prompt-file FILE]
       scannerpulse admin add|password|disable|enable NAME
       scannerpulse admin list
       scannerpulse admin password-signin on|off
       scannerpulse settings list
       scannerpulse settings set KEY JSON
       scannerpulse settings unset KEY
       scannerpulse favorites list
       scannerpulse favorites set NAME SYSTEM/NUMBER...
       scannerpulse favorites remove NAME
       scannerpulse viewers
       scannerpulse rescan
       scannerpulse talkgroups
       scannerpulse talkgroup transcribe SYSTEM/NUMBER on|off
       scannerpulse talkgroup rename SYSTEM/NUMBER NAME
       scannerpulse talkgroups uncorrected
       scannerpulse talkgroup correct SYSTEM/NUMBER [--confirm NAME]
       scannerpulse talkgroup merge SYSTEM/NUMBER --into SYSTEM/NUMBER [--confirm NAME]
       scannerpulse talkgroup purge SYSTEM/NUMBER [--confirm NAME]
       scannerpulse queue retry-failed
       scannerpulse status
       scannerpulse report traffic
       scannerpulse jobs list
       scannerpulse jobs show|stop ID
       scannerpulse jobs queue [--talkgroup SYSTEM/NUMBER] [--from DAY] [--to DAY] [--yes]
       scannerpulse jobs asr-run --set NAME --server URL --model MODEL [--prompt-file FILE]
       scannerpulse cert make --cert FILE --key FILE NAME...
       scannerpulse setup [--answers FILE] [--dry-run]
       scannerpulse backup
       scannerpulse tls install CERT KEY | tls self
       scannerpulse tls letsencrypt web|dns-cloudflare NAME...
       scannerpulse backups push pgbackrest|dump [off]
       scannerpulse backups location FOLDER
       scannerpulse second NAME [ADDRESS/PREFIX]
       scannerpulse uninstall [--remove-database] [--remove-backups] [--remove-archive]
       scannerpulse db promote | db rejoin | db switchover
       scannerpulse db move DB1 DB2 --hub ADDRESS [--referee REF]
       scannerpulse db move --switch | db move --retire
       scannerpulse update [--check | --yes]
       scannerpulse agent | agent install
       scannerpulse archive move FOLDER [--switch]
       scannerpulse node serve | node status | node backup
       scannerpulse join info NAME... | join done NODE
       scannerpulse --version
       scannerpulse --help

db-update   create or update the database schema (reads DATABASE_URL)
ingest      watch the sources and file their calls into the archive, sharing
            the work with the other host through the lease, and transcribe
            them when transcription is on (reads DATABASE_URL,
            SCANNERPULSE_HOST, SCANNERPULSE_PREFERRED, and
            SCANNERPULSE_TRANSCRIPTION_API_KEY for servers that need a key)
lease       show which host holds the processing lease, and each host's
            heartbeat; exit 0 when the lease has a valid holder
web         serve the web interface and its API (reads DATABASE_URL,
            SCANNERPULSE_WEB_LISTEN, default 127.0.0.1:8080; HTTPS with
            SCANNERPULSE_TLS_CERT and SCANNERPULSE_TLS_KEY, which any address
            other machines reach needs;
            SCANNERPULSE_WEB_STATIC, the frontend's built files, and
            SCANNERPULSE_HOST, its name in the list of open pages, and
            SCANNERPULSE_OIDC_CLIENT_SECRET when admins sign in through an
            OIDC provider)
read        print what ingest reads from each file, one JSON line per file;
            ZONE is the time zone of the times in the file names
transcribe  queue the past calls of a talkgroup for transcription, from DAY to
            DAY (YYYY-MM-DD, local days, both included): the calls never tried
            and at least the minimum length; asks first unless --yes
asr         the transcription improvement module, when improvement_module.enabled
            is true:
  sample    draw an evaluation set from the talkgroups of FILE (one
            SYSTEM/NUMBER a line), as evenly as their calls allow across the
            talkgroups and the months of the last N days; a set that people
            have labeled is never replaced
  todo      list the set's calls that have no person's text yet
  report    score production's first output against the people's texts
  sets      list the evaluation sets, their labeled calls and kept runs
  run       transcribe the set's labeled calls with a candidate server, model
            and prompt, without saving any transcript, and keep the score
            (reads SCANNERPULSE_TRANSCRIPTION_API_KEY for servers that need a
            key)
admin       the admin accounts:
  add       add an account; asks for its password twice
  password  give an account a new password and end its sessions
  disable   disable an account and end its sessions (never the last one)
  enable    enable a disabled account
  list      list the accounts
  password-signin
            allow or refuse signing in with a password (on by default; the
            command line can always turn it back on)
settings    list, set or remove the administrator's settings; a value is JSON
            (text in double quotes, a number, true or false, or a list) and
            is checked as the parts that read it check it
favorites   list, set or remove the named favorite lists of talkgroups
viewers     list the open pages of every web host
rescan      ask every running ingest to list its sources now
talkgroups  list the talkgroups and their transcription switches;
            uncorrected: those whose files still carry an old name
talkgroup   turn a talkgroup's transcription on or off (its new calls only;
            `jobs queue --talkgroup` queues its past calls); rename a
            talkgroup (its files keep the old name until they are corrected;
            the recorder's next file of it brings the recorder's name back);
            correct a talkgroup's files (folder, file name and title) after a
            rename, as a job, once its name is typed (or given by --confirm);
            merge a talkgroup into another of its system (its calls and
            favorites move, it goes, and the files are corrected), as a job,
            once the name of the talkgroup that goes is typed; purge a
            talkgroup (its calls are deleted and their files kept in the
            holding folder, setting purge.holding_path), as a job, once its
            name is typed
queue       retry-failed: put every failed call back in the queue
report      traffic: calls per 5 minutes over the last 120 days, as weekday and
            weekend profiles, and the quietest hours (for maintenance)
status      the admin page's Status tab, as JSON: the lease, ingest's files,
            the queue, the servers, the database, the checks after a failover
jobs        the long jobs, which the host holding the lease runs one at a time:
  list      the newest 50
  show      one job and its progress
  stop      stop a job (a running one stops after its current step)
  queue     queue past calls for transcription: those of one talkgroup, or
            without --talkgroup the backlog of every talkgroup whose
            transcription is on; from DAY and through DAY (local days) when
            given; asks first unless --yes
  asr-run   asr run as a job: the set's labeled calls, one a step
cert        make: write a certificate for the host names and addresses NAME
            (FILE, readable by all) and its private key (FILE, the owner's
            alone), good for five years; browsers warn once, since no
            authority signed it
setup       make this machine a ScannerPulse site, as root: asks Simple or
            Advanced, then the folders, the time zone, the first admin and
            transcription, shows what it will do, and does it after a yes
            (PostgreSQL 17, the scannerpulse account, the settings file, the
            certificate, the services); --answers FILE reads the answers
            instead (NAME=value lines, mode 600), --dry-run changes nothing.
            Run again on a site on one machine, it plans the site's
            conversion to high availability (the database machines, the
            referee, the second machine, the archive's share), which the
            admin page's Layout area then guides; CONVERT=yes in an answers
            file gives the plan instead
backup      take the nightly backups, as root (the backup timer runs it):
            pgBackRest (full on Sundays, differential otherwise) and a dump,
            the newest 14 kept, into SCANNERPULSE_BACKUPS; then each kind's
            time for the admin page, and its push address if one is set
tls         as root: install serves CERT and KEY (PEM, checked first) from
            now on; self goes back to a certificate this machine makes;
            letsencrypt has the site get Let's Encrypt's certificate for
            each NAME and renew it, proved over the web (port 443 from the
            internet) or through DNS at Cloudflare (asks for an API token,
            or reads it from standard input; never an argument)
backups     as root: push asks for an address (or reads it from standard
            input; never an argument, since it holds a token) to call after
            each backup of that kind that succeeds, and off removes it;
            location keeps the backups in FOLDER from
            now on, starting with a full backup there (the old ones stay)
db          as root, with two database machines: promote, on the standby
            when the primary is gone, makes it the primary (refused while
            the other still answers as one); rejoin, on a former primary,
            makes it the new primary's standby; switchover, on the
            ScannerPulse machine, swaps the roles while both are healthy.
            With a referee, it promotes and rejoins by itself, and
            switchover asks it. move, on an install on one machine, moves
            its database to DB1 and DB2 (ADDRESS is this machine's address
            as they reach them): the plan, and this database opened to them;
            --switch makes DB1 the primary once it has caught up; --retire
            removes the old database once you agree
second      as root, on the first ScannerPulse machine with two database
            machines: adds NAME, a second ScannerPulse machine, which
            shares ADDRESS/PREFIX with this one; the database machines
            join again, then NAME joins
node        a database machine's, the referee's or the second ScannerPulse
            machine's own: serve is what the first ScannerPulse machine's key
            may run there (status, stop, start, promote [after LSN], follow
            HOST, switchover, version, update PROGRAM); backup, from its
            timer, dumps the database on the primary
join        on the ScannerPulse machine, as root, for a joining machine
            (a database machine, the referee or the second ScannerPulse
            machine): info gives its place and
            the plan; done notes it
update      as root, on the first ScannerPulse machine (or one machine
            alone): finds the newest release, checks its signature, brings
            the database's tables up to date, then updates the referee, the
            database machines, the second ScannerPulse machine and this one,
            one at a time, each answering before the next; --check only says
            what is newest and what each machine runs; --yes asks nothing
archive     as root, on an install without a second machine: move FOLDER
            compares the archive with FOLDER (a network share, usually, for
            a second machine) after the person copied it there, and says what
            is missing and the copy to run; with --switch it pauses filing,
            copies the calls filed since, compares again, points the install
            at FOLDER and files again (the old archive stays as it was)
agent       as root, the first ScannerPulse machine's root helper (its unit
            runs it): it carries out an update, a switchover or a step of the
            conversion to high availability an admin asks for on the admin
            page, and looks for a newer release once a day
            unless updates.check is false; install writes and starts its unit
            (setup does; an install from before 0.9.0 needs it once)
uninstall   undo setup, as root: the services, their units, the settings,
            the account and the program go; the archive, the database and
            the backups stay unless each is named, and each named one is
            deleted only after its name is typed";

/// The hub's side of a database machine joining.
#[derive(Debug, PartialEq, Eq)]
enum Joined {
    Info(Vec<String>),
    Done(String),
}

/// A database machine's own commands.
#[derive(Debug, PartialEq, Eq)]
enum NodeCommand {
    Serve,
    Status,
    Backup,
}

/// Changing the database machines' roles.
#[derive(Debug, PartialEq, Eq)]
enum DbCommand {
    Promote,
    Rejoin,
    Switchover,
}

/// Moving the database from this machine to two database machines.
#[derive(Debug, PartialEq, Eq)]
enum DbMove {
    Begin {
        nodes: [String; 2],
        hub: String,
        referee: Option<String>,
    },
    Switch,
    Retire,
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    DbUpdate {
        quiet: bool,
    },
    Ingest,
    Lease,
    Web,
    Read {
        zone: String,
        files: Vec<String>,
    },
    Transcribe {
        talkgroup: String,
        from: String,
        to: String,
        yes: bool,
    },
    Asr(Asr),
    Admin(Admin),
    Settings(Settings),
    Favorites(Favorites),
    Viewers,
    Jobs(Jobs),
    Rescan,
    CertMake {
        cert: String,
        key: String,
        names: Vec<String>,
    },
    Setup {
        answers: Option<String>,
        dry_run: bool,
    },
    Backup,
    Change(scannerpulse::setup::change::Change),
    Uninstall(scannerpulse::setup::uninstall::Remove),
    Join(Joined),
    Node(NodeCommand),
    Db(DbCommand),
    DbMove(DbMove),
    /// `archive move NEW [--switch]`: the archive compared with NEW, or moved there.
    ArchiveMove {
        new: PathBuf,
        switch: bool,
    },
    /// `agent`: the root helper; `agent install` writes and starts its unit.
    Agent {
        install: bool,
    },
    /// `update`: `check` only reads the newest release; `yes` asks nothing.
    Update {
        check: bool,
        yes: bool,
    },
    Talkgroups,
    TalkgroupSwitch {
        talkgroup: String,
        on: bool,
    },
    TalkgroupRename {
        talkgroup: String,
        name: String,
    },
    Uncorrected,
    Data(Data),
    RetryFailed,
    Status,
    Traffic,
    Version,
    Help,
}

/// The Data tab's jobs, which move files; each starts once the talkgroup's
/// name is typed or given with `--confirm`.
#[derive(Debug, PartialEq, Eq)]
enum Data {
    Correct {
        talkgroup: String,
        confirm: Option<String>,
    },
    Merge {
        talkgroup: String,
        into: String,
        confirm: Option<String>,
    },
    Purge {
        talkgroup: String,
        confirm: Option<String>,
    },
}

/// The improvement module's commands.
#[derive(Debug, PartialEq, Eq)]
enum Asr {
    Sample {
        set: String,
        talkgroups: String,
        calls: String,
        days: String,
        min_seconds: Option<String>,
        seed: Option<String>,
        yes: bool,
    },
    Todo {
        set: String,
    },
    Report {
        set: String,
    },
    Sets,
    Run {
        set: String,
        server: String,
        model: String,
        prompt_file: Option<String>,
    },
}

/// The admin accounts' commands.
#[derive(Debug, PartialEq, Eq)]
enum Admin {
    Add(String),
    Password(String),
    Disable(String),
    Enable(String),
    List,
    PasswordSignin(bool),
}

/// The jobs' commands.
#[derive(Debug, PartialEq, Eq)]
enum Jobs {
    List,
    Show(i64),
    Stop(i64),
    Queue {
        talkgroup: Option<String>,
        from: Option<String>,
        to: Option<String>,
        yes: bool,
    },
    AsrRun {
        set: String,
        server: String,
        model: String,
        prompt_file: Option<String>,
    },
}

fn parse_jobs(args: &[&str]) -> Option<Jobs> {
    let text = |found: &HashMap<&str, &str>, flag: &str| found.get(flag).map(|v| v.to_string());
    match args {
        ["list"] => Some(Jobs::List),
        ["show", id] => id.parse().ok().map(Jobs::Show),
        ["stop", id] => id.parse().ok().map(Jobs::Stop),
        ["queue", rest @ ..] => {
            let found = flags(rest, &[], &["--talkgroup", "--from", "--to"], &["--yes"])?;
            Some(Jobs::Queue {
                talkgroup: text(&found, "--talkgroup"),
                from: text(&found, "--from"),
                to: text(&found, "--to"),
                yes: found.contains_key("--yes"),
            })
        }
        ["asr-run", rest @ ..] => {
            let found = flags(
                rest,
                &["--set", "--server", "--model"],
                &["--prompt-file"],
                &[],
            )?;
            Some(Jobs::AsrRun {
                set: text(&found, "--set")?,
                server: text(&found, "--server")?,
                model: text(&found, "--model")?,
                prompt_file: text(&found, "--prompt-file"),
            })
        }
        _ => None,
    }
}

/// The favorite lists' commands.
#[derive(Debug, PartialEq, Eq)]
enum Favorites {
    List,
    Set(String, Vec<String>),
    Remove(String),
}

fn parse_favorites(args: &[&str]) -> Option<Favorites> {
    match args {
        ["list"] => Some(Favorites::List),
        ["set", name, talkgroups @ ..] if !talkgroups.is_empty() => Some(Favorites::Set(
            name.to_string(),
            talkgroups.iter().map(|t| t.to_string()).collect(),
        )),
        ["remove", name] => Some(Favorites::Remove(name.to_string())),
        _ => None,
    }
}

/// The settings' commands.
#[derive(Debug, PartialEq, Eq)]
enum Settings {
    List,
    Set(String, String),
    Unset(String),
}

fn parse_settings(args: &[&str]) -> Option<Settings> {
    match args {
        ["list"] => Some(Settings::List),
        ["set", key, value] => Some(Settings::Set(key.to_string(), value.to_string())),
        ["unset", key] => Some(Settings::Unset(key.to_string())),
        _ => None,
    }
}

fn parse_admin(args: &[&str]) -> Option<Admin> {
    match args {
        ["add", name] => Some(Admin::Add(name.to_string())),
        ["password", name] => Some(Admin::Password(name.to_string())),
        ["disable", name] => Some(Admin::Disable(name.to_string())),
        ["enable", name] => Some(Admin::Enable(name.to_string())),
        ["list"] => Some(Admin::List),
        ["password-signin", "on"] => Some(Admin::PasswordSignin(true)),
        ["password-signin", "off"] => Some(Admin::PasswordSignin(false)),
        _ => None,
    }
}

/// `--flag value` pairs and lone switches, each given once, in any order.
/// `None` when a flag is unknown, repeated or without its value, or when a
/// `required` flag is missing.
fn flags<'a>(
    args: &[&'a str],
    required: &[&str],
    optional: &[&str],
    switches: &[&str],
) -> Option<HashMap<&'a str, &'a str>> {
    let mut found = HashMap::new();
    let mut rest = args;
    while let [flag, tail @ ..] = rest {
        let value = if switches.contains(flag) {
            rest = tail;
            ""
        } else if required.contains(flag) || optional.contains(flag) {
            let [value, tail @ ..] = tail else {
                return None;
            };
            if value.starts_with("--") {
                return None;
            }
            rest = tail;
            *value
        } else {
            return None;
        };
        if found.insert(*flag, value).is_some() {
            return None;
        }
    }
    required
        .iter()
        .all(|flag| found.contains_key(flag))
        .then_some(found)
}

fn parse_asr(args: &[&str]) -> Option<Asr> {
    let text = |found: &HashMap<&str, &str>, flag: &str| found.get(flag).map(|v| v.to_string());
    match args {
        ["sample", rest @ ..] => {
            let found = flags(
                rest,
                &["--set", "--talkgroups", "--calls", "--days"],
                &["--min-seconds", "--seed"],
                &["--yes"],
            )?;
            Some(Asr::Sample {
                set: text(&found, "--set")?,
                talkgroups: text(&found, "--talkgroups")?,
                calls: text(&found, "--calls")?,
                days: text(&found, "--days")?,
                min_seconds: text(&found, "--min-seconds"),
                seed: text(&found, "--seed"),
                yes: found.contains_key("--yes"),
            })
        }
        ["todo", rest @ ..] => Some(Asr::Todo {
            set: text(&flags(rest, &["--set"], &[], &[])?, "--set")?,
        }),
        ["sets"] => Some(Asr::Sets),
        ["report", rest @ ..] => Some(Asr::Report {
            set: text(&flags(rest, &["--set"], &[], &[])?, "--set")?,
        }),
        ["run", rest @ ..] => {
            let found = flags(
                rest,
                &["--set", "--server", "--model"],
                &["--prompt-file"],
                &[],
            )?;
            Some(Asr::Run {
                set: text(&found, "--set")?,
                server: text(&found, "--server")?,
                model: text(&found, "--model")?,
                prompt_file: text(&found, "--prompt-file"),
            })
        }
        _ => None,
    }
}

/// What the command line asks for; `None` when it is not understood. The words
/// must match exactly: a mistyped flag must not run an update.
fn parse(args: &[&str]) -> Option<Command> {
    match args {
        ["db-update"] => Some(Command::DbUpdate { quiet: false }),
        ["db-update", "--quiet"] => Some(Command::DbUpdate { quiet: true }),
        ["ingest"] => Some(Command::Ingest),
        ["lease"] => Some(Command::Lease),
        ["web"] => Some(Command::Web),
        ["read", "--zone", zone, files @ ..] if !files.is_empty() => Some(Command::Read {
            zone: zone.to_string(),
            files: files.iter().map(|file| file.to_string()).collect(),
        }),
        [
            "transcribe",
            "--talkgroup",
            talkgroup,
            "--from",
            from,
            "--to",
            to,
            rest @ ..,
        ] if matches!(rest, [] | ["--yes"]) => Some(Command::Transcribe {
            talkgroup: talkgroup.to_string(),
            from: from.to_string(),
            to: to.to_string(),
            yes: !rest.is_empty(),
        }),
        ["asr", rest @ ..] => parse_asr(rest).map(Command::Asr),
        ["admin", rest @ ..] => parse_admin(rest).map(Command::Admin),
        ["settings", rest @ ..] => parse_settings(rest).map(Command::Settings),
        ["favorites", rest @ ..] => parse_favorites(rest).map(Command::Favorites),
        ["viewers"] => Some(Command::Viewers),
        ["jobs", rest @ ..] => parse_jobs(rest).map(Command::Jobs),
        ["rescan"] => Some(Command::Rescan),
        ["cert", "make", "--cert", cert, "--key", key, names @ ..] if !names.is_empty() => {
            Some(Command::CertMake {
                cert: cert.to_string(),
                key: key.to_string(),
                names: names.iter().map(|n| n.to_string()).collect(),
            })
        }
        ["setup", rest @ ..] => parse_setup(rest),
        ["backup"] => Some(Command::Backup),
        ["tls", "install", cert, key] => Some(Command::Change(Change::TlsInstall {
            cert: PathBuf::from(cert),
            key: PathBuf::from(key),
        })),
        ["tls", "self"] => Some(Command::Change(Change::TlsSelf)),
        [
            "tls",
            "letsencrypt",
            proof @ ("web" | "dns-cloudflare"),
            names @ ..,
        ] if !names.is_empty() => Some(Command::Change(Change::LetsEncrypt {
            dns: *proof == "dns-cloudflare",
            names: names.iter().map(|n| n.to_string()).collect(),
        })),
        ["backups", "push", kind] => Some(Command::Change(Change::Push {
            kind: kind.to_string(),
            off: false,
        })),
        ["backups", "push", kind, "off"] => Some(Command::Change(Change::Push {
            kind: kind.to_string(),
            off: true,
        })),
        ["second", name] => Some(Command::Change(Change::Second {
            name: name.to_ascii_lowercase(),
            shared: None,
        })),
        ["second", name, shared] => Some(Command::Change(Change::Second {
            name: name.to_ascii_lowercase(),
            shared: Some(shared.to_string()),
        })),
        ["backups", "location", folder] => {
            Some(Command::Change(Change::Location(PathBuf::from(folder))))
        }
        ["uninstall", rest @ ..] => parse_uninstall(rest),
        ["join", "info", names @ ..] if !names.is_empty() => Some(Command::Join(Joined::Info(
            names.iter().map(|n| n.to_string()).collect(),
        ))),
        ["join", "done", node] => Some(Command::Join(Joined::Done(node.to_string()))),
        ["node", "serve"] => Some(Command::Node(NodeCommand::Serve)),
        ["node", "status"] => Some(Command::Node(NodeCommand::Status)),
        ["node", "backup"] => Some(Command::Node(NodeCommand::Backup)),
        ["db", "promote"] => Some(Command::Db(DbCommand::Promote)),
        ["db", "rejoin"] => Some(Command::Db(DbCommand::Rejoin)),
        ["db", "switchover"] => Some(Command::Db(DbCommand::Switchover)),
        ["db", "move", "--switch"] => Some(Command::DbMove(DbMove::Switch)),
        ["db", "move", "--retire"] => Some(Command::DbMove(DbMove::Retire)),
        ["db", "move", first, second, "--hub", hub] => Some(Command::DbMove(DbMove::Begin {
            nodes: [first.to_string(), second.to_string()],
            hub: hub.to_string(),
            referee: None,
        })),
        [
            "db",
            "move",
            first,
            second,
            "--hub",
            hub,
            "--referee",
            referee,
        ] => Some(Command::DbMove(DbMove::Begin {
            nodes: [first.to_string(), second.to_string()],
            hub: hub.to_string(),
            referee: Some(referee.to_string()),
        })),
        ["archive", "move", new] => Some(Command::ArchiveMove {
            new: PathBuf::from(new),
            switch: false,
        }),
        ["archive", "move", new, "--switch"] => Some(Command::ArchiveMove {
            new: PathBuf::from(new),
            switch: true,
        }),
        ["agent"] => Some(Command::Agent { install: false }),
        ["agent", "install"] => Some(Command::Agent { install: true }),
        ["update"] => Some(Command::Update {
            check: false,
            yes: false,
        }),
        ["update", "--yes"] => Some(Command::Update {
            check: false,
            yes: true,
        }),
        ["update", "--check"] => Some(Command::Update {
            check: true,
            yes: false,
        }),
        ["talkgroups"] => Some(Command::Talkgroups),
        ["talkgroups", "uncorrected"] => Some(Command::Uncorrected),
        ["talkgroup", "correct", talkgroup, rest @ ..] => {
            let found = flags(rest, &[], &["--confirm"], &[])?;
            Some(Command::Data(Data::Correct {
                talkgroup: talkgroup.to_string(),
                confirm: found.get("--confirm").map(|c| c.to_string()),
            }))
        }
        ["talkgroup", "merge", talkgroup, rest @ ..] => {
            let found = flags(rest, &["--into"], &["--confirm"], &[])?;
            Some(Command::Data(Data::Merge {
                talkgroup: talkgroup.to_string(),
                into: found["--into"].to_string(),
                confirm: found.get("--confirm").map(|c| c.to_string()),
            }))
        }
        ["talkgroup", "purge", talkgroup, rest @ ..] => {
            let found = flags(rest, &[], &["--confirm"], &[])?;
            Some(Command::Data(Data::Purge {
                talkgroup: talkgroup.to_string(),
                confirm: found.get("--confirm").map(|c| c.to_string()),
            }))
        }
        ["talkgroup", "rename", talkgroup, name] => Some(Command::TalkgroupRename {
            talkgroup: talkgroup.to_string(),
            name: name.to_string(),
        }),
        ["talkgroup", "transcribe", talkgroup, switch] => match *switch {
            "on" | "off" => Some(Command::TalkgroupSwitch {
                talkgroup: talkgroup.to_string(),
                on: *switch == "on",
            }),
            _ => None,
        },
        ["queue", "retry-failed"] => Some(Command::RetryFailed),
        ["status"] => Some(Command::Status),
        ["report", "traffic"] => Some(Command::Traffic),
        ["--version"] => Some(Command::Version),
        ["--help"] => Some(Command::Help),
        _ => None,
    }
}

fn parse_uninstall(rest: &[&str]) -> Option<Command> {
    let mut remove = scannerpulse::setup::uninstall::Remove::default();
    for flag in rest {
        let part = match *flag {
            "--remove-database" => &mut remove.database,
            "--remove-backups" => &mut remove.backups,
            "--remove-archive" => &mut remove.archive,
            _ => return None,
        };
        if *part {
            return None;
        }
        *part = true;
    }
    Some(Command::Uninstall(remove))
}

fn parse_setup(mut rest: &[&str]) -> Option<Command> {
    let (mut answers, mut dry_run) = (None, false);
    loop {
        match rest {
            [] => return Some(Command::Setup { answers, dry_run }),
            ["--answers", file, more @ ..] if answers.is_none() => {
                answers = Some(file.to_string());
                rest = more;
            }
            ["--dry-run", more @ ..] if !dry_run => {
                dry_run = true;
                rest = more;
            }
            _ => return None,
        }
    }
}

fn main() -> ExitCode {
    // The machine's settings file, before any thread exists: a variable
    // already in the environment wins.
    let path = std::env::var_os("SCANNERPULSE_SETTINGS_FILE")
        .unwrap_or_else(|| scannerpulse::hostfile::DEFAULT_PATH.into());
    match scannerpulse::hostfile::to_apply(Path::new(&path), |name| {
        std::env::var_os(name).is_some()
    }) {
        Ok(pairs) => {
            for (name, value) in pairs {
                // SAFETY: no other thread exists yet, so none reads the
                // environment while it changes.
                unsafe { std::env::set_var(name, value) };
            }
        }
        Err(error) => {
            eprintln!("scannerpulse: {error}");
            return ExitCode::FAILURE;
        }
    }
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(run()),
        Err(error) => {
            eprintln!("scannerpulse: cannot start: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> ExitCode {
    // Lossy, so an argument that is not valid UTF-8 gets the usage text, not a panic.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match parse(&args) {
        Some(Command::DbUpdate { quiet }) => match db_update(quiet).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("db-update failed: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Ingest) => match ingest::Config::from_env() {
            Ok(config) => {
                let code = match ingest::run(config, shutdown()).await {
                    Ok(()) => 0,
                    Err(error) => {
                        scannerpulse::log::error(format!("ingest cannot run: {error}"));
                        1
                    }
                };
                // Exit now: dropping the runtime would wait for every blocking
                // thread, and a probe stuck on a hung mount never ends.
                std::process::exit(code)
            }
            Err(error) => {
                eprintln!("ingest: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Lease) => lease_status().await,
        Some(Command::Web) => match web::Config::from_env() {
            Ok(config) => match web::run(config, shutdown()).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    scannerpulse::log::error(format!("web cannot run: {error}"));
                    ExitCode::FAILURE
                }
            },
            Err(error) => {
                eprintln!("web: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Read { zone, files }) => read(&zone, &files),
        Some(Command::Transcribe {
            talkgroup,
            from,
            to,
            yes,
        }) => match transcribe(&talkgroup, &from, &to, yes).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("transcribe: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Asr(command)) => match asr(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("asr: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Admin(command)) => match admin(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("admin: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Settings(command)) => match settings_command(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("settings: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Favorites(command)) => match favorites_command(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("favorites: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Jobs(command)) => match jobs_command(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("jobs: {error}");
                ExitCode::FAILURE
            }
        },
        Some(
            command @ (Command::Talkgroups
            | Command::TalkgroupSwitch { .. }
            | Command::TalkgroupRename { .. }
            | Command::Uncorrected
            | Command::RetryFailed),
        ) => match transcription_command(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Data(command)) => match data_command(command).await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("talkgroup: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Traffic) => match traffic_command().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("report: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Status) => match status_command().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("status: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::CertMake { cert, key, names }) => {
            match scannerpulse::cert::make(&names, Path::new(&cert), Path::new(&key)) {
                Ok(()) => {
                    println!("wrote the certificate {cert} and its key {key}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("cert make: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Command::Setup { answers, dry_run }) => {
            match scannerpulse::setup::run(answers.as_deref().map(Path::new), dry_run).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("setup: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Command::Backup) => {
            let (Ok(backups), Ok(url)) = (
                std::env::var("SCANNERPULSE_BACKUPS"),
                std::env::var("DATABASE_URL"),
            ) else {
                eprintln!(
                    "backup: SCANNERPULSE_BACKUPS and DATABASE_URL must be set; setup writes both in the settings file"
                );
                return ExitCode::FAILURE;
            };
            let mut failed = Vec::new();
            for (kind, outcome) in scannerpulse::backup::nightly(Path::new(&backups), &url) {
                match outcome {
                    Ok(done) => {
                        println!("{done}");
                        backup_succeeded(kind, &url).await;
                    }
                    Err(why) => failed.push(why),
                }
            }
            if failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                eprintln!("backup: {}", failed.join("; "));
                ExitCode::FAILURE
            }
        }
        Some(Command::Change(change)) => {
            match scannerpulse::setup::change::run_change(change).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Command::Join(joined)) => node_answer(join_command(joined)),
        Some(Command::Node(command)) => node_answer(match command {
            NodeCommand::Serve => as_root().and_then(|()| scannerpulse::setup::node::serve()),
            NodeCommand::Status => as_root().and_then(|()| {
                serde_json::to_string(&scannerpulse::setup::node::status())
                    .map_err(|e| e.to_string())
            }),
            NodeCommand::Backup => scannerpulse::setup::node::backup(),
        }),
        Some(Command::Db(command)) => node_answer(as_root().and_then(|()| match command {
            DbCommand::Promote => scannerpulse::setup::node::db_promote(),
            DbCommand::Rejoin => scannerpulse::setup::node::db_rejoin(|question| {
                inquire::Confirm::new(question)
                    .with_default(false)
                    .prompt()
                    .unwrap_or(false)
            }),
            DbCommand::Switchover => {
                scannerpulse::setup::node::db_switchover(|line| println!("{line}"))
            }
        })),
        Some(Command::DbMove(command)) => {
            let result = match as_root() {
                Err(error) => Err(error),
                Ok(()) => match command {
                    DbMove::Begin {
                        nodes,
                        hub,
                        referee,
                    } => {
                        // Run again, the move reads its plan: the database
                        // may be restarting under pg_autoctl.
                        let mut conn = connect().await.ok();
                        scannerpulse::setup::db_move::begin(conn.as_mut(), nodes, hub, referee)
                            .await
                    }
                    // A terminal gone mid-switch must not panic it.
                    DbMove::Switch => {
                        scannerpulse::setup::db_move::switch(|line| {
                            let _ = writeln!(std::io::stdout(), "{line}");
                        })
                        .await
                    }
                    DbMove::Retire => {
                        scannerpulse::setup::db_move::retire(|question| {
                            inquire::Confirm::new(question)
                                .with_default(false)
                                .prompt()
                                .unwrap_or(false)
                        })
                        .await
                    }
                },
            };
            node_answer(result.map_err(|error| format!("db move: {error}")))
        }
        Some(Command::ArchiveMove { new, switch }) => {
            let result = match as_root() {
                Ok(()) => match connect().await {
                    Ok(mut conn) => {
                        // A terminal gone mid-switch must not panic it.
                        scannerpulse::setup::archive_move::run(&mut conn, &new, switch, |line| {
                            let _ = writeln!(std::io::stdout(), "{line}");
                        })
                        .await
                        .map(|(_, said)| said)
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error),
            };
            node_answer(result.map_err(|error| format!("archive move: {error}")))
        }
        Some(Command::Agent { install: true }) => {
            node_answer(as_root().and_then(|()| scannerpulse::agent::install()))
        }
        Some(Command::Agent { install: false }) => match as_root() {
            Ok(()) => match scannerpulse::agent::serve(shutdown()).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    scannerpulse::log::error(format!("agent cannot run: {error}"));
                    ExitCode::FAILURE
                }
            },
            Err(error) => {
                eprintln!("agent: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Update { check, yes }) => {
            let result = match as_root() {
                Ok(()) => {
                    scannerpulse::update::run(
                        check,
                        false,
                        |line| println!("{line}"),
                        |question| {
                            yes || inquire::Confirm::new(question)
                                .with_default(false)
                                .prompt()
                                .unwrap_or(false)
                        },
                    )
                    .await
                }
                Err(error) => Err(error),
            };
            node_answer(result.map_err(|error| format!("update: {error}")))
        }
        Some(Command::Uninstall(remove)) => {
            match scannerpulse::setup::uninstall::run_uninstall(remove).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("uninstall: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        Some(Command::Rescan) => match connect().await {
            Ok(mut conn) => match ingest::request_rescan(&mut conn).await {
                Ok(()) => {
                    println!("every running ingest lists its sources now");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("rescan: {error}");
                    ExitCode::FAILURE
                }
            },
            Err(error) => {
                eprintln!("rescan: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Viewers) => match viewers().await {
            Ok(code) => code,
            Err(error) => {
                eprintln!("viewers: {error}");
                ExitCode::FAILURE
            }
        },
        Some(Command::Version) => {
            println!("scannerpulse {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some(Command::Help) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// Completes at Ctrl-C or SIGTERM, which systemd sends at a stop.
async fn shutdown() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        Err(_) => {
            let _ = tokio::signal::ctrl_c().await;
        }
    }
}

/// Prints a node or join command's answer, or its error.
fn node_answer(result: Result<String, String>) -> ExitCode {
    match result {
        Ok(text) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn as_root() -> Result<(), String> {
    if rustix::process::geteuid().is_root() {
        Ok(())
    } else {
        Err("this changes the machine's own database: run it as root (sudo /usr/local/bin/scannerpulse ...)".into())
    }
}

/// The hub's side of a join, as root: the plan is root's alone.
fn join_command(joined: Joined) -> Result<String, String> {
    as_root()?;
    match joined {
        Joined::Info(names) => scannerpulse::setup::join::info(&names),
        Joined::Done(node) => {
            let mut keys = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut keys)
                .map_err(|error| format!("cannot read the host keys: {error}"))?;
            scannerpulse::setup::join::done(&node, &keys)
        }
    }
}

/// One connection to DATABASE_URL, not a pool: a refused connection fails at
/// once with the real error. An address that never answers would wait for the
/// operating system's own limit, which is minutes, so give up after
/// CONNECT_TIMEOUT.
async fn connect() -> Result<PgConnection, Box<dyn std::error::Error>> {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    Ok(scannerpulse::primary::connect(&url, CONNECT_TIMEOUT).await?)
}

async fn lease_status() -> ExitCode {
    let status = match connect().await {
        Ok(mut conn) => lease::status(&mut conn).await.map_err(Into::into),
        Err(error) => Err(error),
    };
    match status {
        Ok(status) => {
            for line in status.lines() {
                println!("{line}");
            }
            if status.valid() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("lease: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn db_update(quiet: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = connect().await?;
    let before = applied(&mut conn).await?;
    MIGRATOR.run(&mut conn).await?;
    if !quiet {
        let rows =
            sqlx::query("SELECT version, description FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&mut conn)
                .await?;
        for row in &rows {
            let version: i64 = row.get("version");
            let description: String = row.get("description");
            let state = if before.contains(&version) {
                "present"
            } else {
                "applied now"
            };
            println!("{version:04} {description}: {state}");
        }
        println!(
            "schema is up to date ({} applied now)",
            rows.len() - before.len()
        );
    }
    Ok(())
}

/// Queues the past calls of a talkgroup, with a count first and progress.
async fn transcribe(
    talkgroup: &str,
    from: &str,
    to: &str,
    yes: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let (system, number) =
        talkgroup_name(talkgroup).ok_or("write the talkgroup as SYSTEM/NUMBER")?;
    let day = |text: &str| {
        NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .map_err(|_| format!("{text} is not a day such as 2031-01-15"))
    };
    let ask = Ask {
        system: system.to_string(),
        number,
        from: day(from)?,
        to: day(to)?,
    };
    let mut conn = connect().await?;
    let zone = settings::load(&mut conn).await?.time_zone;
    let transcription = settings::load_transcription(&mut conn)
        .await?
        .ok_or("transcription is off: turn it on first")?;
    let min_ms = transcription.min_duration_ms;
    let plan = backfill::plan(&mut conn, &ask, zone, min_ms).await?;
    println!(
        "{} calls of {talkgroup} from {from} to {to} were never tried and are at least {min_ms} ms long",
        plan.count
    );
    if !plan.talkgroup_on {
        println!("note: transcription is off for this talkgroup, so its new calls are not queued");
    }
    if plan.count == 0 {
        return Ok(());
    }
    if !yes && !confirm(&format!("queue {} calls? [y/N] ", plan.count))? {
        println!("nothing was queued");
        return Ok(());
    }
    let started = Instant::now();
    let total = plan.count;
    let queued = backfill::run(&mut conn, &plan.past, 1000, |n| {
        let rate = n as f64 / started.elapsed().as_secs_f64().max(0.001);
        println!("queued {n} of {total} ({rate:.0} calls/s)");
    })
    .await?;
    println!(
        "done: {queued} calls queued in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// A talkgroup written SYSTEM/NUMBER.
fn talkgroup_name(text: &str) -> Option<(&str, i64)> {
    text.rsplit_once('/')
        .and_then(|(system, number)| Some((system, number.parse::<i64>().ok()?)))
        .filter(|(system, _)| !system.is_empty())
}

type CommandResult = Result<ExitCode, Box<dyn std::error::Error>>;

async fn admin(command: Admin) -> CommandResult {
    let mut conn = connect().await?;
    match command {
        Admin::Add(login) => {
            accounts::check_login(&login)?;
            let password = new_password()?;
            accounts::add(&mut conn, &login, &password).await?;
            println!("added {login}");
        }
        Admin::Password(login) => {
            let password = new_password()?;
            accounts::set_password(&mut conn, &login, &password).await?;
            println!("{login} has a new password; its sessions have ended");
        }
        Admin::Disable(login) => {
            accounts::disable(&mut conn, &login).await?;
            println!("{login} is disabled; its sessions have ended");
        }
        Admin::Enable(login) => {
            accounts::enable(&mut conn, &login).await?;
            println!("{login} is enabled");
        }
        Admin::List => {
            for admin in accounts::list(&mut conn).await? {
                let how = match (admin.password, admin.oidc) {
                    (true, true) => "password and OIDC",
                    (true, false) => "password",
                    (false, true) => "OIDC",
                    (false, false) => "no way to sign in",
                };
                let last = admin
                    .last_sign_in
                    .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
                    .unwrap_or_else(|| "never".into());
                println!(
                    "{}\t{}\t{how}\tlast sign-in {last}",
                    admin.login,
                    if admin.disabled {
                        "disabled"
                    } else {
                        "enabled"
                    },
                );
            }
        }
        Admin::PasswordSignin(on) => {
            accounts::set_password_signin(&mut conn, on).await?;
            println!("password sign-in is {}", if on { "on" } else { "off" });
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn settings_command(command: Settings) -> CommandResult {
    let mut conn = connect().await?;
    match command {
        Settings::List => {
            let mut values: Vec<(String, serde_json::Value)> =
                settings::all(&mut conn).await?.into_iter().collect();
            values.sort_by(|a, b| a.0.cmp(&b.0));
            for (key, value) in values {
                println!("{key}\t{value}");
            }
        }
        Settings::Set(key, text) => {
            let value: serde_json::Value = serde_json::from_str(&text).map_err(|_| {
                format!("{text} is not JSON: write text in quotes, such as '\"UTC\"'")
            })?;
            settings::change(&mut conn, &key, Some(&value)).await?;
            println!("{key} is {value}");
        }
        Settings::Unset(key) => {
            settings::change(&mut conn, &key, None).await?;
            println!("{key} is removed");
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn favorites_command(command: Favorites) -> CommandResult {
    let mut conn = connect().await?;
    match command {
        Favorites::List => {
            let rows: Vec<(String, Vec<String>)> = sqlx::query_as(
                "SELECT f.name, COALESCE(array_agg(s.name || '/' || t.number ORDER BY s.name, t.number) \
                        FILTER (WHERE t.id IS NOT NULL), '{}') \
                 FROM favorites f \
                 LEFT JOIN favorite_talkgroups ft ON ft.favorite_id = f.id \
                 LEFT JOIN talkgroups t ON t.id = ft.talkgroup_id \
                 LEFT JOIN systems s ON s.id = t.system_id \
                 GROUP BY f.name ORDER BY f.name",
            )
            .fetch_all(&mut conn)
            .await?;
            for (name, talkgroups) in rows {
                println!("{name}\t{}", talkgroups.join(" "));
            }
        }
        Favorites::Set(name, talkgroups) => {
            let mut ids = Vec::new();
            for text in &talkgroups {
                let (system, number) =
                    talkgroup_name(text).ok_or_else(|| format!("{text} is not SYSTEM/NUMBER"))?;
                let id: Option<i32> = sqlx::query_scalar(
                    "SELECT t.id FROM talkgroups t JOIN systems s ON s.id = t.system_id \
                     WHERE s.name = $1 AND t.number = $2",
                )
                .bind(system)
                .bind(number)
                .fetch_optional(&mut conn)
                .await?;
                ids.push(id.ok_or_else(|| format!("there is no talkgroup {text}"))?);
            }
            favorites::set(&mut conn, &name, &ids).await?;
            println!("{name} holds {} talkgroups", ids.len());
        }
        Favorites::Remove(name) => {
            favorites::remove(&mut conn, &name).await?;
            println!("{name} is removed");
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn transcription_command(command: Command) -> CommandResult {
    let mut conn = connect().await?;
    match command {
        Command::Talkgroups => {
            for t in talkgroups::list(&mut conn).await? {
                println!(
                    "{}/{}\t{}\t{}",
                    t.system,
                    t.number,
                    if t.transcribe { "on" } else { "off" },
                    t.name
                );
            }
        }
        Command::TalkgroupSwitch { talkgroup, on } => {
            let (system, number) =
                talkgroup_name(&talkgroup).ok_or("write the talkgroup as SYSTEM/NUMBER")?;
            let id = talkgroups::find(&mut conn, system, number)
                .await?
                .ok_or_else(|| format!("there is no talkgroup {talkgroup}"))?;
            talkgroups::set_transcribe(&mut conn, id, on).await?;
            println!(
                "transcription is {} for {talkgroup}'s new calls",
                if on { "on" } else { "off" }
            );
        }
        Command::TalkgroupRename { talkgroup, name } => {
            if let Some(problem) = talkgroups::name_problem(&name) {
                return Err(problem.into());
            }
            let (system, number) =
                talkgroup_name(&talkgroup).ok_or("write the talkgroup as SYSTEM/NUMBER")?;
            let id = talkgroups::find(&mut conn, system, number)
                .await?
                .ok_or_else(|| format!("there is no talkgroup {talkgroup}"))?;
            match talkgroups::rename(&mut conn, id, &name).await? {
                talkgroups::Renamed::Done {
                    files_correct: true,
                } => {
                    println!("{talkgroup} is {name:?}; it has no calls to correct")
                }
                talkgroups::Renamed::Done { .. } => println!(
                    "{talkgroup} is {name:?}; its files keep the old name until \
                     `talkgroup correct {talkgroup}` runs"
                ),
                talkgroups::Renamed::Same => println!("{talkgroup} has that name already"),
                talkgroups::Renamed::Taken(_, other) => {
                    return Err(format!(
                        "{other} has that name already: merge into it with \
                         `talkgroup merge {talkgroup} --into {other}`"
                    )
                    .into());
                }
                talkgroups::Renamed::NoSuch => {
                    return Err(format!("there is no talkgroup {talkgroup}").into());
                }
            }
        }
        Command::Uncorrected => {
            for t in talkgroups::uncorrected(&mut conn).await? {
                println!(
                    "{}/{}\t{} calls\t{}\t(files: {})",
                    t.system,
                    t.number,
                    t.calls,
                    t.name,
                    t.files_name.as_deref().unwrap_or("?")
                );
            }
        }
        Command::RetryFailed => {
            let retried = scannerpulse::queue::retry_failed(&mut conn).await?;
            println!("{retried} failed calls are back in the queue");
        }
        _ => unreachable!("only the transcription commands come here"),
    }
    Ok(ExitCode::SUCCESS)
}

/// The id of a talkgroup written SYSTEM/NUMBER.
async fn talkgroup_id(conn: &mut sqlx::PgConnection, written: &str) -> Result<i32, String> {
    let (system, number) = talkgroup_name(written).ok_or("write the talkgroup as SYSTEM/NUMBER")?;
    talkgroups::find(&mut *conn, system, number)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("there is no talkgroup {written}"))
}

/// The confirmation given, or the line typed after `question`.
fn typed(given: Option<String>, question: &str) -> std::io::Result<String> {
    if let Some(given) = given {
        return Ok(given);
    }
    print!("{question}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim_end_matches(['\n', '\r']).to_string())
}

async fn data_command(command: Data) -> CommandResult {
    let mut conn = connect().await?;
    let by = command_line_user();
    let (job, what) = match command {
        Data::Correct { talkgroup, confirm } => {
            let id = talkgroup_id(&mut conn, &talkgroup).await?;
            let preview = scannerpulse::data::correct_preview(&mut conn, id).await?;
            println!(
                "{talkgroup} {:?}: {} files to correct; {} in read-only locations stay as they are",
                preview.talkgroup.name, preview.calls, preview.read_only
            );
            let confirm = typed(confirm, "type the talkgroup's name to start: ")?;
            let (job, count) =
                scannerpulse::data::start_correct(&mut conn, id, &confirm, &by).await?;
            (job, format!("correct {count} files"))
        }
        Data::Merge {
            talkgroup,
            into,
            confirm,
        } => {
            let from = talkgroup_id(&mut conn, &talkgroup).await?;
            let to = talkgroup_id(&mut conn, &into).await?;
            let preview = scannerpulse::data::merge_preview(&mut conn, from, to).await?;
            println!(
                "{talkgroup} {:?} goes into {into} {:?}: {} calls move, then their files are corrected",
                preview.from.name, preview.into.name, preview.calls
            );
            let confirm = typed(
                confirm,
                "type the name of the talkgroup that goes, to start: ",
            )?;
            let (job, count) =
                scannerpulse::data::start_merge(&mut conn, from, to, &confirm, &by).await?;
            (job, format!("merge {count} calls into {into}"))
        }
        Data::Purge { talkgroup, confirm } => {
            let id = talkgroup_id(&mut conn, &talkgroup).await?;
            let preview = scannerpulse::data::purge_preview(&mut conn, id).await?;
            println!(
                "{talkgroup} {:?}: {} calls are deleted, their files kept in {}",
                preview.talkgroup.name,
                preview.calls,
                preview
                    .holding_path
                    .as_deref()
                    .unwrap_or("(purge.holding_path is not set)")
            );
            let confirm = typed(confirm, "type the talkgroup's name to start: ")?;
            let (job, count) =
                scannerpulse::data::start_purge(&mut conn, id, &confirm, &by).await?;
            (job, format!("purge {count} calls"))
        }
    };
    println!("job {job} is started: {what}; `scannerpulse jobs show {job}` follows it");
    Ok(ExitCode::SUCCESS)
}

async fn traffic_command() -> CommandResult {
    let mut conn = connect().await?;
    let zone = settings::load(&mut conn).await?.time_zone;
    let report = scannerpulse::reports::traffic(&mut conn, zone, Utc::now()).await?;
    println!(
        "{} calls in the {} days before today ({} weekdays, {} weekend days), times in {}",
        report.calls, report.days, report.weekdays, report.weekend_days, report.time_zone
    );
    for (name, windows) in [
        ("weekdays", &report.quietest_weekday),
        ("weekends", &report.quietest_weekend),
    ] {
        println!("quietest hours on {name}:");
        for w in windows {
            println!(
                "  {} to {}: {:.1} calls a day",
                w.start, w.end, w.calls_per_day
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn status_command() -> CommandResult {
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is not set")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(CONNECT_TIMEOUT)
        .connect_with(scannerpulse::primary::options(&url).await?)
        .await?;
    let status = scannerpulse::status::read(&pool).await?;
    println!("{}", serde_json::to_string_pretty(&status)?);
    Ok(ExitCode::SUCCESS)
}

/// Who started a job from the command line.
fn command_line_user() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "someone".into());
    format!("{user} on the command line")
}

fn job_line(job: &jobs::Job) -> String {
    let progress = match job.total {
        Some(total) => format!("{} of {total}", job.done),
        None => job.done.to_string(),
    };
    format!(
        "{}\t{}\t{}\t{progress}\t{}\t{}",
        job.id, job.kind, job.state, job.started_by, job.message
    )
}

async fn jobs_command(command: Jobs) -> CommandResult {
    let mut conn = connect().await?;
    match command {
        Jobs::List => {
            for job in jobs::list(&mut conn, web::JOBS_LISTED).await? {
                println!("{}", job_line(&job));
            }
        }
        Jobs::Show(id) => {
            let job = jobs::get(&mut conn, id)
                .await?
                .ok_or(jobs::JobError::NoSuch(id))?;
            println!("{}", serde_json::to_string_pretty(&job)?);
        }
        Jobs::Stop(id) => match jobs::stop(&mut conn, id, &command_line_user()).await? {
            jobs::Stopped::Now => println!("job {id} is stopped"),
            jobs::Stopped::AfterStep => println!("job {id} stops after its current step"),
        },
        Jobs::Queue {
            talkgroup,
            from,
            to,
            yes,
        } => {
            let talkgroup_id = match &talkgroup {
                None => None,
                Some(text) => {
                    let (system, number) =
                        talkgroup_name(text).ok_or("write the talkgroup as SYSTEM/NUMBER")?;
                    let id: Option<i32> = sqlx::query_scalar(
                        "SELECT t.id FROM talkgroups t JOIN systems s ON s.id = t.system_id \
                         WHERE s.name = $1 AND t.number = $2",
                    )
                    .bind(system)
                    .bind(number)
                    .fetch_optional(&mut conn)
                    .await?;
                    Some(id.ok_or_else(|| format!("there is no talkgroup {text}"))?)
                }
            };
            let day = |text: &Option<String>| -> Result<Option<NaiveDate>, String> {
                text.as_deref()
                    .map(|t| {
                        NaiveDate::parse_from_str(t, "%Y-%m-%d")
                            .map_err(|_| format!("{t} is not a day such as 2031-01-15"))
                    })
                    .transpose()
            };
            let past = jobs::queue_plan(&mut conn, talkgroup_id, day(&from)?, day(&to)?).await?;
            let count = scannerpulse::queue::count_past(&mut conn, &past).await?;
            let which = talkgroup
                .as_deref()
                .unwrap_or("the talkgroups whose transcription is on");
            println!(
                "{count} calls of {which} were never tried and are at least {} ms long",
                past.min_ms
            );
            if count == 0 {
                return Ok(ExitCode::SUCCESS);
            }
            if !yes && !confirm(&format!("start a job to queue {count} calls? [y/N] "))? {
                println!("no job was started");
                return Ok(ExitCode::SUCCESS);
            }
            let (id, _) = jobs::start_queue(&mut conn, &past, &command_line_user()).await?;
            println!("job {id} is started; `scannerpulse jobs show {id}` follows it");
        }
        Jobs::AsrRun {
            set,
            server,
            model,
            prompt_file,
        } => {
            let prompt = match prompt_file {
                None => None,
                Some(file) => Some(
                    std::fs::read_to_string(&file)
                        .map_err(|error| format!("{file}: {error}"))?
                        .trim()
                        .to_string(),
                ),
            };
            let (id, count) = jobs::start_asr_run(
                &mut conn,
                &set,
                &server,
                &model,
                prompt,
                &command_line_user(),
            )
            .await?;
            println!(
                "job {id} is started: {count} labeled calls; `scannerpulse jobs show {id}` follows it"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

async fn viewers() -> CommandResult {
    let mut conn = connect().await?;
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT host, address, COALESCE(admin, '-'), \
                to_char(connected_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), \
                user_agent \
         FROM viewers WHERE seen_at >= now() - make_interval(secs => $1) ORDER BY connected_at",
    )
    .bind(f64::from(web::viewers::STALE_SECONDS))
    .fetch_all(&mut conn)
    .await?;
    for (host, address, admin, connected_at, user_agent) in rows {
        // The browser's description is the visitor's own text: control
        // characters are not printed.
        let user_agent: String = user_agent.chars().filter(|c| !c.is_control()).collect();
        println!("{host}\t{address}\t{admin}\t{connected_at}\t{user_agent}");
    }
    Ok(ExitCode::SUCCESS)
}

/// A new password, typed twice. From a terminal the typing is not shown;
/// otherwise the first two lines of the input are read.
fn new_password() -> Result<String, Box<dyn std::error::Error>> {
    let first = read_secret("new password: ")?;
    accounts::check_password(&first)?;
    let second = read_secret("again: ")?;
    if first != second {
        return Err("the two passwords differ".into());
    }
    Ok(first)
}

fn read_secret(prompt: &str) -> std::io::Result<String> {
    use std::io::IsTerminal;
    let terminal = std::io::stdin().is_terminal();
    let stty = |arg: &str| {
        std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .status()
    };
    if terminal {
        eprint!("{prompt}");
        std::io::stderr().flush()?;
        stty("-echo")?;
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if terminal {
        let _ = stty("echo");
        eprintln!();
    }
    read?;
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

async fn asr(command: Asr) -> CommandResult {
    let mut conn = connect().await?;
    if !settings::load_improvement(&mut conn).await?.enabled {
        return Err(
            "the improvement module is off: set improvement_module.enabled to true first".into(),
        );
    }
    match command {
        Asr::Sample {
            set,
            talkgroups,
            calls,
            days,
            min_seconds,
            seed,
            yes,
        } => {
            asr_sample(
                &mut conn,
                &set,
                &talkgroups,
                &calls,
                &days,
                min_seconds.as_deref(),
                seed.as_deref(),
                yes,
            )
            .await
        }
        Asr::Todo { set } => asr_todo(&mut conn, &set).await,
        Asr::Report { set } => asr_report(&mut conn, &set).await,
        Asr::Sets => {
            for set in scannerpulse::evaluation::sets(&mut conn).await? {
                println!(
                    "{}\t{} calls\t{} labeled\t{} runs",
                    set.set, set.size, set.labeled, set.runs
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Asr::Run {
            set,
            server,
            model,
            prompt_file,
        } => asr_run(&mut conn, &set, &server, &model, prompt_file.as_deref()).await,
    }
}

/// The talkgroups of a list file: one SYSTEM/NUMBER a line; empty lines and
/// lines that start with # are skipped, and a talkgroup listed twice counts once.
fn talkgroup_list(text: &str) -> Result<Vec<(String, i64)>, String> {
    let mut list: Vec<(String, i64)> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (system, number) = talkgroup_name(line)
            .ok_or_else(|| format!("line {}: {line:?} is not SYSTEM/NUMBER", n + 1))?;
        if !list.iter().any(|(s, t)| s == system && *t == number) {
            list.push((system.to_string(), number));
        }
    }
    if list.is_empty() {
        return Err("the file lists no talkgroup".into());
    }
    Ok(list)
}

#[allow(clippy::too_many_arguments)]
async fn asr_sample(
    conn: &mut PgConnection,
    set: &str,
    file: &str,
    calls: &str,
    days: &str,
    min_seconds: Option<&str>,
    seed: Option<&str>,
    yes: bool,
) -> CommandResult {
    let calls: usize = calls
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or("--calls is a whole number from 1 up")?;
    let days: u32 = days
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or("--days is a whole number from 1 up")?;
    let min_seconds: f64 = match min_seconds {
        None => 0.0,
        Some(text) => text
            .parse()
            .ok()
            .filter(|s: &f64| s.is_finite() && *s >= 0.0 && *s < 86_400.0)
            .ok_or("--min-seconds is a number of seconds from 0 up")?,
    };
    let seed: i64 = match seed {
        None => 1,
        Some(text) => text.parse().map_err(|_| "--seed is a whole number")?,
    };
    let text = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
    let talkgroups = talkgroup_list(&text).map_err(|why| format!("{file}: {why}"))?;
    let zone = settings::load(&mut *conn).await?.time_zone;
    // Never shorter than what transcription takes: a shorter call has no
    // machine output to score.
    let mut min_ms = (min_seconds * 1000.0).round() as i32;
    if let Some(transcription) = settings::load_transcription(&mut *conn).await? {
        min_ms = min_ms.max(transcription.min_duration_ms);
    }
    let size = evaluation::set_size(&mut *conn, set).await?;
    if size > 0
        && !yes
        && !confirm(&format!(
            "replace the {size} calls of the set {set}? [y/N] "
        ))?
    {
        println!("nothing was drawn");
        return Ok(ExitCode::SUCCESS);
    }
    let ask = Draw {
        set: set.to_string(),
        talkgroups,
        calls,
        days,
        min_ms,
        seed,
    };
    let drawn = evaluation::draw(conn, &ask, Utc::now(), zone).await?;
    for t in &drawn.talkgroups {
        let all = if t.drawn == t.eligible && t.drawn > 0 {
            " (all it had)"
        } else {
            ""
        };
        println!(
            "  {}/{}  {} of {} eligible{all}",
            t.system, t.number, t.drawn, t.eligible
        );
        if !t.transcribe {
            println!(
                "  note: transcription is off for {}/{}, so its new calls get no machine output",
                t.system, t.number
            );
        }
    }
    println!(
        "set {set}: {} calls from {} talkgroups, at least {min_ms} ms long, from the last {days} days (replaced {})",
        drawn.total(),
        drawn.talkgroups.iter().filter(|t| t.drawn > 0).count(),
        drawn.replaced
    );
    if drawn.total() < calls {
        println!(
            "only {} of the {calls} calls could be drawn: widen --days or lower --min-seconds",
            drawn.total()
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn asr_todo(conn: &mut PgConnection, set: &str) -> CommandResult {
    let zone = settings::load(&mut *conn).await?.time_zone;
    let size = evaluation::set_size(&mut *conn, set).await?;
    let list = evaluation::to_label(&mut *conn, set).await?;
    println!(
        "{} of the {size} calls of the set {set} have no person's text",
        list.len()
    );
    for call in &list {
        println!(
            "{}  {}  {}/{} {}  {:.1} s",
            call.call_id,
            call.started_at
                .with_timezone(&zone)
                .format("%Y-%m-%d %H:%M:%S"),
            call.system,
            call.number,
            call.talkgroup,
            f64::from(call.duration_ms.unwrap_or(0)) / 1000.0
        );
    }
    Ok(ExitCode::SUCCESS)
}

async fn asr_report(conn: &mut PgConnection, set: &str) -> CommandResult {
    let size = evaluation::set_size(&mut *conn, set).await?;
    if size == 0 {
        return Err(format!("there is no set {set}").into());
    }
    let labeled = accuracy::labeled(&mut *conn, set).await?;
    println!(
        "{} of the {size} calls of the set {set} have a person's text",
        labeled.len()
    );
    if labeled.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "production's first output: {}",
        accuracy::production(&labeled).summary()
    );
    for run in accuracy::runs(&mut *conn, set).await? {
        println!("{}", run_line(&run));
    }
    Ok(ExitCode::SUCCESS)
}

fn run_line(run: &Run) -> String {
    format!(
        "run of {}: {} at {}{}: WER {:.1}% on {} calls, {:.2} s per call",
        run.run_at.format("%Y-%m-%d %H:%M UTC"),
        run.model,
        run.server,
        if run.prompt.is_some() {
            " with a prompt"
        } else {
            ""
        },
        run.wer * 100.0,
        run.calls,
        run.seconds_per_call
    )
}

async fn asr_run(
    conn: &mut PgConnection,
    set: &str,
    server: &str,
    model: &str,
    prompt_file: Option<&str>,
) -> CommandResult {
    if !settings::server_url_ok(server) {
        return Err("--server must be an http:// or https:// URL without a user name, password, query or fragment; a key goes in SCANNERPULSE_TRANSCRIPTION_API_KEY".into());
    }
    let prompt = match prompt_file {
        None => None,
        Some(file) => {
            let text = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(text)
        }
    };
    let production = settings::load_transcription(&mut *conn).await?;
    let labeled = accuracy::labeled(&mut *conn, set).await?;
    if labeled.is_empty() {
        return Err(format!("no call of the set {set} has a person's text yet").into());
    }
    let candidate = Candidate {
        server,
        model,
        language: production.as_ref().and_then(|p| p.language.as_deref()),
        prompt: prompt.as_deref(),
        timeout: production
            .as_ref()
            .map_or(Duration::from_secs(120), |p| p.request_timeout),
    };
    let client = scannerpulse::asr::Client::new(
        std::env::var("SCANNERPULSE_TRANSCRIPTION_API_KEY")
            .ok()
            .filter(|key| !key.is_empty()),
    );
    let total = labeled.len();
    println!("{total} labeled calls of the set {set}, one at a time, to {server}");
    let tried = accuracy::try_candidate(&client, &labeled, &candidate, |n| {
        if n % 10 == 0 || n == total {
            println!("done {n} of {total}");
        }
    })
    .await;
    for (call, why) in &tried.skipped {
        println!("  call {call} skipped: {why}");
    }
    if let Some(why) = &tried.stopped {
        println!("stopped: {why}; nothing was kept");
        return Ok(ExitCode::FAILURE);
    }
    let (Some(wer), Some(seconds)) = (tried.candidate.wer(), tried.seconds_per_call()) else {
        println!("no call was scored; nothing was kept");
        return Ok(ExitCode::FAILURE);
    };
    println!("candidate: {}", tried.candidate.summary());
    println!("production, same calls: {}", tried.production.summary());
    println!("{seconds:.2} s per call");
    // The run may have outlasted the connection.
    *conn = connect().await?;
    // Production's speed is known from a kept run with its server and model.
    let earlier = accuracy::runs(&mut *conn, set).await?;
    let production_run = production.as_ref().and_then(|p| {
        earlier
            .iter()
            .rev()
            .find(|run| run.model == p.model && p.servers.iter().any(|s| s.url == run.server))
    });
    match production_run {
        Some(run) => println!(
            "production's seconds per call: {:.2} (run of {})",
            run.seconds_per_call,
            run.run_at.format("%Y-%m-%d %H:%M UTC")
        ),
        None => println!(
            "production's seconds per call: not measured; run asr run with production's server and model to measure it"
        ),
    }
    let run = Run {
        run_at: Utc::now(),
        server: server.to_string(),
        model: model.to_string(),
        prompt,
        calls: tried.candidate.calls as i32,
        wer: wer as f32,
        seconds_per_call: seconds as f32,
    };
    let id = accuracy::save_run(&mut *conn, set, &run).await?;
    println!("kept as run {id}");
    Ok(ExitCode::SUCCESS)
}

/// Asks a yes-or-no question on the terminal; anything but y or yes is no.
fn confirm(question: &str) -> std::io::Result<bool> {
    print!("{question}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

/// After a backup of `kind` succeeds: its time for the admin page, and its
/// push address, if the settings file has one. Neither fails the backup;
/// each says when it could not be done.
async fn backup_succeeded(kind: &str, database_url: &str) {
    use scannerpulse::log::warn;
    let not_kept = |why: String| {
        warn(format!(
            "backup: the {kind} backup's time was not kept: {why}"
        ))
    };
    match scannerpulse::primary::connect(database_url, CONNECT_TIMEOUT).await {
        Ok(mut conn) => {
            if let Err(error) = scannerpulse::backup::record(&mut conn, kind).await {
                not_kept(error.to_string());
            }
        }
        Err(why) => not_kept(why),
    }
    if let Some(var) = scannerpulse::backup::push_var(kind)
        && let Ok(url) = std::env::var(var)
        && let Err(error) = scannerpulse::backup::push(&url).await
    {
        warn(format!("backup: the {kind} push was not made: {error}"));
    }
}

/// Prints one JSON line per file. Exits 0 when every file was read.
fn read(zone: &str, files: &[String]) -> ExitCode {
    let Ok(zone) = zone.parse::<chrono_tz::Tz>() else {
        eprintln!("read: {zone:?} is not a known time zone");
        return ExitCode::from(2);
    };
    let mut all_read = true;
    for file in files {
        let line = match reader::read_path(Path::new(file), zone) {
            Ok((call, _)) => call_json(file, &call),
            Err(ReadError::Rejected(why)) => {
                all_read = false;
                json!({"file": file, "rejected": why})
            }
            Err(ReadError::Io(error)) => {
                all_read = false;
                json!({"file": file, "error": error.to_string()})
            }
        };
        println!("{line}");
    }
    if all_read {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn call_json(file: &str, call: &FileCall) -> serde_json::Value {
    let d = &call.described;
    json!({
        "file": file,
        "started_at": call.started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        "time_how": match call.time_how {
            TimeHow::Plain => "plain",
            TimeHow::ByMtime => "by_mtime",
            TimeHow::EarlierDefault => "earlier_default",
            TimeHow::Gap => "gap",
        },
        "duration_ms": call.duration_ms,
        "file_mtime": call.file_mtime.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        "system": d.system,
        "site": d.site,
        "talkgroup": d.talkgroup.as_ref().map(|t| t.number),
        "talkgroup_name": d.talkgroup.as_ref().map(|t| &t.name),
        "title": d.title,
        "radio_id": d.radio_id,
        "radio_alias": d.radio_alias,
    })
}

/// Versions already applied; empty when the database has never been updated.
async fn applied(conn: &mut PgConnection) -> sqlx::Result<Vec<i64>> {
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *conn)
        .await?;
    if !exists {
        return Ok(Vec::new());
    }
    sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
        .fetch_all(&mut *conn)
        .await
}

#[cfg(test)]
mod tests {
    use super::{
        Admin, Asr, Change, Command, Data, DbCommand, DbMove, Favorites, Jobs, Joined, NodeCommand,
        Settings, parse, talkgroup_list,
    };

    #[test]
    fn setup_takes_an_answers_file_and_a_dry_run_in_either_order() {
        assert_eq!(
            parse(&["setup"]),
            Some(Command::Setup {
                answers: None,
                dry_run: false
            })
        );
        assert_eq!(
            parse(&["setup", "--dry-run", "--answers", "a.env"]),
            Some(Command::Setup {
                answers: Some("a.env".into()),
                dry_run: true
            })
        );
        assert_eq!(parse(&["setup", "--answers"]), None);
        assert_eq!(parse(&["setup", "--dry-run", "--dry-run"]), None);
        assert_eq!(parse(&["setup", "now"]), None);
        assert_eq!(parse(&["backup"]), Some(Command::Backup));
        assert_eq!(parse(&["backup", "now"]), None);
    }

    #[test]
    fn the_second_command_adds_a_second_scannerpulse_machine() {
        assert_eq!(
            parse(&["second", "web2.example.org", "192.0.2.50/24"]),
            Some(Command::Change(Change::Second {
                name: "web2.example.org".into(),
                shared: Some("192.0.2.50/24".into()),
            }))
        );
        assert_eq!(
            parse(&["second", "web2.example.org"]),
            Some(Command::Change(Change::Second {
                name: "web2.example.org".into(),
                shared: None,
            }))
        );
    }

    #[test]
    fn the_tls_and_backups_commands_name_their_change() {
        assert_eq!(
            parse(&["tls", "install", "/root/site.pem", "/root/site.key"]),
            Some(Command::Change(Change::TlsInstall {
                cert: "/root/site.pem".into(),
                key: "/root/site.key".into(),
            }))
        );
        assert_eq!(
            parse(&["tls", "self"]),
            Some(Command::Change(Change::TlsSelf))
        );
        // The address carries a token: it is read from standard input, never
        // given as an argument (sudo logs arguments; ps shows them).
        assert!(matches!(
            parse(&["backups", "push", "dump"]),
            Some(Command::Change(Change::Push { .. }))
        ));
        assert!(matches!(
            parse(&["backups", "push", "dump", "off"]),
            Some(Command::Change(Change::Push { .. }))
        ));
        assert_eq!(
            parse(&[
                "backups",
                "push",
                "pgbackrest",
                "https://kuma.example.org/api/push/x"
            ]),
            None
        );
        assert_eq!(
            parse(&["backups", "location", "/srv/backups"]),
            Some(Command::Change(Change::Location("/srv/backups".into())))
        );
        assert_eq!(parse(&["tls", "install", "/root/site.pem"]), None);
        assert_eq!(
            parse(&[
                "tls",
                "letsencrypt",
                "web",
                "radio.example.org",
                "www.example.org"
            ]),
            Some(Command::Change(Change::LetsEncrypt {
                dns: false,
                names: vec!["radio.example.org".into(), "www.example.org".into()],
            }))
        );
        // The Cloudflare token is read like a push address, never an argument.
        assert_eq!(
            parse(&["tls", "letsencrypt", "dns-cloudflare", "radio.example.org"]),
            Some(Command::Change(Change::LetsEncrypt {
                dns: true,
                names: vec!["radio.example.org".into()],
            }))
        );
        assert_eq!(parse(&["tls", "letsencrypt", "web"]), None);
        assert_eq!(
            parse(&["tls", "letsencrypt", "http", "radio.example.org"]),
            None
        );
    }

    #[test]
    fn the_database_machines_commands_name_what_they_do() {
        assert_eq!(
            parse(&["join", "info", "db2", "192.0.2.22"]),
            Some(Command::Join(Joined::Info(vec![
                "db2".into(),
                "192.0.2.22".into()
            ])))
        );
        assert_eq!(
            parse(&["join", "done", "db2"]),
            Some(Command::Join(Joined::Done("db2".into())))
        );
        assert_eq!(parse(&["join", "info"]), None);
        assert_eq!(
            parse(&["node", "serve"]),
            Some(Command::Node(NodeCommand::Serve))
        );
        assert_eq!(
            parse(&["node", "status"]),
            Some(Command::Node(NodeCommand::Status))
        );
        assert_eq!(
            parse(&["node", "backup"]),
            Some(Command::Node(NodeCommand::Backup))
        );
        // The hub's key reaches a node only through `node serve`.
        assert_eq!(parse(&["node", "promote"]), None);
        assert_eq!(
            parse(&["db", "promote"]),
            Some(Command::Db(DbCommand::Promote))
        );
        assert_eq!(
            parse(&["db", "rejoin"]),
            Some(Command::Db(DbCommand::Rejoin))
        );
        assert_eq!(
            parse(&["db", "switchover"]),
            Some(Command::Db(DbCommand::Switchover))
        );
        assert_eq!(parse(&["db", "failover"]), None);
        assert_eq!(
            parse(&["db", "move", "db1", "db2", "--hub", "192.0.2.10"]),
            Some(Command::DbMove(DbMove::Begin {
                nodes: ["db1".into(), "db2".into()],
                hub: "192.0.2.10".into(),
                referee: None,
            }))
        );
        assert_eq!(
            parse(&["db", "move", "--switch"]),
            Some(Command::DbMove(DbMove::Switch))
        );
        assert_eq!(
            parse(&["db", "move", "--retire"]),
            Some(Command::DbMove(DbMove::Retire))
        );
        assert_eq!(parse(&["db", "move", "db1", "db2"]), None);
        assert_eq!(
            parse(&[
                "db",
                "move",
                "db1",
                "db2",
                "--hub",
                "192.0.2.10",
                "--referee",
                "ref"
            ]),
            Some(Command::DbMove(DbMove::Begin {
                nodes: ["db1".into(), "db2".into()],
                hub: "192.0.2.10".into(),
                referee: Some("ref".into()),
            }))
        );
    }

    #[test]
    fn update_checks_asks_or_goes_ahead() {
        assert_eq!(
            parse(&["update"]),
            Some(Command::Update {
                check: false,
                yes: false
            })
        );
        assert_eq!(
            parse(&["update", "--yes"]),
            Some(Command::Update {
                check: false,
                yes: true
            })
        );
        assert_eq!(
            parse(&["update", "--check"]),
            Some(Command::Update {
                check: true,
                yes: false
            })
        );
        assert_eq!(parse(&["update", "--check", "--yes"]), None);
        assert_eq!(parse(&["agent"]), Some(Command::Agent { install: false }));
        assert_eq!(
            parse(&["agent", "install"]),
            Some(Command::Agent { install: true })
        );
        assert_eq!(parse(&["agent", "now"]), None);
        assert_eq!(
            parse(&["archive", "move", "/srv/shared"]),
            Some(Command::ArchiveMove {
                new: std::path::PathBuf::from("/srv/shared"),
                switch: false
            })
        );
        assert_eq!(
            parse(&["archive", "move", "/srv/shared", "--switch"]),
            Some(Command::ArchiveMove {
                new: std::path::PathBuf::from("/srv/shared"),
                switch: true
            })
        );
        assert_eq!(parse(&["archive", "move"]), None);
        assert_eq!(parse(&["update", "now"]), None);
    }

    #[test]
    fn uninstall_names_each_part_to_remove_once() {
        use scannerpulse::setup::uninstall::Remove;
        assert_eq!(
            parse(&["uninstall"]),
            Some(Command::Uninstall(Remove::default()))
        );
        assert_eq!(
            parse(&["uninstall", "--remove-archive", "--remove-database"]),
            Some(Command::Uninstall(Remove {
                database: true,
                backups: false,
                archive: true
            }))
        );
        assert_eq!(
            parse(&["uninstall", "--remove-backups", "--remove-backups"]),
            None
        );
        assert_eq!(parse(&["uninstall", "--everything"]), None);
    }

    #[test]
    fn cert_make_takes_its_two_files_and_one_name_or_more() {
        assert_eq!(
            parse(&[
                "cert",
                "make",
                "--cert",
                "c.pem",
                "--key",
                "k.pem",
                "radio.example",
                "192.0.2.10"
            ]),
            Some(Command::CertMake {
                cert: "c.pem".into(),
                key: "k.pem".into(),
                names: vec!["radio.example".into(), "192.0.2.10".into()],
            })
        );
        assert_eq!(
            parse(&["cert", "make", "--cert", "c.pem", "--key", "k.pem"]),
            None
        );
        assert_eq!(parse(&["cert", "make", "radio.example"]), None);
    }

    #[test]
    fn db_update_runs_with_or_without_quiet() {
        assert_eq!(
            parse(&["db-update"]),
            Some(Command::DbUpdate { quiet: false })
        );
        assert_eq!(
            parse(&["db-update", "--quiet"]),
            Some(Command::DbUpdate { quiet: true })
        );
    }

    #[test]
    fn version_and_help_stand_alone() {
        assert_eq!(parse(&["--version"]), Some(Command::Version));
        assert_eq!(parse(&["--help"]), Some(Command::Help));
    }

    #[test]
    fn ingest_and_lease_take_no_argument() {
        assert_eq!(parse(&["ingest"]), Some(Command::Ingest));
        assert_eq!(parse(&["ingest", "--now"]), None);
        assert_eq!(parse(&["lease"]), Some(Command::Lease));
        assert_eq!(parse(&["lease", "--quiet"]), None);
        assert_eq!(parse(&["web"]), Some(Command::Web));
        assert_eq!(parse(&["web", "--now"]), None);
    }

    #[test]
    fn read_takes_a_zone_and_one_file_or_more() {
        assert_eq!(
            parse(&["read", "--zone", "UTC", "a.mp3", "b.mp3"]),
            Some(Command::Read {
                zone: "UTC".to_string(),
                files: vec!["a.mp3".to_string(), "b.mp3".to_string()],
            })
        );
        assert_eq!(parse(&["read", "--zone", "UTC"]), None);
        assert_eq!(parse(&["read", "a.mp3"]), None);
    }

    #[test]
    fn transcribe_takes_a_talkgroup_two_days_and_maybe_yes() {
        let args = [
            "transcribe",
            "--talkgroup",
            "SYSA/127",
            "--from",
            "2031-01-01",
            "--to",
            "2031-01-31",
        ];
        let want = |yes| Command::Transcribe {
            talkgroup: "SYSA/127".to_string(),
            from: "2031-01-01".to_string(),
            to: "2031-01-31".to_string(),
            yes,
        };
        assert_eq!(parse(&args), Some(want(false)));
        let mut with_yes = args.to_vec();
        with_yes.push("--yes");
        assert_eq!(parse(&with_yes), Some(want(true)));
        with_yes.push("--yes");
        assert_eq!(parse(&with_yes), None);
        assert_eq!(parse(&args[..5]), None);
    }

    #[test]
    fn asr_sample_takes_its_flags_in_any_order() {
        let want = |min: Option<&str>, seed: Option<&str>, yes| {
            Some(Command::Asr(Asr::Sample {
                set: "s1".into(),
                talkgroups: "list.txt".into(),
                calls: "400".into(),
                days: "180".into(),
                min_seconds: min.map(str::to_string),
                seed: seed.map(str::to_string),
                yes,
            }))
        };
        let base = [
            "asr",
            "sample",
            "--set",
            "s1",
            "--talkgroups",
            "list.txt",
            "--calls",
            "400",
            "--days",
            "180",
        ];
        assert_eq!(parse(&base), want(None, None, false));
        let mut all = base.to_vec();
        all.extend(["--yes", "--seed", "7", "--min-seconds", "5"]);
        assert_eq!(parse(&all), want(Some("5"), Some("7"), true));
        let reordered = [
            "asr",
            "sample",
            "--days",
            "180",
            "--calls",
            "400",
            "--talkgroups",
            "list.txt",
            "--set",
            "s1",
        ];
        assert_eq!(parse(&reordered), want(None, None, false));
        // A missing, repeated or unknown flag, or a flag without its value.
        assert_eq!(parse(&base[..8]), None);
        let mut twice = base.to_vec();
        twice.extend(["--set", "s2"]);
        assert_eq!(parse(&twice), None);
        let mut unknown = base.to_vec();
        unknown.push("--all");
        assert_eq!(parse(&unknown), None);
        let mut dangling = base.to_vec();
        dangling.push("--seed");
        assert_eq!(parse(&dangling), None);
        let mut flag_as_value = base.to_vec();
        flag_as_value.extend(["--seed", "--yes"]);
        assert_eq!(parse(&flag_as_value), None);
    }

    #[test]
    fn asr_todo_report_and_run_take_a_set() {
        assert_eq!(
            parse(&["asr", "todo", "--set", "s1"]),
            Some(Command::Asr(Asr::Todo { set: "s1".into() }))
        );
        assert_eq!(
            parse(&["asr", "report", "--set", "s1"]),
            Some(Command::Asr(Asr::Report { set: "s1".into() }))
        );
        assert_eq!(parse(&["asr", "report"]), None);
        assert_eq!(
            parse(&[
                "asr",
                "run",
                "--set",
                "s1",
                "--server",
                "http://asr-1/v1/audio/transcriptions",
                "--model",
                "model-y",
                "--prompt-file",
                "p.txt",
            ]),
            Some(Command::Asr(Asr::Run {
                set: "s1".into(),
                server: "http://asr-1/v1/audio/transcriptions".into(),
                model: "model-y".into(),
                prompt_file: Some("p.txt".into()),
            }))
        );
        assert_eq!(parse(&["asr", "run", "--set", "s1", "--model", "m"]), None);
        assert_eq!(parse(&["asr"]), None);
        assert_eq!(parse(&["asr", "draw", "--set", "s1"]), None);
    }

    #[test]
    fn a_talkgroup_list_skips_comments_and_repeats_and_refuses_a_bad_line() {
        let list = talkgroup_list("# dispatch\nSYSA/127\n\n  SYSB/9 \nSYSA/127\n").unwrap();
        assert_eq!(
            list,
            vec![("SYSA".to_string(), 127), ("SYSB".to_string(), 9)]
        );
        let bad = talkgroup_list("SYSA/127\nAlpha Fire\n").unwrap_err();
        assert!(bad.contains("line 2"), "{bad}");
        assert!(talkgroup_list("# nothing\n").is_err());
    }

    #[test]
    fn admin_takes_a_name_or_a_switch() {
        assert_eq!(
            parse(&["admin", "add", "ann"]),
            Some(Command::Admin(Admin::Add("ann".into())))
        );
        assert_eq!(
            parse(&["admin", "password-signin", "off"]),
            Some(Command::Admin(Admin::PasswordSignin(false)))
        );
        assert_eq!(parse(&["admin", "list"]), Some(Command::Admin(Admin::List)));
        assert_eq!(parse(&["admin", "add"]), None);
        assert_eq!(parse(&["admin", "add", "ann", "bob"]), None);
        assert_eq!(parse(&["admin", "password-signin", "yes"]), None);
        assert_eq!(parse(&["admin", "remove", "ann"]), None);
    }

    #[test]
    fn settings_take_a_key_and_a_value() {
        assert_eq!(
            parse(&["settings", "list"]),
            Some(Command::Settings(Settings::List))
        );
        assert_eq!(
            parse(&["settings", "set", "time_zone", "\"UTC\""]),
            Some(Command::Settings(Settings::Set(
                "time_zone".into(),
                "\"UTC\"".into()
            )))
        );
        assert_eq!(
            parse(&["settings", "unset", "time_zone"]),
            Some(Command::Settings(Settings::Unset("time_zone".into())))
        );
        assert_eq!(parse(&["settings", "set", "time_zone"]), None);
        assert_eq!(parse(&["settings"]), None);
    }

    #[test]
    fn favorites_take_a_name_and_talkgroups() {
        assert_eq!(
            parse(&["favorites", "set", "Fire", "SYSA/1", "SYSA/2"]),
            Some(Command::Favorites(Favorites::Set(
                "Fire".into(),
                vec!["SYSA/1".into(), "SYSA/2".into()]
            )))
        );
        assert_eq!(parse(&["favorites", "set", "Fire"]), None);
        assert_eq!(
            parse(&["favorites", "remove", "Fire"]),
            Some(Command::Favorites(Favorites::Remove("Fire".into())))
        );
        assert_eq!(parse(&["viewers"]), Some(Command::Viewers));
        assert_eq!(parse(&["viewers", "all"]), None);
    }

    #[test]
    fn jobs_take_an_id_or_their_flags() {
        assert_eq!(parse(&["jobs", "list"]), Some(Command::Jobs(Jobs::List)));
        assert_eq!(
            parse(&["jobs", "stop", "7"]),
            Some(Command::Jobs(Jobs::Stop(7)))
        );
        assert_eq!(parse(&["jobs", "stop", "seven"]), None);
        assert_eq!(
            parse(&["jobs", "queue", "--yes"]),
            Some(Command::Jobs(Jobs::Queue {
                talkgroup: None,
                from: None,
                to: None,
                yes: true
            }))
        );
        assert_eq!(
            parse(&[
                "jobs",
                "queue",
                "--talkgroup",
                "SYSA/7",
                "--from",
                "2031-01-01"
            ]),
            Some(Command::Jobs(Jobs::Queue {
                talkgroup: Some("SYSA/7".into()),
                from: Some("2031-01-01".into()),
                to: None,
                yes: false
            }))
        );
        assert_eq!(
            parse(&["jobs", "asr-run", "--set", "s1", "--server", "u"]),
            None
        );
        assert_eq!(parse(&["jobs", "queue", "--day", "x"]), None);
    }

    #[test]
    fn the_transcription_commands_take_a_talkgroup_and_a_switch() {
        assert_eq!(
            parse(&["talkgroup", "transcribe", "SYSA/7", "on"]),
            Some(Command::TalkgroupSwitch {
                talkgroup: "SYSA/7".into(),
                on: true
            })
        );
        assert_eq!(parse(&["talkgroup", "transcribe", "SYSA/7", "yes"]), None);
        assert_eq!(
            parse(&["queue", "retry-failed"]),
            Some(Command::RetryFailed)
        );
        assert_eq!(parse(&["asr", "sets"]), Some(Command::Asr(Asr::Sets)));
    }

    #[test]
    fn the_data_commands_are_read() {
        assert_eq!(
            parse(&["talkgroup", "rename", "SYSA/7", "Alpha / Fire"]),
            Some(Command::TalkgroupRename {
                talkgroup: "SYSA/7".into(),
                name: "Alpha / Fire".into()
            })
        );
        assert_eq!(parse(&["talkgroup", "rename", "SYSA/7"]), None);
        assert_eq!(
            parse(&["talkgroup", "correct", "SYSA/7", "--confirm", "Alpha"]),
            Some(Command::Data(Data::Correct {
                talkgroup: "SYSA/7".into(),
                confirm: Some("Alpha".into())
            }))
        );
        assert_eq!(
            parse(&["talkgroup", "correct", "SYSA/7"]),
            Some(Command::Data(Data::Correct {
                talkgroup: "SYSA/7".into(),
                confirm: None
            }))
        );
        assert_eq!(parse(&["talkgroup", "correct", "SYSA/7", "--yes"]), None);
        assert_eq!(
            parse(&["talkgroup", "merge", "SYSA/7", "--into", "SYSA/8"]),
            Some(Command::Data(Data::Merge {
                talkgroup: "SYSA/7".into(),
                into: "SYSA/8".into(),
                confirm: None
            }))
        );
        assert_eq!(parse(&["talkgroup", "merge", "SYSA/7"]), None);
        assert_eq!(
            parse(&["talkgroup", "purge", "SYSA/7", "--confirm", "Group 7"]),
            Some(Command::Data(Data::Purge {
                talkgroup: "SYSA/7".into(),
                confirm: Some("Group 7".into())
            }))
        );
        assert_eq!(
            parse(&["talkgroups", "uncorrected"]),
            Some(Command::Uncorrected)
        );
    }

    #[test]
    fn anything_else_is_refused() {
        let refused: [&[&str]; 9] = [
            &[],
            &["nonsense"],
            &["db-update", "--quite"],
            &["db-update", "--help"],
            &["db-update", "--quiet", "--quiet"],
            &["db-update", "extra"],
            &["--quiet"],
            &["--version", "db-update"],
            &["--help", "--version"],
        ];
        for args in refused {
            assert_eq!(parse(args), None, "{args:?}");
        }
    }
}
