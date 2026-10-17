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
