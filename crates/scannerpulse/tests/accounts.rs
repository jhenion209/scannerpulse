use scannerpulse::accounts::{self, AccountError};
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn session(pool: &PgPool, login: &str) {
    sqlx::query(
        "INSERT INTO sessions (token_hash, admin_id, csrf, created_at, last_seen) \
         SELECT sha256(convert_to($1, 'UTF8')), id, 'c', now(), now() FROM admins WHERE login = $1",
    )
    .bind(login)
    .execute(pool)
    .await
    .unwrap();
}

async fn sessions(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM sessions")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn an_added_account_signs_in_with_its_password(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    let (_, hash, disabled) = accounts::by_login(&pool, "ann").await.unwrap().unwrap();
    assert!(hash.starts_with("$argon2id$"), "{hash}");
    assert!(!hash.contains(PASSWORD));
    assert!(accounts::verify_password(&hash, PASSWORD));
    assert!(!accounts::verify_password(&hash, "correct horse battery!"));
    assert!(!disabled);
    let listed = accounts::list(&pool).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].password && !listed[0].oidc && !listed[0].disabled);
}

#[sqlx::test(migrations = "./migrations")]
async fn names_and_passwords_are_checked(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    for bad in ["", "a b", "ann<", &"a".repeat(65)] {
        assert!(
            matches!(
                accounts::add(&mut conn, bad, PASSWORD).await,
                Err(AccountError::BadName(_))
            ),
            "{bad:?}"
        );
    }
    assert!(matches!(
        accounts::add(&mut conn, "ann", "eleven char").await,
        Err(AccountError::BadPassword(_))
    ));
    assert!(matches!(
        accounts::add(&mut conn, "ann", &"x".repeat(1025)).await,
        Err(AccountError::BadPassword(_))
    ));
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    assert!(matches!(
        accounts::add(&mut conn, "ann", PASSWORD).await,
        Err(AccountError::Exists(_))
    ));
}

#[test]
fn an_empty_or_broken_hash_matches_nothing() {
    assert!(!accounts::verify_password("", ""));
    assert!(!accounts::verify_password("", PASSWORD));
    assert!(!accounts::verify_password("$argon2id$garbage", PASSWORD));
    let hash = accounts::hash_password(PASSWORD);
    assert!(!accounts::verify_password(&hash, &"x".repeat(2000)));
    assert_ne!(
        hash,
        accounts::hash_password(PASSWORD),
        "each hash has its own salt"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn the_last_enabled_admin_cannot_be_disabled(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    assert!(matches!(
        accounts::disable(&mut conn, "ann").await,
        Err(AccountError::LastAdmin)
    ));
    accounts::add(&mut conn, "bob", PASSWORD).await.unwrap();
    session(&pool, "bob").await;
    session(&pool, "ann").await;
    accounts::disable(&mut conn, "bob").await.unwrap();
    assert_eq!(sessions(&pool).await, 1, "bob's session ended, ann's stays");
    assert!(matches!(
        accounts::disable(&mut conn, "ann").await,
        Err(AccountError::LastAdmin)
    ));
    accounts::disable(&mut conn, "bob").await.unwrap();
    assert!(matches!(
        accounts::disable(&mut conn, "cy").await,
        Err(AccountError::NoSuch(_))
    ));
    accounts::enable(&pool, "bob").await.unwrap();
    accounts::disable(&mut conn, "ann").await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn two_admins_disabling_each_other_at_once_leave_one_enabled(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    accounts::add(&mut conn, "bob", PASSWORD).await.unwrap();
    for _ in 0..5 {
        let (a, b) = (pool.clone(), pool.clone());
        let one = tokio::spawn(async move {
            let mut conn = a.acquire().await.unwrap();
            accounts::disable(&mut conn, "ann").await
        });
        let two = tokio::spawn(async move {
            let mut conn = b.acquire().await.unwrap();
            accounts::disable(&mut conn, "bob").await
        });
        let results = [one.await.unwrap(), two.await.unwrap()];
        assert_eq!(
            results.iter().filter(|r| r.is_ok()).count(),
            1,
            "{results:?}"
        );
        let enabled: i64 = sqlx::query_scalar("SELECT count(*) FROM admins WHERE NOT disabled")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(enabled, 1);
        sqlx::query("UPDATE admins SET disabled = false")
            .execute(&pool)
            .await
            .unwrap();
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_password_ends_the_accounts_sessions(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    session(&pool, "ann").await;
    accounts::set_password(&mut conn, "ann", "a different long one")
        .await
        .unwrap();
    assert_eq!(sessions(&pool).await, 0);
    let (_, hash, _) = accounts::by_login(&pool, "ann").await.unwrap().unwrap();
    assert!(accounts::verify_password(&hash, "a different long one"));
    assert!(!accounts::verify_password(&hash, PASSWORD));
    assert!(matches!(
        accounts::set_password(&mut conn, "cy", "a different long one").await,
        Err(AccountError::NoSuch(_))
    ));
}
