mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use scannerpulse::web::{self, AppState};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use tower::ServiceExt;

fn app(pool: &PgPool, database: PgConnectOptions) -> axum::Router {
    web::router(AppState::new(pool.clone(), database))
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[sqlx::test(migrations = "./migrations")]
async fn health_answers_and_healthz_checks_the_database(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    assert_eq!(
        get(app(&pool, options.clone()), "/health").await,
        (StatusCode::OK, "ok".into())
    );
    assert_eq!(
        get(app(&pool, options), "/healthz").await,
        (StatusCode::OK, "ok".into())
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn healthz_fails_when_the_database_cannot_be_reached(pool: PgPool) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let nowhere = PgConnectOptions::new().host("127.0.0.1").port(port);
    let (status, body) = get(app(&pool, nowhere), "/healthz").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body,
        r#"{"detail":"the database does not answer","error":"service unavailable"}"#
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_path_is_not_found(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    let (status, body) = get(app(&pool, options), "/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, r#"{"detail":"","error":"not found"}"#);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_wrong_method_is_refused_in_json_with_the_methods_allowed(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    let response = app(&pool, options)
        .oneshot(
            Request::get("/api/export/audio")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers()["allow"], "POST");
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], br#"{"detail":"","error":"method not allowed"}"#);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_frontends_files_are_served_and_its_page_answers_its_own_paths(pool: PgPool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<p>page</p>").unwrap();
    std::fs::create_dir(dir.path().join("assets")).unwrap();
    std::fs::write(dir.path().join("assets/app.js"), "let x = 1;").unwrap();
    let mut state = AppState::new(pool.clone(), (*pool.connect_options()).clone());
    state.static_dir = Some(dir.path().to_path_buf());
    let app = web::router(state);
    assert_eq!(
        get(app.clone(), "/assets/app.js").await,
        (StatusCode::OK, "let x = 1;".into())
    );
    assert_eq!(
        get(app.clone(), "/").await,
        (StatusCode::OK, "<p>page</p>".into())
    );
    assert_eq!(
        get(app.clone(), "/calls/123").await,
        (StatusCode::OK, "<p>page</p>".into())
    );
    let (status, body) = get(app.clone(), "/api/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("\"error\":\"not found\""), "{body}");
    // A known API path with a wrong method is not the frontend's page.
    let (status, body) = get(app.clone(), "/api/export/audio").await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert!(body.contains("\"error\":\"method not allowed\""), "{body}");
    // A path that climbs out of the folder never reads outside it.
    let (_, body) = get(app, "/../../../../etc/passwd").await;
    assert!(!body.contains("root:"), "{body}");
}
