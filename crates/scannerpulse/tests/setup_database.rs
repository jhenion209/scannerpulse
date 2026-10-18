//! Setup checks a database the person runs before it plans anything on it.

use sqlx::PgPool;

#[sqlx::test]
async fn a_postgresql_17_database_whose_role_may_make_tables_passes(pool: PgPool) {
    let options = pool.connect_options().as_ref().clone();
    let version = scannerpulse::setup::ask::check_database(options)
        .await
        .unwrap();
    assert!(version.starts_with("PostgreSQL 17."), "{version}");
}

#[sqlx::test]
async fn a_database_that_does_not_answer_says_where_it_looked(pool: PgPool) {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    let options = pool
        .connect_options()
        .as_ref()
        .clone()
        .host("127.0.0.1")
        .port(port);
    let error = scannerpulse::setup::ask::check_database(options)
        .await
        .unwrap_err();
    assert!(error.contains(&format!("on 127.0.0.1:{port}")), "{error}");
}
