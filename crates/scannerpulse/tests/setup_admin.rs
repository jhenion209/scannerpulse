//! Setup's first admin, on a new database and on one `uninstall` kept.

use scannerpulse::accounts;
use scannerpulse::setup::apply::first_admin;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn the_first_admin_is_added_then_given_the_new_password_on_a_kept_database(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    first_admin(&mut conn, "chief", "the first long password")
        .await
        .unwrap();
    first_admin(&mut conn, "chief", "the second long password")
        .await
        .unwrap();
    let all = accounts::list(&mut *conn).await.unwrap();
    assert_eq!(all.len(), 1);
    let (_, hash, disabled) = accounts::by_login(&mut *conn, "chief")
        .await
        .unwrap()
        .unwrap();
    assert!(!disabled);
    assert!(accounts::verify_password(&hash, "the second long password"));
    let error = first_admin(&mut conn, "chief", "short").await.unwrap_err();
    assert!(error.starts_with("the admin chief: "), "{error}");
}

/// The database URL of the test's own database, its host not answering:
/// the steps must find the primary among the nodes.
fn url_on_nowhere(pool: &PgPool) -> String {
    let server = std::env::var("DATABASE_URL").unwrap();
    let (front, query) = server.split_once('?').unwrap_or((&server, ""));
    let (front, _) = front.rsplit_once('/').unwrap();
    let (scheme_and_login, _host) = front.rsplit_once('@').unwrap_or(("postgres://", front));
    let database = pool.connect_options().get_database().unwrap().to_string();
    let at = if scheme_and_login.ends_with("://") {
        ""
    } else {
        "@"
    };
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    format!("{scheme_and_login}{at}127.0.0.1:1/{database}{query}")
}

#[sqlx::test(migrations = "./migrations")]
async fn the_first_admin_is_added_on_the_primary(pool: PgPool) {
    use scannerpulse::primary::Node;
    use scannerpulse::setup::apply::{Context, apply};
    use scannerpulse::setup::steps::{Action, Step};
    let options = pool.connect_options();
    let context = Context {
        database_url: url_on_nowhere(&pool),
        db_password: String::new(),
        nodes: vec![
            Node {
                host: "127.0.0.1".into(),
                port: 1,
            },
            Node {
                host: options.get_host().into(),
                port: options.get_port(),
            },
        ],
    };
    let steps = [Step {
        title: "Add the first admin".into(),
        action: Action::Admin {
            login: "chief".into(),
            password: "the first long password".into(),
        },
    }];
    apply(&steps, &context, true).await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    assert!(
        accounts::by_login(&mut *conn, "chief")
            .await
            .unwrap()
            .is_some()
    );
}
