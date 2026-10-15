//! `/api/calls`: the list of calls and the archive search.

use std::time::Duration;

use axum::Json;
use axum::extract::{Query, State};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ApiError, AppState};
use crate::listing::{self, Cursor, Filter, PAGE_SIZES, Row};

/// A count stops here unless the request asks for the exact count.
pub const COUNT_CAP: i64 = 10_000;
/// The longest one read of the list or its count may take.
pub const READ_LIMIT: Duration = Duration::from_secs(30);
/// Exact counts that may run at once per host; another gets the capped count.
pub const EXACT_COUNTS: usize = 2;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    /// Talkgroup ids, separated by commas.
    pub talkgroups: Option<String>,
    pub system: Option<i32>,
    /// RFC 3339 instants.
    pub from: Option<String>,
    pub until: Option<String>,
    pub q: Option<String>,
    /// `newest` (the default) or `oldest`.
    pub order: Option<String>,
    pub after: Option<String>,
    pub size: Option<i64>,
    pub smart: bool,
    /// `false` skips the count, for a page turn that already has it.
    pub count: Option<bool>,
    pub exact: bool,
}

#[derive(Debug, Serialize)]
pub struct ListAnswer {
    pub calls: Vec<Row>,
    pub next: Option<String>,
    pub total: Option<i64>,
    /// True when `total` stopped at the cap: there are more.
    pub total_capped: bool,
}

/// The filter a request asks for.
pub fn filter(params: &ListParams) -> Result<Filter, ApiError> {
    let talkgroups = match params.talkgroups.as_deref() {
        None | Some("") => Vec::new(),
        Some(list) => list
            .split(',')
            .map(|id| id.trim().parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ApiError::BadRequest("talkgroups is a list of ids".into()))?,
    };
    let instant = |name: &str, text: &Option<String>| -> Result<Option<DateTime<Utc>>, ApiError> {
        match text.as_deref() {
            None | Some("") => Ok(None),
            Some(text) => DateTime::parse_from_rfc3339(text)
                .map(|t| Some(t.with_timezone(&Utc)))
                .map_err(|_| ApiError::BadRequest(format!("{name} is not an RFC 3339 instant"))),
        }
    };
    Ok(Filter {
        talkgroups,
        system: params.system,
        from: instant("from", &params.from)?,
        until: instant("until", &params.until)?,
        text: params.q.clone().filter(|q| !q.trim().is_empty()),
    })
}

pub async fn list(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<ListAnswer>, ApiError> {
    let filter = filter(&params)?;
    let oldest_first = match params.order.as_deref() {
        None | Some("newest") => false,
        Some("oldest") => true,
        Some(_) => return Err(ApiError::BadRequest("order is newest or oldest".into())),
    };
    let size = params.size.unwrap_or(100);
    if !PAGE_SIZES.contains(&size) {
        return Err(ApiError::BadRequest(format!(
            "size is one of {PAGE_SIZES:?}"
        )));
    }
    let after = match params.after.as_deref() {
        None | Some("") => None,
        Some(text) => Some(
            Cursor::decode(text)
                .ok_or_else(|| ApiError::BadRequest("after is not a cursor".into()))?,
        ),
    };
    let mut conn = state.pool.acquire().await?;
    let mut page = listing::page(&mut conn, &filter, oldest_first, after, size, READ_LIMIT).await?;
    if params.smart {
        crate::smartsort::smart_sort(&mut page.rows);
    }
    let (total, total_capped) = if params.count == Some(false) {
        (None, false)
    } else {
        // An exact count waits for nothing: when two run already, this one
        // stops at the cap and says so.
        let permit = if params.exact {
            state.exact_counts.clone().try_acquire_owned().ok()
        } else {
            None
        };
        let cap = if permit.is_some() {
            None
        } else {
            Some(COUNT_CAP)
        };
        let (n, capped) = listing::count(&mut conn, &filter, cap, READ_LIMIT).await?;
        (Some(n), capped)
    };
    Ok(Json(ListAnswer {
        calls: page.rows,
        next: page.next.map(|c| c.encode()),
        total,
        total_capped,
    }))
}
