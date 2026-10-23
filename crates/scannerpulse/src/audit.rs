//! The audit log: one entry for each change an admin made, or a command run
//! on a machine that changed something, naming who. Each entry is also a
//! line in the journal, so it reaches a syslog server when one is set. The
//! database refuses to change or delete an entry; it is kept with no time
//! limit. It is read in the terminal (`scannerpulse audit`), not on the
//! admin page.

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
    crate::log::info(format!("audit: {actor}{from}: {what}"));
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

/// Records a command's change, run on this machine: its entry, then its
/// line in this machine's journal through the system's log socket (a
/// command's own error output is the terminal).
pub async fn record_command(db: impl sqlx::PgExecutor<'_>, what: &str) -> sqlx::Result<()> {
    let actor = command_actor();
    keep(db, &actor, None, what).await?;
    if let Ok(socket) = std::os::unix::net::UnixDatagram::unbound() {
        // user.info, as syslog(3) would send it.
        let line = format!("<14>scannerpulse: audit: {actor}: {what}");
        let _ = socket.send_to(line.as_bytes(), "/dev/log");
    }
    Ok(())
}

/// Who runs a command on this machine: "<account> on <machine>".
pub fn command_actor() -> String {
    let user = std::env::var("USER").unwrap_or_else(|_| "someone".into());
    let host = std::env::var("SCANNERPULSE_HOST")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/proc/sys/kernel/hostname")
                .ok()
                .map(|name| name.trim().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this machine".into());
    format!("{user} on {host}")
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
    format!("{at}  {}{from}: {}", entry.actor, entry.what)
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
    fn a_day_starts_at_its_midnight_in_the_sites_zone() {
        let zone: Tz = "America/Chicago".parse().unwrap();
        let day = NaiveDate::from_ymd_opt(2026, 10, 23).unwrap();
        assert_eq!(
            day_start(day, zone),
            "2026-10-23T05:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
}
