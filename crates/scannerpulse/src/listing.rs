//! The list of calls and the archive search: one query with filters, read a
//! page at a time by a keyset cursor, newest or oldest first.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgConnection, Postgres, QueryBuilder};

use crate::smartsort::Sortable;

/// The page sizes a request may ask for.
pub const PAGE_SIZES: [i64; 6] = [10, 25, 50, 100, 250, 500];

/// What a list shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Talkgroup ids; empty means every talkgroup.
    pub talkgroups: Vec<i32>,
    pub system: Option<i32>,
    /// From this instant, included.
    pub from: Option<DateTime<Utc>>,
    /// Up to this instant, excluded.
    pub until: Option<DateTime<Utc>>,
    /// The search box's text (see `search`).
    pub text: Option<String>,
}

/// Where a page starts: the last call of the previous page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub started_at: DateTime<Utc>,
    pub id: i64,
}

impl Cursor {
    /// `<microseconds since 1970>.<id>`.
    pub fn encode(&self) -> String {
        format!("{}.{}", self.started_at.timestamp_micros(), self.id)
    }

    pub fn decode(text: &str) -> Option<Cursor> {
        let (micros, id) = text.split_once('.')?;
        Some(Cursor {
            started_at: DateTime::from_timestamp_micros(micros.parse().ok()?)?,
            id: id.parse().ok()?,
        })
    }
}

/// One call of a list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Row {
    pub id: i64,
    pub started_at: DateTime<Utc>,
    pub duration_ms: Option<i32>,
    pub talkgroup_id: Option<i32>,
    pub talkgroup: Option<i64>,
    pub talkgroup_name: Option<String>,
    pub system: Option<String>,
    pub site: Option<String>,
    pub radio_id: Option<i64>,
    pub radio_alias: Option<String>,
    pub transcript: Option<String>,
    /// `machine` or `human`.
    pub origin: Option<String>,
}

impl Sortable for Row {
    fn talkgroup(&self) -> Option<i64> {
        self.talkgroup_id.map(i64::from)
    }
    fn start_seconds(&self) -> f64 {
        self.started_at.timestamp_micros() as f64 / 1e6
    }
    fn length_seconds(&self) -> f64 {
        f64::from(self.duration_ms.unwrap_or(0)) / 1000.0
    }
}

/// A page and the cursor of the next one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub rows: Vec<Row>,
    pub next: Option<Cursor>,
}

/// `None` when the filter's text holds nothing to search for: the list is
/// then the one without text.
fn text_query(filter: &Filter) -> Option<String> {
    filter.text.as_deref().and_then(crate::search::tsquery)
}

/// The FROM and WHERE that both the page and the count use.
fn push_filter<'a>(q: &mut QueryBuilder<'a, Postgres>, filter: &'a Filter) {
    q.push(
        " FROM calls c \
         LEFT JOIN talkgroups t ON t.id = c.talkgroup_id \
         LEFT JOIN systems s ON s.id = t.system_id \
         LEFT JOIN sites si ON si.id = c.site_id \
         LEFT JOIN transcripts tr ON tr.call_id = c.id AND tr.started_at = c.started_at \
         WHERE true",
    );
    if !filter.talkgroups.is_empty() {
        q.push(" AND c.talkgroup_id = ANY(")
            .push_bind(&filter.talkgroups)
            .push(")");
    }
    if let Some(system) = filter.system {
        q.push(" AND t.system_id = ").push_bind(system);
    }
    if let Some(from) = filter.from {
        q.push(" AND c.started_at >= ").push_bind(from);
    }
    if let Some(until) = filter.until {
        q.push(" AND c.started_at < ").push_bind(until);
    }
    if let Some(query) = text_query(filter) {
        q.push(" AND tr.search @@ to_tsquery('simple', ")
            .push_bind(query)
            .push(")");
    }
}

/// One page of the list, read under the time limit `limit`.
pub async fn page(
    conn: &mut PgConnection,
    filter: &Filter,
    oldest_first: bool,
    after: Option<Cursor>,
    size: i64,
    limit: std::time::Duration,
) -> sqlx::Result<Page> {
    let (op, dir) = if oldest_first {
        (">", "ASC")
    } else {
        ("<", "DESC")
    };
    let build = || {
        let mut q = QueryBuilder::new(
            "SELECT c.id, c.started_at, c.duration_ms, c.talkgroup_id, t.number AS talkgroup, \
                    t.name AS talkgroup_name, s.name AS system, si.name AS site, c.radio_id, \
                    c.radio_alias, tr.text AS transcript, tr.origin",
        );
        push_filter(&mut q, filter);
        if let Some(after) = after {
            q.push(format!(" AND (c.started_at, c.id) {op} ("))
                .push_bind(after.started_at)
                .push(", ")
                .push_bind(after.id)
                .push(")");
        }
        q.push(format!(" ORDER BY c.started_at {dir}, c.id {dir} LIMIT "))
            .push_bind(size + 1);
        q
    };
    let mut rows: Vec<Row> = Vec::new();
    for attempt in 0..2 {
        let mut tx = begin(conn, limit).await?;
        let mut q = build();
        match q.build_query_as().fetch_all(&mut *tx).await {
            Ok(found) => {
                tx.commit().await?;
                rows = found;
                break;
            }
            Err(error) if attempt == 0 && is_deadlock(&error) => continue,
            Err(error) => return Err(error),
        }
    }
    let next = if rows.len() as i64 > size {
        rows.truncate(size as usize);
        rows.last().map(|r| Cursor {
            started_at: r.started_at,
            id: r.id,
        })
    } else {
        None
    };
    Ok(Page { rows, next })
}

/// How many calls the filter matches, up to `cap` (`None`: all), read under
/// the time limit `limit`. Returns the count and whether it stopped at the
/// cap.
pub async fn count(
    conn: &mut PgConnection,
    filter: &Filter,
    cap: Option<i64>,
    limit: std::time::Duration,
) -> sqlx::Result<(i64, bool)> {
    let build = || {
        let mut q = QueryBuilder::new("SELECT count(*) FROM (SELECT 1");
        push_filter(&mut q, filter);
        if let Some(cap) = cap {
            q.push(" LIMIT ").push_bind(cap + 1);
        }
        q.push(") matched");
        q
    };
    let mut n = 0i64;
    for attempt in 0..2 {
        let mut tx = begin(conn, limit).await?;
        let mut q = build();
        match q.build_query_scalar().fetch_one(&mut *tx).await {
            Ok(found) => {
                tx.commit().await?;
                n = found;
                break;
            }
            Err(error) if attempt == 0 && is_deadlock(&error) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(match cap {
        Some(cap) if n > cap => (cap, true),
        _ => (n, false),
    })
}

/// A transaction for one read, with a statement time limit: the list is
/// public, and any filter can be slow on a large archive. A read is tried
/// once more when it was a deadlock's victim: ingest
/// can create a month's partitions while it reads.
async fn begin(
    conn: &mut PgConnection,
    limit: std::time::Duration,
) -> sqlx::Result<sqlx::Transaction<'_, Postgres>> {
    use sqlx::Connection;
    let mut tx = conn.begin().await?;
    sqlx::query("SELECT set_config('statement_timeout', $1, true)")
        .bind(format!("{}ms", limit.as_millis()))
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

pub(crate) fn is_deadlock(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|code| code == "40P01")
}

#[cfg(test)]
mod tests {
    use super::Cursor;
    use chrono::{TimeZone, Utc};

    #[test]
    fn a_cursor_reads_back_what_it_wrote_and_refuses_garbage() {
        let c = Cursor {
            started_at: Utc.timestamp_micros(1_900_000_000_123_456).unwrap(),
            id: 42,
        };
        assert_eq!(Cursor::decode(&c.encode()), Some(c));
        for bad in ["", "1", "x.1", "1.x", "1.2.3", "99999999999999999999.1"] {
            assert_eq!(Cursor::decode(bad), None, "{bad}");
        }
    }
}
