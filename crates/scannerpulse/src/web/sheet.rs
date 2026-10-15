//! `/api/export/xlsx`: every match of a search, newest first, as an Excel
//! file.

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;

use super::audio::{attachment, export_name, time_zone};
use super::calls::{ListParams, READ_LIMIT};
use super::{ApiError, AppState};
use crate::listing;
use crate::xlsx::{Cell, workbook};

/// The most rows one file holds; a longer list is cut, and the
/// `X-Export-Capped-At` header says so.
pub const MAX_ROWS: i64 = 100_000;

pub async fn export(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Response, ApiError> {
    let _permit = state.exports.clone().try_acquire_owned().map_err(|_| {
        ApiError::Limit(
            StatusCode::TOO_MANY_REQUESTS,
            "two exports are running; try again shortly".into(),
        )
    })?;
    let filter = super::calls::filter(&params)?;
    let zone = time_zone(&state).await?;
    let mut conn = state.pool.acquire().await?;
    let page = listing::page(&mut conn, &filter, false, None, MAX_ROWS, READ_LIMIT).await?;
    drop(conn);
    let capped = page.next.is_some();
    let rows: Vec<Vec<Cell>> = page
        .rows
        .iter()
        .map(|r| {
            let local = r.started_at.with_timezone(&zone);
            vec![
                Cell::Text(local.format("%Y-%m-%d").to_string()),
                Cell::Text(local.format("%H:%M:%S").to_string()),
                Cell::Text(r.talkgroup_name.clone().unwrap_or_default()),
                Cell::Number(f64::from(r.duration_ms.unwrap_or(0)) / 1000.0),
                Cell::Text(r.transcript.clone().unwrap_or_default()),
            ]
        })
        .collect();
    let oldest_first: Vec<&listing::Row> = page.rows.iter().rev().collect();
    let name = export_name(&oldest_first, filter.from, filter.until, zone, "xlsx");
    drop(page);
    let data = tokio::task::spawn_blocking(move || {
        workbook(
            "Calls",
            &["Date", "Time", "Talkgroup", "Length (s)", "Text"],
            &rows,
        )
    })
    .await
    .map_err(|error| ApiError::Internal(format!("the Excel export failed: {error}")))?
    .map_err(|message| ApiError::Limit(StatusCode::PAYLOAD_TOO_LARGE, message))?;
    let mut response = Response::new(Body::from(data));
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ),
    );
    headers.insert(header::CONTENT_DISPOSITION, attachment(&name));
    if capped {
        headers.insert("x-export-capped-at", HeaderValue::from(MAX_ROWS));
    }
    Ok(response)
}
