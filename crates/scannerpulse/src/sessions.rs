//! Admin sessions and the lockout of failed password sign-ins. Both live in
//! the database, so any web host can serve any request.

use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgConnection;

/// A session ends this long after the admin's last request...
pub const IDLE_HOURS: i32 = 12;
/// ...and at the latest this long after signing in.
pub const MAX_DAYS: i32 = 7;
/// A session opened through the OIDC provider ends this long after signing in, whatever its use.
pub const OIDC_HOURS: i32 = 12;
/// This many failed sign-ins for one name, or from one address, within
/// `WINDOW_MINUTES` lock that name or that address for `WINDOW_MINUTES`.
pub const FAILURES: i64 = 5;
pub const WINDOW_MINUTES: i32 = 15;

/// A signed-in admin.
#[derive(Debug, Clone)]
pub struct Session {
    pub admin_id: i64,
    pub login: String,
    /// The token every change must carry in its `X-CSRF-Token` header.
    pub csrf: String,
}

/// `bytes` random bytes, as lowercase hex.
pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Opens a session for the admin: returns the token for the cookie and the
/// CSRF token. Records the sign-in and removes sessions that have ended.
pub async fn create(
    conn: &mut PgConnection,
    admin_id: i64,
    oidc: bool,
) -> sqlx::Result<(String, String)> {
    let token = random_hex(32);
    let csrf = random_hex(16);
    sqlx::query(
        "INSERT INTO sessions (token_hash, admin_id, csrf, oidc, created_at, last_seen) \
         VALUES ($1, $2, $3, $4, now(), now())",
    )
    .bind(digest(&token))
    .bind(admin_id)
    .bind(&csrf)
    .bind(oidc)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE admins SET last_sign_in = now() WHERE id = $1")
        .bind(admin_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "DELETE FROM sessions \
         WHERE last_seen < now() - make_interval(hours => $1) \
            OR created_at < now() - make_interval(days => $2)",
    )
    .bind(IDLE_HOURS)
    .bind(MAX_DAYS)
    .execute(&mut *conn)
    .await?;
    Ok((token, csrf))
}

/// The session of `token`, when it has not ended and its account is
/// enabled. Counts as a request: the idle limit starts again.
pub async fn find(db: impl sqlx::PgExecutor<'_>, token: &str) -> sqlx::Result<Option<Session>> {
    lookup(db, token, true).await
}

/// `find`, with the choice of whether the lookup counts as use. A lookup
/// that does not (`touch` false: a read the admin page makes on a timer)
/// checks the session in every way, the idle limit too, but leaves
/// `last_seen` where it was, so a page left open cannot keep a session
/// alive past its idle limit.
pub async fn lookup(
    db: impl sqlx::PgExecutor<'_>,
    token: &str,
    touch: bool,
) -> sqlx::Result<Option<Session>> {
    let found: Option<(i64, String, String)> = sqlx::query_as(
        "UPDATE sessions s SET last_seen = CASE WHEN $5 THEN now() ELSE s.last_seen END \
         FROM admins a \
         WHERE s.token_hash = $1 AND a.id = s.admin_id AND NOT a.disabled \
           AND s.last_seen >= now() - make_interval(hours => $2) \
           AND s.created_at >= now() - make_interval(days => $3) \
           AND (NOT s.oidc OR s.created_at >= now() - make_interval(hours => $4)) \
         RETURNING a.id, a.login, s.csrf",
    )
    .bind(digest(token))
    .bind(IDLE_HOURS)
    .bind(MAX_DAYS)
    .bind(OIDC_HOURS)
    .bind(touch)
    .fetch_optional(db)
    .await?;
    Ok(found.map(|(admin_id, login, csrf)| Session {
        admin_id,
        login,
        csrf,
    }))
}

/// Signs out: the session is deleted.
pub async fn end(db: impl sqlx::PgExecutor<'_>, token: &str) -> sqlx::Result<()> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
        .bind(digest(token))
        .execute(db)
        .await?;
    Ok(())
}

/// What a failed sign-in is counted against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Name,
    Address,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Name => "name",
            Kind::Address => "address",
        }
    }
}

/// What is stored for a failure: an address as it is, a name only as the
/// lowercase hex SHA-256 of its text, since a mistyped password may land in
/// the name field. An unknown name is counted like any other.
fn key(kind: Kind, value: &str) -> String {
    match kind {
        Kind::Address => value.to_string(),
        Kind::Name => Sha256::digest(value.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    }
}

/// Whether the name or the address is locked: within the last
/// `WINDOW_MINUTES` there was a failure that was at least the `FAILURES`th
/// within `WINDOW_MINUTES`. A name is looked up by its hash.
pub async fn locked(db: impl sqlx::PgExecutor<'_>, kind: Kind, value: &str) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (\
           SELECT 1 FROM sign_in_failures f \
           WHERE f.kind = $1 AND f.value = $2 \
             AND f.at > now() - make_interval(mins => $3) \
             AND (SELECT count(*) FROM sign_in_failures g \
                  WHERE g.kind = f.kind AND g.value = f.value \
                    AND g.at BETWEEN f.at - make_interval(mins => $3) AND f.at) >= $4)",
    )
    .bind(kind.as_str())
    .bind(key(kind, value))
    .bind(WINDOW_MINUTES)
    .bind(FAILURES)
    .fetch_one(db)
    .await
}

/// Counts a failed sign-in against the name (kept only as its SHA-256) and
/// the address, and forgets failures older than a day.
pub async fn failed(conn: &mut PgConnection, name: &str, address: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO sign_in_failures (kind, value, at) \
         VALUES ('name', $1, now()), ('address', $2, now())",
    )
    .bind(key(Kind::Name, name))
    .bind(key(Kind::Address, address))
    .execute(&mut *conn)
    .await?;
    sqlx::query("DELETE FROM sign_in_failures WHERE at < now() - interval '1 day'")
        .execute(&mut *conn)
        .await?;
    Ok(())
}
