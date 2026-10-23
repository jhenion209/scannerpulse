//! The audit log: one entry for each change an admin made, or a command run
//! on a machine that changed something, naming who. Each entry is also a
//! line in the journal, so it reaches a syslog server when one is set. The
//! database refuses to change or delete an entry; it is kept with no time
//! limit. It is read in the terminal (`scannerpulse audit`), not on the
//! admin page.

use std::path::Path;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Serialize;

/// Keeps one entry, in the caller's transaction when it has one.
pub async fn keep(
    db: impl sqlx::PgExecutor<'_>,
    actor: &str,
    address: Option<&str>,
    what: &str,
) -> sqlx::Result<()> {
    sqlx::query("INSERT INTO audit_log (actor, address, what) VALUES ($1, $2, $3)")
        .bind(actor)
        .bind(address)
        .bind(what)
        .execute(db)
        .await?;
    Ok(())
}

/// The entry's line in the journal.
pub fn journal(actor: &str, address: Option<&str>, what: &str) {
    let from = address.map(|a| format!(" from {a}")).unwrap_or_default();
    crate::log::info(shown(&format!("audit: {actor}{from}: {what}")));
}

/// Text as it is shown, in a terminal or the journal: a control character,
/// which an admin may have typed into what an entry quotes, written out
/// (`\u{1b}`), never sent as itself. The entry keeps the text as it was.
fn shown(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Records a change already made: its line in the journal, then its entry.
/// A database that does not take the entry fails nothing; the journal says
/// the entry was lost.
pub async fn record(db: impl sqlx::PgExecutor<'_>, actor: &str, address: Option<&str>, what: &str) {
    journal(actor, address, what);
    if let Err(error) = keep(db, actor, address, what).await {
        crate::log::error(format!("audit: the entry above was not kept: {error}"));
    }
}

/// The system's log socket: a command's lines reach this machine's journal
/// through it (a command's own error output is the terminal).
pub const LOG_SOCKET: &str = "/dev/log";

/// Records a command's change, run on this machine: its line in this
/// machine's journal through the log socket `socket`, then its entry in `db`
/// (the database, or why it did not answer). A lost entry is a line in the
/// journal too, as `record` writes one.
pub async fn record_command(
    socket: &Path,
    db: Result<&mut sqlx::PgConnection, String>,
    what: &str,
) -> Result<(), String> {
    let actor = command_actor();
    let send = |priority: u8, line: String| {
        if let Ok(log) = std::os::unix::net::UnixDatagram::unbound() {
            let line = format!("<{priority}>scannerpulse: {}", shown(&line));
            let _ = log.send_to(line.as_bytes(), socket);
        }
    };
    // user.info and user.err, as syslog(3) would send them.
    send(14, format!("audit: {actor}: {what}"));
    let kept = match db {
        Ok(conn) => keep(conn, &actor, None, what)
            .await
            .map_err(|error| error.to_string()),
        Err(why) => Err(why),
    };
    if let Err(why) = &kept {
        send(11, format!("audit: the entry above was not kept: {why}"));
    }
    kept
}

/// Who runs a command on this machine: "<account> on <machine>".
pub fn command_actor() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "someone".into());
    format!("{user} on {}", this_machine())
}

/// This machine's name, as the site knows it.
pub fn this_machine() -> String {
    std::env::var("SCANNERPULSE_HOST")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this machine".into())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Entry {
    pub id: i64,
    pub at: DateTime<Utc>,
    pub actor: String,
    pub address: Option<String>,
    pub what: String,
}

/// The entries after the entry `after` (0 for all), at or after `since`, of
/// `actor` (an admin's login, or an account on any machine), oldest first,
/// at most `limit`.
pub async fn list(
    db: impl sqlx::PgExecutor<'_>,
    after: i64,
    since: Option<DateTime<Utc>>,
    actor: Option<&str>,
    limit: i64,
) -> sqlx::Result<Vec<Entry>> {
    sqlx::query_as(
        "SELECT id, at, actor, address, what FROM audit_log \
         WHERE id > $1 AND ($2::timestamptz IS NULL OR at >= $2) \
           AND ($3::text IS NULL OR actor = $3 OR left(actor, length($3) + 4) = $3 || ' on ') \
         ORDER BY id LIMIT $4",
    )
    .bind(after)
    .bind(since)
    .bind(actor)
    .bind(limit)
    .fetch_all(db)
    .await
}

/// The start of `day` in the site's zone.
pub fn day_start(day: NaiveDate, zone: Tz) -> DateTime<Utc> {
    let noon = day.and_hms_opt(12, 0, 0).expect("noon is a time");
    let at = zone
        .from_local_datetime(&noon)
        .earliest()
        .map_or_else(|| noon.and_utc(), |at| at.with_timezone(&Utc));
    crate::recorders::start_of_day(at, zone)
}

/// An entry as `scannerpulse audit` prints it, its time in the site's zone.
pub fn line(entry: &Entry, zone: Tz) -> String {
    let at = entry.at.with_timezone(&zone).format("%Y-%m-%d %H:%M:%S");
    let from = entry
        .address
        .as_deref()
        .map(|a| format!(" from {a}"))
        .unwrap_or_default();
    shown(&format!("{at}  {}{from}: {}", entry.actor, entry.what))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_names_the_time_in_the_sites_zone_who_and_from_where() {
        let entry = Entry {
            id: 1,
            at: "2026-10-23T19:05:12Z".parse().unwrap(),
            actor: "ann".into(),
            address: Some("198.51.100.7".into()),
            what: "set web.system_order to [\"B\",\"A\"]".into(),
        };
        let zone: Tz = "America/Chicago".parse().unwrap();
        assert_eq!(
            line(&entry, zone),
            "2026-10-23 14:05:12  ann from 198.51.100.7: set web.system_order to [\"B\",\"A\"]"
        );
        let command = Entry {
            actor: "root on sp-a".into(),
            address: None,
            ..entry
        };
        assert!(line(&command, zone).starts_with("2026-10-23 14:05:12  root on sp-a: "));
    }

    #[test]
    fn a_control_character_is_shown_escaped_never_sent_to_the_terminal() {
        // A model name an associate typed, holding a terminal's sequences.
        let entry = Entry {
            id: 1,
            at: "2026-10-23T19:05:12Z".parse().unwrap(),
            actor: "cy".into(),
            address: Some("198.51.100.7".into()),
            what: "started an evaluation run of x\r\u{1b}[2K\u{1b}]52;c;aGk=\u{7}\nroot: y".into(),
        };
        let zone: Tz = "America/Chicago".parse().unwrap();
        let shown = line(&entry, zone);
        assert!(!shown.chars().any(char::is_control), "{shown:?}");
        assert!(
            shown.ends_with(r"run of x\r\u{1b}[2K\u{1b}]52;c;aGk=\u{7}\nroot: y"),
            "{shown:?}"
        );
    }

    #[tokio::test]
    async fn a_commands_line_reaches_the_journal_first_and_a_lost_entry_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let journal = std::os::unix::net::UnixDatagram::bind(&path).unwrap();
        journal
            .set_read_timeout(Some(std::time::Duration::from_secs(1)))
            .unwrap();
        let kept = record_command(
            &path,
            Err("the database does not answer".into()),
            "ran `scannerpulse settings set a b`",
        )
        .await;
        assert_eq!(kept, Err("the database does not answer".into()));
        let mut buffer = [0u8; 512];
        let mut next = || {
            let n = journal.recv(&mut buffer).expect("a line in the journal");
            String::from_utf8_lossy(&buffer[..n]).into_owned()
        };
        let first = next();
        assert!(first.starts_with("<14>scannerpulse: audit: "), "{first}");
        assert!(
            first.ends_with(": ran `scannerpulse settings set a b`"),
            "{first}"
        );
        assert_eq!(
            next(),
            "<11>scannerpulse: audit: the entry above was not kept: the database does not answer"
        );
    }

    #[test]
    fn a_day_starts_at_its_midnight_in_the_sites_zone() {
        let zone: Tz = "America/Chicago".parse().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        assert_eq!(
            day_start(day, zone),
            "2026-10-23T05:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
}
