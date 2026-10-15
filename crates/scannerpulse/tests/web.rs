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
    assert_eq!(body, "the database does not answer");
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_path_is_not_found(pool: PgPool) {
    let options = (*pool.connect_options()).clone();
    let (status, _) = get(app(&pool, options), "/nonexistent").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
