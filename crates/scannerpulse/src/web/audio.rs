//! A call's audio, and many calls joined into one file.

use std::path::PathBuf;

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use tower::ServiceExt;

use super::calls::{ListParams, READ_LIMIT};
use super::{ApiError, AppState};
use crate::listing::{self, Filter};

/// The most calls one export joins.
pub const MAX_EXPORT_CALLS: usize = 200;
/// The most audio, in bytes, one export reads: the join is held in memory,
/// so the exports at once hold at most this much each.
pub const MAX_EXPORT_BYTES: u64 = 256 << 20;

/// Where a call's file is.
async fn file_of(state: &AppState, id: i64) -> Result<PathBuf, ApiError> {
    let found: Option<(String, String)> = sqlx::query_as(
        "SELECT l.root, c.rel_path FROM calls c JOIN locations l ON l.id = c.location_id \
         WHERE c.id = $1",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let (root, rel_path) = found.ok_or(ApiError::NotFound)?;
    Ok(PathBuf::from(root).join(rel_path))
}

/// `/audio/<id>`: the call's file as it is, with byte ranges so a player can
/// seek.
pub async fn one(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    request: Request,
) -> Result<Response, ApiError> {
    let path = file_of(&state, id).await?;
    // The type comes from the extension: every call's file is an .mp3.
    let served = tower_http::services::ServeFile::new(path)
        .oneshot(request)
        .await
        .unwrap_or_else(|never| match never {});
    if served.status() == StatusCode::NOT_FOUND {
        return Err(ApiError::NotFound);
    }
    Ok(served.map(Body::new).into_response())
}

/// What an export joins: the calls of a list of ids, or of a filter.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExportAsk {
    pub ids: Option<Vec<i64>>,
    #[serde(flatten)]
    pub filter: ListParams,
}

/// One call to join.
#[derive(Debug, sqlx::FromRow)]
struct Part {
    started_at: DateTime<Utc>,
    root: String,
    rel_path: String,
    talkgroup: Option<i64>,
    system: Option<String>,
}

/// `POST /api/export/audio`: the calls, oldest first, joined into one mp3.
/// A list of ids ignores the filter. A call whose file cannot be read is left
/// out, and the `X-Export-Missing` header counts them.
pub async fn export(
    State(state): State<AppState>,
    Json(ask): Json<ExportAsk>,
) -> Result<Response, ApiError> {
    let permit = state.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::Limit(
            StatusCode::TOO_MANY_REQUESTS,
            "two exports are running; try again shortly".into(),
        )
    })?;
    let filter = super::calls::filter(&ask.filter)?;
    let parts = match &ask.ids {
        Some(ids) => by_ids(&state, ids).await?,
        None => by_filter(&state, &filter).await?,
    };
    let zone = time_zone(&state).await?;
    let name = export_name(&parts, filter.from, filter.until, zone, "mp3");
    let files: Vec<PathBuf> = parts
        .iter()
        .map(|p| PathBuf::from(&p.root).join(&p.rel_path))
        .collect();
    let (joined, missing) = tokio::task::spawn_blocking(move || {
        // The job keeps the permit: it runs on when the client goes away.
        let _permit = permit;
        join(&files)
    })
    .await
    .map_err(|error| ApiError::Internal(format!("the audio export failed: {error}")))??;
    let mut response = Response::new(Body::from(joined));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
    headers.insert(header::CONTENT_DISPOSITION, attachment(&name));
    headers.insert("x-export-missing", HeaderValue::from(missing));
    Ok(response)
}

async fn by_ids(state: &AppState, ids: &[i64]) -> Result<Vec<Part>, ApiError> {
    if ids.is_empty() {
        return Err(ApiError::BadRequest("choose at least one call".into()));
    }
    if ids.len() > MAX_EXPORT_CALLS {
        return Err(ApiError::Limit(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "{} calls were chosen; one export joins at most {MAX_EXPORT_CALLS}",
                ids.len()
            ),
        ));
    }
    Ok(sqlx::query_as(
        "SELECT c.started_at, l.root, c.rel_path, t.number AS talkgroup, s.name AS system \
         FROM calls c JOIN locations l ON l.id = c.location_id \
         LEFT JOIN talkgroups t ON t.id = c.talkgroup_id LEFT JOIN systems s ON s.id = t.system_id \
         WHERE c.id = ANY($1) ORDER BY c.started_at, c.id",
    )
    .bind(ids)
    .fetch_all(&state.pool)
    .await?)
}

async fn by_filter(state: &AppState, filter: &Filter) -> Result<Vec<Part>, ApiError> {
    let mut conn = state.pool.acquire().await?;
    let (n, capped) =
        listing::count(&mut conn, filter, Some(MAX_EXPORT_CALLS as i64), READ_LIMIT).await?;
    if capped {
        return Err(ApiError::Limit(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("more than {MAX_EXPORT_CALLS} calls match; narrow the filter"),
        ));
    }
    if n == 0 {
        return Err(ApiError::BadRequest("no call matches".into()));
    }
    let page = listing::page(&mut conn, filter, true, None, 500, READ_LIMIT).await?;
    let ids: Vec<i64> = page.rows.iter().map(|r| r.id).collect();
    by_ids(state, &ids).await
}

pub(super) async fn time_zone(state: &AppState) -> Result<Tz, ApiError> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'time_zone'")
            .fetch_optional(&state.pool)
            .await?;
    Ok(value
        .and_then(|v| v.as_str().and_then(|name| name.parse().ok()))
        .unwrap_or(chrono_tz::UTC))
}

/// The audio of every file, one after the other, and how many could not be
/// read; refused before any is read when their sizes add up to more than
/// `MAX_EXPORT_BYTES`.
fn join(files: &[PathBuf]) -> Result<(Vec<u8>, usize), ApiError> {
    let total: u64 = files
        .iter()
        .filter_map(|file| std::fs::metadata(file).ok())
        .map(|m| m.len())
        .sum();
    if total > MAX_EXPORT_BYTES {
        return Err(ApiError::Limit(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "the calls' audio is {total} bytes; one export reads at most {MAX_EXPORT_BYTES}"
            ),
        ));
    }
    let mut joined = Vec::new();
    let mut missing = 0;
    for file in files {
        match std::fs::read(file) {
            Ok(data) => match crate::mp3::audio(&data) {
                Some(audio) => joined.extend_from_slice(audio),
                None => missing += 1,
            },
            Err(_) => missing += 1,
        }
    }
    Ok((joined, missing))
}

/// `20310301_120000-20310301_130000_ScannerPulse_TG-7-SYSA,9-SYSB.mp3`: the
/// time range first, in the site's zone, so exports sort with the calls'
/// own files; the filter's range when it has one, else the calls' own.
pub fn export_name(
    parts: &[impl ExportPart],
    from: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
    zone: Tz,
    ext: &str,
) -> String {
    let stamp = |t: DateTime<Utc>| t.with_timezone(&zone).format("%Y%m%d_%H%M%S").to_string();
    let first = from.or_else(|| parts.first().map(|p| p.started_at()));
    let last = until.or_else(|| parts.last().map(|p| p.started_at()));
    let mut talkgroups: Vec<String> = Vec::new();
    for part in parts {
        if let (Some(number), Some(system)) = (part.talkgroup(), part.system()) {
            let tg = format!("{number}-{system}");
            if !talkgroups.contains(&tg) {
                talkgroups.push(tg);
            }
        }
    }
    let range = match (first, last) {
        (Some(a), Some(b)) => format!("{}-{}_", stamp(a), stamp(b)),
        _ => String::new(),
    };
    let tg = if talkgroups.is_empty() {
        String::new()
    } else {
        format!("_TG-{}", talkgroups.join(","))
    };
    format!("{range}ScannerPulse{tg}.{ext}")
}

/// What an export's name needs of a call.
pub trait ExportPart {
    fn started_at(&self) -> DateTime<Utc>;
    fn talkgroup(&self) -> Option<i64>;
    fn system(&self) -> Option<&str>;
}

impl<T: ExportPart> ExportPart for &T {
    fn started_at(&self) -> DateTime<Utc> {
        (**self).started_at()
    }
    fn talkgroup(&self) -> Option<i64> {
        (**self).talkgroup()
    }
    fn system(&self) -> Option<&str> {
        (**self).system()
    }
}

impl ExportPart for crate::listing::Row {
    fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
    fn talkgroup(&self) -> Option<i64> {
        self.talkgroup
    }
    fn system(&self) -> Option<&str> {
        self.system.as_deref()
    }
}

impl ExportPart for Part {
    fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
    fn talkgroup(&self) -> Option<i64> {
        self.talkgroup
    }
    fn system(&self) -> Option<&str> {
        self.system.as_deref()
    }
}

/// `attachment` with the name, in ASCII and in RFC 5987 form.
pub fn attachment(name: &str) -> HeaderValue {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    HeaderValue::from_str(&format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}"
    ))
    .expect("only visible ASCII")
}

#[cfg(test)]
mod tests {
    use super::{ExportPart, attachment, export_name};
    use chrono::{DateTime, TimeZone, Utc};

    struct P(DateTime<Utc>, Option<i64>, Option<&'static str>);
    impl ExportPart for P {
        fn started_at(&self) -> DateTime<Utc> {
            self.0
        }
        fn talkgroup(&self) -> Option<i64> {
            self.1
        }
        fn system(&self) -> Option<&str> {
            self.2
        }
    }

    #[test]
    fn the_name_leads_with_the_time_range_in_the_sites_zone() {
        let a = Utc.with_ymd_and_hms(2031, 7, 1, 17, 0, 0).unwrap();
        let b = Utc.with_ymd_and_hms(2031, 7, 1, 18, 30, 0).unwrap();
        let parts = [
            P(a, Some(7), Some("SYSA")),
            P(b, Some(9), Some("SYSB")),
            P(b, Some(7), Some("SYSA")),
        ];
        let zone: chrono_tz::Tz = "America/New_York".parse().unwrap();
        assert_eq!(
            export_name(&parts, None, None, zone, "mp3"),
            "20310701_130000-20310701_143000_ScannerPulse_TG-7-SYSA,9-SYSB.mp3"
        );
        let from = Utc.with_ymd_and_hms(2031, 7, 1, 16, 0, 0).unwrap();
        assert!(
            export_name(&parts, Some(from), None, chrono_tz::UTC, "xlsx")
                .starts_with("20310701_160000-")
        );
        assert_eq!(
            export_name(&[] as &[P], None, None, chrono_tz::UTC, "xlsx"),
            "ScannerPulse.xlsx"
        );
    }

    #[test]
    fn the_attachment_header_is_ascii_with_the_utf8_name_beside_it() {
        let header = attachment("caf\u{e9} \"x\".mp3");
        let text = header.to_str().unwrap();
        assert!(
            text.starts_with("attachment; filename=\"caf_ _x_.mp3\""),
            "{text}"
        );
        assert!(
            text.contains("filename*=UTF-8''caf%C3%A9%20%22x%22.mp3"),
            "{text}"
        );
    }
}
