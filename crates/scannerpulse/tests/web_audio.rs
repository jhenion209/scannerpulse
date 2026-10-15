mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, TimeZone, Utc};
use http_body_util::BodyExt;
use scannerpulse::web::{self, AppState};
use sqlx::PgPool;
use tempfile::TempDir;
use tower::ServiceExt;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap()
}

/// `n` calls of SYSA/127, each a file of `frames` frames in the location,
/// except the calls listed in `missing`, which have no file.
async fn calls_with_files(
    pool: &PgPool,
    n: u32,
    frames: usize,
    missing: &[u32],
) -> (TempDir, Vec<i64>) {
    let root = tempfile::tempdir().unwrap();
    let places = common::seed_places(pool, root.path(), root.path()).await;
    let mut ids = Vec::new();
    for i in 0..n {
        let rel_path = format!("2031-03-01/127/call-{i}.mp3");
        if !missing.contains(&i) {
            std::fs::create_dir_all(root.path().join("2031-03-01/127")).unwrap();
            let mut tag = id3::Tag::new();
            id3::TagLike::set_title(&mut tag, "x");
            std::fs::write(root.path().join(&rel_path), common::tagged(&tag, frames)).unwrap();
        }
        let (id, _) = common::insert_call(pool, places, &rel_path, at(i), 2000, true).await;
        ids.push(id);
    }
    (root, ids)
}

async fn send(
    pool: &PgPool,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let app = web::router(AppState::new(
        pool.clone(),
        (*pool.connect_options()).clone(),
    ));
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

fn post(json: serde_json::Value) -> Request<Body> {
    Request::post("/api/export/audio")
        .header("content-type", "application/json")
        .body(Body::from(json.to_string()))
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_is_served_whole_or_by_range(pool: PgPool) {
    let (root, ids) = calls_with_files(&pool, 2, 5, &[1]).await;
    let file = std::fs::read(root.path().join("2031-03-01/127/call-0.mp3")).unwrap();
    let (status, headers, body) = send(
        &pool,
        Request::get(format!("/audio/{}", ids[0]))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "audio/mpeg");
    assert_eq!(body, file);
    let ranged = Request::get(format!("/audio/{}", ids[0]))
        .header("range", "bytes=10-109")
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(&pool, ranged).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, file[10..110].to_vec());
    // A call whose file is gone, and a call that does not exist.
    let gone = Request::get(format!("/audio/{}", ids[1]))
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&pool, gone).await.0, StatusCode::NOT_FOUND);
    let none = Request::get("/audio/999999").body(Body::empty()).unwrap();
    assert_eq!(send(&pool, none).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_export_joins_the_calls_audio_and_counts_the_missing(pool: PgPool) {
    let (_root, ids) = calls_with_files(&pool, 3, 4, &[2]).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"UTC\"')")
        .execute(&pool)
        .await
        .unwrap();
    let (status, headers, body) = send(&pool, post(serde_json::json!({"ids": ids}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["content-type"], "audio/mpeg");
    assert_eq!(headers["x-export-missing"], "1");
    // The tags are gone: the body is the two readable calls' frames.
    assert_eq!(body, common::frames(8));
    let disposition = headers["content-disposition"].to_str().unwrap();
    assert!(
        disposition.contains("20310301_120000-20310301_120200_ScannerPulse_TG-127-SYSA.mp3"),
        "{disposition}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_export_by_filter_takes_the_matching_calls(pool: PgPool) {
    let (_root, _ids) = calls_with_files(&pool, 3, 2, &[]).await;
    let (status, _, body) = send(
        &pool,
        post(serde_json::json!({"from": "2031-03-01T12:01:00Z"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, common::frames(4));
    let (status, _, _) = send(
        &pool,
        post(serde_json::json!({"from": "2032-01-01T00:00:00Z"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no call matches");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_export_too_large_or_empty_is_refused(pool: PgPool) {
    let (_root, _) = calls_with_files(&pool, 1, 1, &[]).await;
    let (status, _, body) = send(&pool, post(serde_json::json!({"ids": []}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(String::from_utf8_lossy(&body).contains("at least one"));
    let many: Vec<i64> = (1..=201).collect();
    let (status, _, body) = send(&pool, post(serde_json::json!({"ids": many}))).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(String::from_utf8_lossy(&body).contains("201 calls"));
}
