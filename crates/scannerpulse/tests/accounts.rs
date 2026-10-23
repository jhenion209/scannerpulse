use scannerpulse::accounts::{self, AccountError};
use scannerpulse::roles::{Function, Role};
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

#[test]
fn a_password_hashed_by_version_0_1_still_signs_in() {
    // Made by argon2 0.5, which ScannerPulse 0.1 hashed with.
    let old = "$argon2id$v=19$m=19456,t=2,p=1$c2Nhbm5lcnB1bHNlLXRlc3Q$LWuF7C5e8FyNXGNCs/Rm6OrDRmU3PwoRSLUqRB+Qzuc";
    assert!(accounts::verify_password(old, "an old pass phrase"));
    assert!(!accounts::verify_password(old, PASSWORD));
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

#[sqlx::test(migrations = "./migrations")]
async fn password_sign_in_is_on_unless_switched_off(pool: PgPool) {
    assert!(accounts::password_signin(&pool).await.unwrap());
    accounts::set_password_signin(&pool, false).await.unwrap();
    assert!(!accounts::password_signin(&pool).await.unwrap());
    accounts::set_password_signin(&pool, true).await.unwrap();
    assert!(accounts::password_signin(&pool).await.unwrap());
    sqlx::query(
        r#"INSERT INTO settings (key, value) VALUES ('auth.password_signin', '"x"')
           ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(accounts::password_signin(&pool).await.unwrap());
}

/// The database as the release before the roles left it: every migration
/// before 0019.
async fn before_roles(pool: &PgPool) {
    let dir = tempfile::tempdir().unwrap();
    let all = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(all).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if name.as_str() < "0019" {
            std::fs::copy(&path, dir.path().join(name)).unwrap();
        }
    }
    sqlx::migrate::Migrator::new(dir.path())
        .await
        .unwrap()
        .run(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn every_admin_from_before_the_roles_is_a_super_admin(pool: PgPool) {
    before_roles(&pool).await;
    sqlx::query("INSERT INTO admins (login, password_hash) VALUES ('ann', 'x')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO admins (login, oidc_issuer, oidc_subject) \
         VALUES ('bob', 'https://id.example.org', 'b-1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    scannerpulse::MIGRATOR.run(&pool).await.unwrap();
    let listed = accounts::list(&pool).await.unwrap();
    let roles: Vec<(&str, &str)> = listed
        .iter()
        .map(|a| (a.login.as_str(), a.role.as_str()))
        .collect();
    assert_eq!(roles, [("ann", "super"), ("bob", "super")]);
    // Someone signing in through the provider for the first time after it
    // arrives as an associate with every function off.
    let mut conn = pool.acquire().await.unwrap();
    accounts::oidc_account(&mut conn, "https://id.example.org", "c-2", Some("cy"))
        .await
        .unwrap();
    let cy = accounts::list(&pool).await.unwrap().pop().unwrap();
    assert_eq!((cy.login.as_str(), cy.role.as_str()), ("cy", "associate"));
    assert!(cy.functions.is_empty());
}

#[sqlx::test(migrations = "./migrations")]
async fn an_oidc_account_keeps_the_role_and_functions_a_super_admin_set(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let issuer = "https://id.example.org";
    accounts::oidc_account(&mut conn, issuer, "c-2", Some("cy"))
        .await
        .unwrap();
    accounts::switch(&pool, "cy", &[Function::Move], true)
        .await
        .unwrap();
    accounts::oidc_account(&mut conn, issuer, "c-2", Some("cy"))
        .await
        .unwrap();
    let cy = accounts::list(&pool).await.unwrap().pop().unwrap();
    assert_eq!(cy.role, "associate");
    assert_eq!(cy.functions, ["move"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_associate_starts_with_every_function_off_and_is_switched_one_by_one(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    let functions = |pool: PgPool| async move {
        let all = accounts::list(&pool).await.unwrap();
        all.into_iter().find(|a| a.login == "cy").unwrap().functions
    };
    assert!(functions(pool.clone()).await.is_empty());
    accounts::switch(&pool, "cy", &[Function::Purge, Function::Move], true)
        .await
        .unwrap();
    accounts::switch(&pool, "cy", &[Function::Move], true)
        .await
        .unwrap();
    assert_eq!(functions(pool.clone()).await, ["move", "purge"]);
    accounts::switch(&pool, "cy", &[Function::Purge], false)
        .await
        .unwrap();
    assert_eq!(functions(pool.clone()).await, ["move"]);
    assert!(matches!(
        accounts::switch(&pool, "ann", &[Function::Move], true).await,
        Err(AccountError::Super(_))
    ));
    assert!(matches!(
        accounts::switch(&pool, "dan", &[Function::Move], true).await,
        Err(AccountError::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_last_enabled_super_admin_is_neither_disabled_nor_made_an_associate(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    let hash = accounts::new_hash(PASSWORD).unwrap();
    accounts::add_hashed(&mut conn, "cy", &hash, Role::Associate)
        .await
        .unwrap();
    // An enabled associate does not make ann's account any less the last.
    assert!(matches!(
        accounts::disable(&mut conn, "ann").await,
        Err(AccountError::LastAdmin)
    ));
    assert!(matches!(
        accounts::set_role(&mut conn, "ann", Role::Associate).await,
        Err(AccountError::LastAdmin)
    ));
    accounts::disable(&mut conn, "cy").await.unwrap();
    accounts::enable(&pool, "cy").await.unwrap();
    // Made a super admin, cy lets ann become an associate, and back.
    accounts::set_role(&mut conn, "cy", Role::Super)
        .await
        .unwrap();
    accounts::set_role(&mut conn, "ann", Role::Associate)
        .await
        .unwrap();
    assert!(matches!(
        accounts::set_role(&mut conn, "cy", Role::Associate).await,
        Err(AccountError::LastAdmin)
    ));
    accounts::set_role(&mut conn, "ann", Role::Super)
        .await
        .unwrap();
    assert!(matches!(
        accounts::set_role(&mut conn, "dan", Role::Super).await,
        Err(AccountError::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn two_super_admins_demoting_each_other_at_once_leave_one(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    accounts::add(&mut conn, "ann", PASSWORD).await.unwrap();
    accounts::add(&mut conn, "bob", PASSWORD).await.unwrap();
    for _ in 0..5 {
        let (a, b) = (pool.clone(), pool.clone());
        let one = tokio::spawn(async move {
            let mut conn = a.acquire().await.unwrap();
            accounts::set_role(&mut conn, "ann", Role::Associate).await
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
        let supers: i64 =
            sqlx::query_scalar("SELECT count(*) FROM admins WHERE NOT disabled AND role = 'super'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(supers, 1);
        sqlx::query("UPDATE admins SET role = 'super', disabled = false")
            .execute(&pool)
            .await
            .unwrap();
    }
}
