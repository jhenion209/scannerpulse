//! The admin accounts: one role, several accounts, at least one enabled.
//! The command line and the admin page call the same functions.

use std::fmt;
use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Connection;
use sqlx::postgres::PgConnection;

/// The shortest password an account may have.
pub const MIN_PASSWORD: usize = 12;
/// The longest, so that hashing a request's password stays cheap to refuse.
pub const MAX_PASSWORD: usize = 1024;
/// The longest login name.
pub const MAX_LOGIN: usize = 64;

#[derive(Debug)]
pub enum AccountError {
    /// The login name cannot be used; the text says why.
    BadName(String),
    /// The password cannot be used; the text says why.
    BadPassword(String),
    Exists(String),
    NoSuch(String),
    /// Disabling this account would leave no enabled admin.
    LastAdmin,
    Database(sqlx::Error),
}

impl fmt::Display for AccountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AccountError::BadName(why) | AccountError::BadPassword(why) => f.write_str(why),
            AccountError::Exists(login) => write!(f, "an admin named {login} already exists"),
            AccountError::NoSuch(login) => write!(f, "there is no admin named {login}"),
            AccountError::LastAdmin => {
                f.write_str("this is the last enabled admin: enable or add another first")
            }
            AccountError::Database(error) => write!(f, "the database failed: {error}"),
        }
    }
}

impl std::error::Error for AccountError {}

impl From<sqlx::Error> for AccountError {
    fn from(error: sqlx::Error) -> AccountError {
        AccountError::Database(error)
    }
}

/// A login name: 1 to 64 letters, digits and `.`, `_`, `-`, `@`.
pub fn check_login(login: &str) -> Result<(), AccountError> {
    let ok = !login.is_empty()
        && login.len() <= MAX_LOGIN
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@'));
    if ok {
        Ok(())
    } else {
        Err(AccountError::BadName(format!(
            "a login name is 1 to {MAX_LOGIN} letters, digits and . _ - @"
        )))
    }
}

pub fn check_password(password: &str) -> Result<(), AccountError> {
    let length = password.chars().count();
    if length < MIN_PASSWORD {
        return Err(AccountError::BadPassword(format!(
            "a password has at least {MIN_PASSWORD} characters"
        )));
    }
    if password.len() > MAX_PASSWORD {
        return Err(AccountError::BadPassword(format!(
            "a password has at most {MAX_PASSWORD} bytes"
        )));
    }
    Ok(())
}

/// An Argon2id hash of `password`, with a fresh salt.
pub fn hash_password(password: &str) -> String {
    Argon2::default()
        .hash_password(password.as_bytes())
        .expect("Argon2 with its default parameters hashes any password")
        .to_string()
}

/// Whether `password` matches `hash`. An empty or unreadable hash matches
/// nothing; a password too long to hash matches nothing.
pub fn verify_password(hash: &str, password: &str) -> bool {
    if password.len() > MAX_PASSWORD {
        return false;
    }
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .is_ok()
}

/// Takes as long as checking a real password, for a name that has no
/// account, so the time of the answer does not tell whether the name exists.
pub fn verify_nothing(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("not the password of any account"));
    verify_password(hash, password);
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Admin {
    pub login: String,
    /// The account can sign in with a password.
    pub password: bool,
    /// The account signs in through the OIDC provider.
    pub oidc: bool,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_sign_in: Option<DateTime<Utc>>,
}

pub async fn list(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<Vec<Admin>> {
    sqlx::query_as(
        "SELECT login, password_hash <> '' AS password, oidc_subject IS NOT NULL AS oidc, \
                disabled, created_at, last_sign_in \
         FROM admins ORDER BY login",
    )
    .fetch_all(db)
    .await
}

/// An account found by its login name: its id, its password hash, and whether
/// it is disabled.
pub async fn by_login(
    db: impl sqlx::PgExecutor<'_>,
    login: &str,
) -> sqlx::Result<Option<(i64, String, bool)>> {
    sqlx::query_as("SELECT id, password_hash, disabled FROM admins WHERE login = $1")
        .bind(login)
        .fetch_optional(db)
        .await
}

/// A checked password's hash. Hashing takes tens of milliseconds of one
/// core: a server calls this on its blocking threads.
pub fn new_hash(password: &str) -> Result<String, AccountError> {
    check_password(password)?;
    Ok(hash_password(password))
}

/// Adds an enabled account that signs in with `password`; returns its id.
pub async fn add(
    conn: &mut PgConnection,
    login: &str,
    password: &str,
) -> Result<i64, AccountError> {
    check_login(login)?;
    let hash = new_hash(password)?;
    add_hashed(conn, login, &hash).await
}

/// Adds an enabled account with an Argon2id `hash` from `new_hash`; returns its id.
pub async fn add_hashed(
    conn: &mut PgConnection,
    login: &str,
    hash: &str,
) -> Result<i64, AccountError> {
    check_login(login)?;
    let id: Option<i64> = sqlx::query_scalar(
        "INSERT INTO admins (login, password_hash) VALUES ($1, $2) \
         ON CONFLICT (login) DO NOTHING RETURNING id",
    )
    .bind(login)
    .bind(hash)
    .fetch_optional(&mut *conn)
    .await?;
    id.ok_or_else(|| AccountError::Exists(login.to_string()))
}

/// Gives the account a new password and ends its sessions.
pub async fn set_password(
    conn: &mut PgConnection,
    login: &str,
    password: &str,
) -> Result<(), AccountError> {
    let hash = new_hash(password)?;
    set_hash(conn, login, &hash).await
}

/// Gives the account the password whose hash is `hash` (from `new_hash`) and ends its sessions.
pub async fn set_hash(
    conn: &mut PgConnection,
    login: &str,
    hash: &str,
) -> Result<(), AccountError> {
    let mut tx = conn.begin().await?;
    let id: Option<i64> =
        sqlx::query_scalar("UPDATE admins SET password_hash = $2 WHERE login = $1 RETURNING id")
            .bind(login)
            .bind(hash)
            .fetch_optional(&mut *tx)
            .await?;
    let id = id.ok_or_else(|| AccountError::NoSuch(login.to_string()))?;
    sqlx::query("DELETE FROM sessions WHERE admin_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Disables the account and ends its sessions; refused for the last enabled
/// account.
pub async fn disable(conn: &mut PgConnection, login: &str) -> Result<(), AccountError> {
    let mut tx = conn.begin().await?;
    // Two admins disabling each other at once are taken one after the other,
    // so the second sees the first's change.
    let enabled: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, login FROM admins WHERE NOT disabled ORDER BY id FOR UPDATE")
            .fetch_all(&mut *tx)
            .await?;
    let Some((id, _)) = enabled.iter().find(|(_, name)| name == login) else {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM admins WHERE login = $1)")
                .bind(login)
                .fetch_one(&mut *tx)
                .await?;
        return if exists {
            Ok(())
        } else {
            Err(AccountError::NoSuch(login.to_string()))
        };
    };
    if enabled.len() == 1 {
        return Err(AccountError::LastAdmin);
    }
    sqlx::query("UPDATE admins SET disabled = true WHERE id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM sessions WHERE admin_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn enable(db: impl sqlx::PgExecutor<'_>, login: &str) -> Result<(), AccountError> {
    let done = sqlx::query("UPDATE admins SET disabled = false WHERE login = $1")
        .bind(login)
        .execute(db)
        .await?;
    if done.rows_affected() == 0 {
        return Err(AccountError::NoSuch(login.to_string()));
    }
    Ok(())
}

/// The setting that allows password sign-in; it is on when not set.
pub const PASSWORD_SIGNIN: &str = "auth.password_signin";

/// Whether password sign-in is allowed. A value that is not true or false
/// counts as on, so a broken setting never locks every admin out.
pub async fn password_signin(db: impl sqlx::PgExecutor<'_>) -> sqlx::Result<bool> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = $1")
            .bind(PASSWORD_SIGNIN)
            .fetch_optional(db)
            .await?;
    Ok(value.and_then(|v| v.as_bool()).unwrap_or(true))
}

pub async fn set_password_signin(db: impl sqlx::PgExecutor<'_>, on: bool) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES ($1, to_jsonb($2), now()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
    )
    .bind(PASSWORD_SIGNIN)
    .bind(on)
    .execute(db)
    .await?;
    Ok(())
}

/// The account of someone who signed in through the OIDC provider `issuer`
/// as `subject`: its id, login name and whether it is disabled. The first
/// sign-in creates it, named `wanted` when that name is free and valid. An
/// account of another way in is never taken over: a name already in use
/// gets a suffix.
pub async fn oidc_account(
    conn: &mut PgConnection,
    issuer: &str,
    subject: &str,
    wanted: Option<&str>,
) -> sqlx::Result<(i64, String, bool)> {
    let base = wanted
        .filter(|name| check_login(name).is_ok())
        .unwrap_or("oidc")
        .chars()
        .take(MAX_LOGIN - 7)
        .collect::<String>();
    for attempt in 0..5 {
        let found: Option<(i64, String, bool)> = sqlx::query_as(
            "SELECT id, login, disabled FROM admins WHERE oidc_issuer = $1 AND oidc_subject = $2",
        )
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(found) = found {
            return Ok(found);
        }
        let login = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", crate::sessions::random_hex(3))
        };
        let created: Option<i64> = sqlx::query_scalar(
            "INSERT INTO admins (login, oidc_issuer, oidc_subject) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(&login)
        .bind(issuer)
        .bind(subject)
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(id) = created {
            return Ok((id, login, false));
        }
    }
    Err(sqlx::Error::Protocol(
        "no free login name for a new OIDC account".into(),
    ))
}
