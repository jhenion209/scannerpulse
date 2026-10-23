//! Signing in through an OpenID Connect provider: the authorization code
//! flow with PKCE. The ID token's signature (by a key the issuer
//! publishes), issuer, audience, expiry and nonce are checked, and its
//! `groups` claim must hold the admin group at every sign-in.

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::client::Client;
use super::{ApiError, AppState, auth};
use crate::sessions::{self, random_hex};

/// The cookie that carries the sign-in's state, nonce and PKCE verifier
/// from the start to the callback.
pub const STATE_COOKIE: &str = "scannerpulse_oidc";
/// How long a sign-in may take at the provider.
const STATE_SECONDS: u32 = 600;
/// The largest answer read from the provider.
const MAX_ANSWER: usize = 1 << 20;
/// The signatures accepted: the issuer's own keys only, never a shared
/// secret and never none.
const ALGORITHMS: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

/// The provider, when every OIDC setting and the client secret are set.
#[derive(Clone)]
pub struct Provider {
    pub issuer: String,
    pub client_id: String,
    pub admin_group: String,
    pub redirect_url: String,
    pub secret: String,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .field("admin_group", &self.admin_group)
            .field("redirect_url", &self.redirect_url)
            .field("secret", &"<hidden>")
            .finish()
    }
}

pub async fn provider(state: &AppState) -> Result<Option<Provider>, ApiError> {
    let Some(secret) = state.oidc_secret.clone() else {
        return Ok(None);
    };
    let values = crate::settings::all(&state.pool).await?;
    let text = |key: &str| {
        values
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    Ok((|| {
        Some(Provider {
            issuer: text("oidc.issuer")?,
            client_id: text("oidc.client_id")?,
            admin_group: text("oidc.admin_group")?,
            redirect_url: text("oidc.redirect_url")?,
            secret,
        })
    })())
}

#[derive(Debug, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

/// The provider could not be used; the text is logged, the visitor told
/// only that.
fn provider_failed(why: String) -> ApiError {
    crate::log::error(format!("oidc: {why}"));
    ApiError::Unavailable("the sign-in provider did not answer as expected".into())
}

async fn read_json<T: DeserializeOwned>(answer: reqwest::Response) -> Result<T, ApiError> {
    let url = answer.url().to_string();
    if !answer.status().is_success() {
        return Err(provider_failed(format!(
            "{url} answered {}",
            answer.status()
        )));
    }
    if answer
        .content_length()
        .is_some_and(|n| n > MAX_ANSWER as u64)
    {
        return Err(provider_failed(format!("{url} answered too much")));
    }
    let body = answer
        .bytes()
        .await
        .map_err(|error| provider_failed(format!("{url}: {error}")))?;
    if body.len() > MAX_ANSWER {
        return Err(provider_failed(format!("{url} answered too much")));
    }
    serde_json::from_slice(&body).map_err(|error| provider_failed(format!("{url}: {error}")))
}

async fn get_json<T: DeserializeOwned>(state: &AppState, url: &str) -> Result<T, ApiError> {
    let answer = state
        .http
        .get(url)
        .send()
        .await
        .map_err(|error| provider_failed(format!("{url}: {error}")))?;
    read_json(answer).await
}

async fn discover(state: &AppState, provider: &Provider) -> Result<Discovery, ApiError> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        provider.issuer.trim_end_matches('/')
    );
    let found: Discovery = get_json(state, &url).await?;
    if found.issuer != provider.issuer {
        return Err(provider_failed(format!(
            "the provider calls itself {:?}, not {:?}",
            found.issuer, provider.issuer
        )));
    }
    Ok(found)
}

fn not_set_up() -> ApiError {
    ApiError::Missing("sign-in through the OIDC provider is not set up".into())
}

/// `GET /api/auth/oidc`: sends the browser to the provider.
pub async fn start(State(state): State<AppState>) -> Result<Response, ApiError> {
    let provider = provider(&state).await?.ok_or_else(not_set_up)?;
    let found = discover(&state, &provider).await?;
    let (csrf_state, nonce, verifier) = (random_hex(16), random_hex(16), random_hex(32));
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = reqwest::Url::parse(&found.authorization_endpoint)
        .map_err(|error| provider_failed(format!("authorization endpoint: {error}")))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &provider.client_id)
        .append_pair("redirect_uri", &provider.redirect_url)
        .append_pair("scope", "openid profile email")
        .append_pair("state", &csrf_state)
        .append_pair("nonce", &nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    let cookie = format!(
        "{STATE_COOKIE}={csrf_state}.{nonce}.{verifier}; Path=/api/auth/oidc; HttpOnly; Secure; \
         SameSite=Lax; Max-Age={STATE_SECONDS}"
    );
    redirect(url.as_str(), &[cookie])
}

fn redirect(to: &str, cookies: &[String]) -> Result<Response, ApiError> {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let location = HeaderValue::from_str(to)
        .map_err(|_| provider_failed(format!("{to:?} is not an address")))?;
    response.headers_mut().insert(header::LOCATION, location);
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    Ok(response)
}

/// The sign-in's state, nonce and verifier, from its cookie.
fn pending(headers: &HeaderMap) -> Option<(String, String, String)> {
    let value = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(STATE_COOKIE)?.strip_prefix('='))?;
    let mut parts = value.splitn(3, '.').map(str::to_string);
    let (state, nonce, verifier) = (parts.next()?, parts.next()?, parts.next()?);
    (!state.is_empty() && !nonce.is_empty() && !verifier.is_empty())
        .then_some((state, nonce, verifier))
}

#[derive(Debug, Deserialize)]
pub struct Callback {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    id_token: String,
}

#[derive(Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub nonce: Option<String>,
    #[serde(default)]
    pub groups: Vec<String>,
    pub preferred_username: Option<String>,
}

/// The ID token's claims, when its signature is by one of the issuer's
/// keys and it was issued by the issuer, for this client, has not expired
/// and carries the sign-in's nonce.
pub fn verify(
    id_token: &str,
    keys: &JwkSet,
    provider: &Provider,
    nonce: &str,
) -> Result<Claims, String> {
    let head = jsonwebtoken::decode_header(id_token).map_err(|e| format!("token: {e}"))?;
    if !ALGORITHMS.contains(&head.alg) {
        return Err(format!(
            "a token signed with {:?} is not accepted",
            head.alg
        ));
    }
    let key = match &head.kid {
        Some(kid) => keys.find(kid),
        None if keys.keys.len() == 1 => keys.keys.first(),
        None => None,
    }
    .ok_or("the token names no key the issuer publishes")?;
    let key = DecodingKey::from_jwk(key).map_err(|e| format!("key: {e}"))?;
    let mut rules = Validation::new(head.alg);
    rules.set_issuer(&[&provider.issuer]);
    rules.set_audience(&[&provider.client_id]);
    rules.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    let claims = jsonwebtoken::decode::<Claims>(id_token, &key, &rules)
        .map_err(|e| format!("token: {e}"))?
        .claims;
    if claims.nonce.as_deref() != Some(nonce) {
        return Err("the token's nonce is not this sign-in's".into());
    }
    Ok(claims)
}

/// `GET /api/auth/oidc/callback`: the provider sends the browser back here,
/// and a signed-in admin goes on to the admin page.
pub async fn callback(
    State(state): State<AppState>,
    Client(address): Client,
    Query(query): Query<Callback>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(error) = &query.error {
        let error: String = error
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
            .take(64)
            .collect();
        return Err(ApiError::Forbidden(format!(
            "the provider refused: {error}"
        )));
    }
    let provider = provider(&state).await?.ok_or_else(not_set_up)?;
    let (expected, nonce, verifier) = pending(&headers)
        .ok_or_else(|| ApiError::BadRequest("no sign-in is under way: start again".into()))?;
    let (Some(code), Some(returned)) = (query.code, query.state) else {
        return Err(ApiError::BadRequest("the provider sent no code".into()));
    };
    if !auth::same(&returned, &expected) {
        return Err(ApiError::BadRequest(
            "this answer is not for the sign-in under way: start again".into(),
        ));
    }
    let found = discover(&state, &provider).await?;
    let answer = state
        .http
        .post(&found.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", provider.redirect_url.as_str()),
            ("client_id", provider.client_id.as_str()),
            ("client_secret", provider.secret.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await
        .map_err(|error| provider_failed(format!("token endpoint: {error}")))?;
    let tokens: Tokens = read_json(answer).await?;
    let keys: JwkSet = get_json(&state, &found.jwks_uri).await?;
    let claims = verify(&tokens.id_token, &keys, &provider, &nonce).map_err(|why| {
        crate::log::info(format!("refused an OIDC sign-in from {address}: {why}"));
        ApiError::Forbidden("the provider's answer could not be trusted".into())
    })?;
    if !claims.groups.iter().any(|g| g == &provider.admin_group) {
        crate::log::info(format!(
            "refused an OIDC sign-in from {address}: {} is not in {}",
            claims.sub, provider.admin_group
        ));
        return Err(ApiError::Forbidden(format!(
            "only members of {} may sign in",
            provider.admin_group
        )));
    }
    let mut conn = state.pool.acquire().await?;
    let (admin, login, disabled) = crate::accounts::oidc_account(
        &mut conn,
        &provider.issuer,
        &claims.sub,
        claims.preferred_username.as_deref(),
    )
    .await?;
    if disabled {
        return Err(ApiError::Forbidden("this account is disabled".into()));
    }
    let (token, _) = sessions::create(&mut conn, admin, true).await?;
    let from = address.to_string();
    crate::audit::record(
        &mut *conn,
        &login,
        Some(&from),
        "signed in through single sign-on",
    )
    .await;
    let ended =
        format!("{STATE_COOKIE}=; Path=/api/auth/oidc; HttpOnly; Secure; SameSite=Lax; Max-Age=0");
    redirect("/admin", &[auth::session_cookie(&token), ended])
}
