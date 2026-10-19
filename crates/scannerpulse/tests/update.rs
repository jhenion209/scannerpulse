//! The newest release, read and downloaded from a release server, and
//! checked by its signature and its checksums before anything is changed.

use axum::Router;
use axum::extract::Path as UrlPath;
use axum::http::StatusCode;
use axum::routing::get;
use base64::Engine;
use scannerpulse::update::{Release, Source, fetch, latest, release_key};
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/update")
}

/// The test release's key (the fixtures' checksums carry its signature).
fn key() -> Vec<u8> {
    let text = std::fs::read_to_string(fixtures().join("key.b64")).unwrap();
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .unwrap()
}

/// A release server with the fixtures as v9.9.9, laid out as GitHub's;
/// `x86_64` replaces that program's bytes when given.
async fn server(x86_64: Option<&'static [u8]>) -> Source {
    let app = Router::new()
        .route(
            "/latest",
            get(|| async {
                axum::Json(serde_json::json!({"tag_name": "v9.9.9", "body": "What changed."}))
            }),
        )
        .route(
            "/download/v9.9.9/{file}",
            get(move |UrlPath(file): UrlPath<String>| async move {
                match x86_64 {
                    Some(bytes) if file == "scannerpulse-x86_64-linux" => Ok(bytes.to_vec()),
                    _ => std::fs::read(fixtures().join(&file)).map_err(|_| StatusCode::NOT_FOUND),
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Source::at(&format!("http://{address}"))
}

fn both() -> Vec<String> {
    vec!["x86_64".into(), "aarch64".into()]
}

#[tokio::test]
async fn the_newest_release_and_its_programs_are_downloaded_and_checked() {
    let source = server(None).await;
    let release = latest(&source).await.unwrap();
    assert_eq!(
        release,
        Release {
            tag: "v9.9.9".into(),
            version: "9.9.9".into(),
            notes: "What changed.".into()
        }
    );
    let dir = tempfile::tempdir().unwrap();
    fetch(&source, &release, &both(), dir.path(), &key())
        .await
        .unwrap();
    for program in ["scannerpulse-x86_64-linux", "scannerpulse-aarch64-linux"] {
        let path = dir.path().join(program);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(fixtures().join(program)).unwrap(),
            "{program}"
        );
        // It runs from there once, for the schema, before it is put in place.
        let output = std::process::Command::new(&path).output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "scannerpulse 9.9.9"
        );
    }
}

#[tokio::test]
async fn a_program_changed_on_the_server_is_refused() {
    let source = server(Some(b"#!/bin/sh\necho changed\n")).await;
    let release = latest(&source).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let error = fetch(&source, &release, &both(), dir.path(), &key())
        .await
        .unwrap_err();
    assert!(
        error.contains("scannerpulse-x86_64-linux does not match its signed checksum"),
        "{error}"
    );
    assert!(!dir.path().join("scannerpulse-x86_64-linux").exists());
}

#[tokio::test]
async fn checksums_signed_by_another_key_are_refused() {
    let source = server(None).await;
    let release = latest(&source).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    // The project's own key did not sign the test release.
    let error = fetch(
        &source,
        &release,
        &both(),
        dir.path(),
        &release_key().unwrap(),
    )
    .await
    .unwrap_err();
    assert!(error.contains("does not check out"), "{error}");
    assert!(!dir.path().join("scannerpulse-x86_64-linux").exists());
}

#[tokio::test]
async fn a_release_server_that_does_not_answer_is_named() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = free.local_addr().unwrap();
    drop(free);
    let error = latest(&Source::at(&format!("http://{address}")))
        .await
        .unwrap_err();
    assert!(
        error.contains(&format!("http://{address}/latest")),
        "{error}"
    );
}
