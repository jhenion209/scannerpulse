//! Signing in and out, and what an admin route requires: a session in the
//! cookie and, for a change, the session's CSRF token in a header and what
//! the change needs (`needs`): a function the admin holds, or a super admin.

use axum::Json;
use axum::extract::{FromRequestParts, MatchedPath, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::json;

use super::client::Client;
use super::needs::{self, Need};
use super::{ApiError, AppState};
use crate::accounts;
use crate::roles::{Function, Role};
use crate::sessions::{self, Kind, Session};

/// The cookie that holds the session's token.
pub const COOKIE: &str = "scannerpulse_session";
/// The header that carries the session's CSRF token on every change.
pub const CSRF_HEADER: &str = "x-csrf-token";
/// The header the admin page puts on a read it makes on a timer: such a
/// read does not count as use of the session (`1`; ignored on a change).
pub const POLL_HEADER: &str = "x-scannerpulse-poll";

/// The session's token from the request's cookies.
fn token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix(COOKIE)?.strip_prefix('='))
        .find(|token| token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_string)
}

/// Compares in time that does not depend on where the texts first differ.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn csrf_ok(headers: &HeaderMap, session: &Session) -> bool {
    headers
        .get(CSRF_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| same(value, &session.csrf))
}

fn reads_only(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// A read the page made on its own: a change always counts as use.
fn polled(parts: &Parts) -> bool {
    reads_only(&parts.method)
        && parts
            .headers
            .get(POLL_HEADER)
            .is_some_and(|value| value == "1")
}

/// The session the request's cookie names, if it is open; the CSRF token
/// is not checked.
pub async fn session(
    db: impl sqlx::PgExecutor<'_>,
    headers: &HeaderMap,
) -> sqlx::Result<Option<Session>> {
    match token(headers) {
        Some(token) => sessions::find(db, &token).await,
        None => Ok(None),
    }
}

/// A signed-in admin; for a change, with the session's CSRF token, and
/// allowed what the change needs, and the address it came from (for the
/// audit log; none for a read).
#[derive(Debug, Clone)]
pub struct Admin(pub Session, pub Option<String>);

impl Admin {
    /// Records a change this admin made: the audit log's entry and its
    /// journal line.
    pub async fn record(&self, state: &AppState, what: impl AsRef<str>) {
        crate::audit::record(&state.pool, &self.0.login, self.1.as_deref(), what.as_ref()).await;
    }

    /// Refuses unless the admin holds `function`.
    pub fn require(&self, function: Function) -> Result<(), ApiError> {
        if self.0.grant.may(function) {
            return Ok(());
        }
        Err(ApiError::Forbidden(format!(
            "\"{}\" is off for {}: a super admin can turn it on in Accounts",
            function.shown().1,
            self.0.login
        )))
    }

    /// Refuses unless the admin is a super admin.
    pub fn require_super(&self) -> Result<(), ApiError> {
        if self.0.grant.role == Role::Super {
            return Ok(());
        }
        Err(ApiError::Forbidden(format!(
            "only a super admin may do this; {} is an associate",
            self.0.login
        )))
    }
}

impl FromRequestParts<AppState> for Admin {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Admin, ApiError> {
        let signed_out = || ApiError::Unauthorized("sign in as an admin first".into());
        let token = token(&parts.headers).ok_or_else(signed_out)?;
        let session = sessions::lookup(&state.pool, &token, !polled(parts))
            .await?
            .ok_or_else(signed_out)?;
        if reads_only(&parts.method) {
            return Ok(Admin(session, None));
        }
        if !csrf_ok(&parts.headers, &session) {
            return Err(ApiError::Forbidden(format!(
                "a change needs the session's {CSRF_HEADER} header"
            )));
        }
        let Client(address) = Client::from_request_parts(parts, state).await?;
        let admin = Admin(session, Some(address.to_string()));
        let path = parts
            .extensions
            .get::<MatchedPath>()
            .map_or("", MatchedPath::as_str);
        match needs::need(parts.method.as_str(), path) {
            Need::Function(function) => admin.require(function)?,
            Need::Super => admin.require_super()?,
            Need::Inside => {}
        }
        Ok(admin)
    }
}

/// The admin who sent the request, if one did: for a change, only with the
/// session's CSRF token, so another site cannot act in an admin's name.
#[derive(Debug, Clone)]
pub struct MaybeAdmin(pub Option<Session>);

impl FromRequestParts<AppState> for MaybeAdmin {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<MaybeAdmin, ApiError> {
        let Some(token) = token(&parts.headers) else {
            return Ok(MaybeAdmin(None));
        };
        let session = sessions::find(&state.pool, &token)
            .await?
            .filter(|session| reads_only(&parts.method) || csrf_ok(&parts.headers, session));
        Ok(MaybeAdmin(session))
    }
}

#[derive(Debug, Deserialize)]
pub struct SignIn {
    pub login: String,
    pub password: String,
}

/// `POST /api/auth/login`: a password sign-in. The answer for a wrong
/// password is the same as for a name that has no account.
pub async fn login(
    State(state): State<AppState>,
    Client(address): Client,
    Json(body): Json<SignIn>,
) -> Result<Response, ApiError> {
    if !accounts::password_signin(&state.pool).await? {
        return Err(ApiError::Forbidden("password sign-in is off".into()));
    }
    // A longer name cannot be an account's; it is kept short in the counts.
    let name: String = body.login.chars().take(accounts::MAX_LOGIN).collect();
    let address = address.to_string();
    // One attempt at a time for a name and for an address, on every host:
    // the check, the password and the record of a failure run in this
    // transaction, so parallel guesses cannot all pass the check before
    // their failures are counted. A second attempt does not wait.
    let mut tx = state.pool.begin().await?;
    let free: bool = sqlx::query_scalar(
        "SELECT pg_try_advisory_xact_lock(hashtext('scannerpulse sign-in name ' || $1)) \
            AND pg_try_advisory_xact_lock(hashtext('scannerpulse sign-in address ' || $2))",
    )
    .bind(&name)
    .bind(&address)
    .fetch_one(&mut *tx)
    .await?;
    let too_many = |why: String| ApiError::Limit(StatusCode::TOO_MANY_REQUESTS, why);
    if !free {
        return Err(too_many(
            "a sign-in for this name or from this address is under way: try again".into(),
        ));
    }
    if sessions::locked(&mut *tx, Kind::Name, &name).await?
        || sessions::locked(&mut *tx, Kind::Address, &address).await?
    {
        return Err(too_many(format!(
            "too many failed sign-ins: wait {} minutes",
            sessions::WINDOW_MINUTES
        )));
    }
    // Each check holds about 19 MiB for tens of milliseconds: a few at once.
    let permit = state
        .sign_ins
        .clone()
        .try_acquire_owned()
        .map_err(|_| too_many("too many sign-ins at once: try again".into()))?;
    let account = accounts::by_login(&mut *tx, &name).await?;
    let known = account.is_some();
    let password = body.password;
    // Hashing takes tens of milliseconds of one core: not on the async threads.
    let admin = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        match account {
            Some((id, hash, disabled)) if !hash.is_empty() => {
                (accounts::verify_password(&hash, &password) && !disabled).then_some(id)
            }
            _ => {
                accounts::verify_nothing(&password);
                None
            }
        }
    })
    .await
    .map_err(|error| ApiError::Internal(format!("password check: {error}")))?;
    let Some(admin) = admin else {
        sessions::failed(&mut tx, &name, &address).await?;
        tx.commit().await?;
        // A mistyped password can land in the name field: only an
        // account's name is logged.
        if known {
            crate::log::info(format!("failed sign-in as {name} from {address}"));
        } else {
            crate::log::info(format!(
                "failed sign-in with an unknown name from {address}"
            ));
        }
        return Err(ApiError::Unauthorized(
            "the name or the password is wrong".into(),
        ));
    };
    let (token, _) = sessions::create(&mut tx, admin, false).await?;
    let session = sessions::lookup(&mut *tx, &token, false)
        .await?
        .ok_or_else(|| ApiError::Internal("the new session is missing".into()))?;
    tx.commit().await?;
    crate::audit::record(
        &state.pool,
        &name,
        Some(&address),
        "signed in with a password",
    )
    .await;
    Ok(signed_in(&token, &session))
}

/// The `Set-Cookie` value of a new session.
pub fn session_cookie(token: &str) -> String {
    format!(
        "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={}",
        i64::from(sessions::MAX_DAYS) * 86_400
    )
}

/// Who the session is, for the admin page: the login, the CSRF token, the
/// role and the functions on (every one for a super admin).
fn whoami(session: &Session) -> serde_json::Value {
    json!({
        "login": session.login,
        "csrf": session.csrf,
        "role": session.grant.role,
        "functions": session.grant.names(),
    })
}

/// The answer that sets the session's cookie.
pub fn signed_in(token: &str, session: &Session) -> Response {
    let mut response = Json(whoami(session)).into_response();
    if let Ok(value) = HeaderValue::from_str(&session_cookie(token)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// `POST /api/auth/logout`: ends the session, if there is one.
pub async fn logout(
    State(state): State<AppState>,
    Client(address): Client,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = token(&headers) {
        let session = sessions::lookup(&state.pool, &token, false).await?;
        sessions::end(&state.pool, &token).await?;
        if let Some(session) = session {
            let address = address.to_string();
            crate::audit::record(&state.pool, &session.login, Some(&address), "signed out").await;
        }
    }
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_static(
            "scannerpulse_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
        ),
    );
    Ok(response)
}

/// `GET /api/auth/me`: the signed-in admin, the session's CSRF token, and
/// what the admin may change.
pub async fn me(Admin(session, _): Admin) -> Json<serde_json::Value> {
    Json(whoami(&session))
}

/// `GET /api/auth/methods`: the ways to sign in, for the sign-in page.
pub async fn methods(State(state): State<AppState>) -> Result<Json<serde_json::Value>, ApiError> {
    Ok(Json(json!({
        "password": accounts::password_signin(&state.pool).await?,
        "oidc": super::oidc::provider(&state).await?.is_some(),
    })))
}
