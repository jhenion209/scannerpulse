mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::send;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use scannerpulse::web::oidc::{self, Provider};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

const SECRET: &str = "client secret for tests";
const CLIENT: &str = "scannerpulse";
const GROUP: &str = "scannerpulse-admins";

/// A signing key made for the test, and its public half as a JWK.
struct Key {
    der: Vec<u8>,
    jwk: Value,
}

fn key(kid: &str) -> Key {
    let pkcs8 =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .unwrap();
    let pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        pkcs8.as_ref(),
        &SystemRandom::new(),
    )
    .unwrap();
    let point = pair.public_key().as_ref();
    Key {
        der: pkcs8.as_ref().to_vec(),
        jwk: json!({
            "kty": "EC", "crv": "P-256", "kid": kid, "use": "sig", "alg": "ES256",
            "x": URL_SAFE_NO_PAD.encode(&point[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&point[33..65]),
        }),
    }
}

fn sign(key: &Key, kid: &str, claims: &Value) -> String {
    let mut header = Header::new(Algorithm::ES256);
    header.kid = Some(kid.into());
    jsonwebtoken::encode(&header, claims, &EncodingKey::from_ec_der(&key.der)).unwrap()
}

/// What the fake provider puts in the ID token, and what it saw.
#[derive(Default)]
struct Seen {
    claims: Value,
    challenge: String,
    token_requests: Vec<HashMap<String, String>>,
}

#[derive(Clone)]
struct Fake {
    issuer: String,
    key: Arc<Key>,
    seen: Arc<Mutex<Seen>>,
}

async fn discovery(State(fake): State<Fake>) -> impl IntoResponse {
    axum::Json(json!({
        "issuer": fake.issuer,
        "authorization_endpoint": format!("{}/authorize", fake.issuer),
        "token_endpoint": format!("{}/token", fake.issuer),
        "jwks_uri": format!("{}/jwks", fake.issuer),
    }))
}

async fn jwks(State(fake): State<Fake>) -> impl IntoResponse {
    axum::Json(json!({"keys": [fake.key.jwk]}))
}

async fn token(State(fake): State<Fake>, body: String) -> impl IntoResponse {
    let form: HashMap<String, String> = reqwest::Url::parse(&format!("http://form/?{body}"))
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect();
    let mut seen = fake.seen.lock().unwrap();
    seen.token_requests.push(form.clone());
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let pkce = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == seen.challenge;
    if form.get("code").map(String::as_str) != Some("the-code")
        || form.get("client_secret").map(String::as_str) != Some(SECRET)
        || !pkce
    {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": "invalid_grant"})),
        )
            .into_response();
    }
    axum::Json(json!({"id_token": sign(&fake.key, "k1", &seen.claims), "token_type": "Bearer"}))
        .into_response()
}

async fn provider_on_a_port() -> Fake {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fake = Fake {
        issuer: format!("http://{}", listener.local_addr().unwrap()),
        key: Arc::new(key("k1")),
        seen: Arc::default(),
    };
    let app = axum::Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/token", post(token))
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    fake
}

async fn set_up(pool: &PgPool, fake: &Fake) -> axum::Router {
    for (key, value) in [
        ("oidc.issuer", fake.issuer.clone()),
        ("oidc.client_id", CLIENT.into()),
        ("oidc.admin_group", GROUP.into()),
        (
            "oidc.redirect_url",
            "https://sp.example/api/auth/oidc/callback".into(),
        ),
    ] {
        sqlx::query("INSERT INTO settings (key, value) VALUES ($1, to_jsonb($2::text))")
            .bind(key)
            .bind(value)
            .execute(pool)
            .await
            .unwrap();
    }
    let mut state = scannerpulse::web::AppState::new(pool.clone());
    state.oidc_secret = Some(SECRET.into());
    scannerpulse::web::router(state)
}

fn claims(fake: &Fake, sub: &str, nonce: &str, groups: &[&str]) -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "iss": fake.issuer, "aud": CLIENT, "sub": sub, "iat": now, "exp": now + 300,
        "nonce": nonce, "groups": groups, "preferred_username": "ann",
    })
}

/// Starts a sign-in; returns the authorization address's parameters and the
/// sign-in's cookie.
async fn start(app: &axum::Router) -> (HashMap<String, String>, String) {
    let started = send(app, "GET", "/api/auth/oidc", &[], None).await;
    assert_eq!(started.status, 303, "{:?}", started.body);
    let location = reqwest::Url::parse(started.headers["location"].to_str().unwrap()).unwrap();
    let params: HashMap<String, String> = location.query_pairs().into_owned().collect();
    let cookie = started.headers["set-cookie"].to_str().unwrap();
    assert!(
        cookie.contains("HttpOnly") && cookie.contains("Path=/api/auth/oidc"),
        "{cookie}"
    );
    (params, cookie.split(';').next().unwrap().to_string())
}

/// Signs in as `sub` in `groups`; returns the callback's answer.
async fn sign_in(app: &axum::Router, fake: &Fake, sub: &str, groups: &[&str]) -> common::Reply {
    let (params, cookie) = start(app).await;
    {
        let mut seen = fake.seen.lock().unwrap();
        seen.challenge = params["code_challenge"].clone();
        seen.claims = claims(fake, sub, &params["nonce"], groups);
    }
    let uri = format!(
        "/api/auth/oidc/callback?code=the-code&state={}",
        params["state"]
    );
    send(app, "GET", &uri, &[("cookie", &cookie)], None).await
}

#[sqlx::test(migrations = "./migrations")]
async fn a_member_of_the_group_signs_in_and_gets_an_account(pool: PgPool) {
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    assert_eq!(
        send(&app, "GET", "/api/auth/methods", &[], None).await.body["oidc"],
        true
    );
    let (params, _) = start(&app).await;
    assert_eq!(params["client_id"], CLIENT);
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["response_type"], "code");
    assert_eq!(
        params["redirect_uri"],
        "https://sp.example/api/auth/oidc/callback"
    );
    let back = sign_in(&app, &fake, "user-1", &["staff", GROUP]).await;
    assert_eq!(back.status, 303, "{:?}", back.body);
    assert_eq!(back.headers["location"], "/admin");
    let cookies: Vec<&str> = back
        .headers
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect();
    let session = cookies
        .iter()
        .find(|c| c.starts_with("scannerpulse_session="))
        .unwrap();
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("scannerpulse_oidc=;") && c.contains("Max-Age=0"))
    );
    let pair = session.split(';').next().unwrap();
    let me = send(&app, "GET", "/api/auth/me", &[("cookie", pair)], None).await;
    assert_eq!(me.body["login"], "ann");
    let account: (String, Option<String>, Option<String>, String) =
        sqlx::query_as("SELECT login, oidc_issuer, oidc_subject, password_hash FROM admins")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        account,
        (
            "ann".into(),
            Some(fake.issuer.clone()),
            Some("user-1".into()),
            String::new()
        )
    );
    // The token request carried the secret, the code and the verifier.
    let request = fake.seen.lock().unwrap().token_requests[0].clone();
    assert_eq!(request["grant_type"], "authorization_code");
    assert_eq!(request["client_id"], CLIENT);
    // A second sign-in finds the same account.
    assert_eq!(sign_in(&app, &fake, "user-1", &[GROUP]).await.status, 303);
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(accounts, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn someone_outside_the_group_is_refused(pool: PgPool) {
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    let back = sign_in(&app, &fake, "user-2", &["staff"]).await;
    assert_eq!(back.status, 403);
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM admins")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(accounts, 0);
    // Taken out of the group, an admin loses access at the next sign-in.
    assert_eq!(sign_in(&app, &fake, "user-3", &[GROUP]).await.status, 303);
    assert_eq!(sign_in(&app, &fake, "user-3", &[]).await.status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_local_account_of_the_same_name_is_not_taken_over(pool: PgPool) {
    common::admin(&pool, "ann", "correct horse battery").await;
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    assert_eq!(sign_in(&app, &fake, "user-1", &[GROUP]).await.status, 303);
    let accounts: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT login, oidc_subject FROM admins ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(accounts[0], ("ann".into(), None));
    assert!(accounts[1].0.starts_with("ann-"), "{accounts:?}");
    assert_eq!(accounts[1].1.as_deref(), Some("user-1"));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_disabled_account_is_refused(pool: PgPool) {
    common::admin(&pool, "bob", "correct horse battery").await;
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    assert_eq!(sign_in(&app, &fake, "user-1", &[GROUP]).await.status, 303);
    sqlx::query("UPDATE admins SET disabled = true WHERE oidc_subject = 'user-1'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(sign_in(&app, &fake, "user-1", &[GROUP]).await.status, 403);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_answer_for_another_sign_in_is_refused(pool: PgPool) {
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    let (params, cookie) = start(&app).await;
    let wrong = send(
        &app,
        "GET",
        "/api/auth/oidc/callback?code=the-code&state=0000",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(wrong.status, 400);
    let uri = format!(
        "/api/auth/oidc/callback?code=the-code&state={}",
        params["state"]
    );
    assert_eq!(
        send(&app, "GET", &uri, &[], None).await.status,
        400,
        "no sign-in cookie"
    );
    let refused = send(
        &app,
        "GET",
        "/api/auth/oidc/callback?error=access_denied",
        &[],
        None,
    )
    .await;
    assert_eq!(refused.status, 403);
    // A wrong PKCE verifier: the provider refuses the code.
    fake.seen.lock().unwrap().challenge = "not the challenge".into();
    assert_eq!(
        send(&app, "GET", &uri, &[("cookie", &cookie)], None)
            .await
            .status,
        503
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn without_its_secret_oidc_is_off(pool: PgPool) {
    let app = common::app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/auth/oidc", &[], None).await.status,
        404
    );
    assert_eq!(
        send(&app, "GET", "/api/auth/methods", &[], None).await.body["oidc"],
        false
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_oidc_session_ends_twelve_hours_after_sign_in(pool: PgPool) {
    let fake = provider_on_a_port().await;
    let app = set_up(&pool, &fake).await;
    let back = sign_in(&app, &fake, "user-1", &[GROUP]).await;
    assert_eq!(back.status, 303, "{:?}", back.body);
    let pair = back
        .headers
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|c| c.starts_with("scannerpulse_session="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    // Still in use: the idle limit is fresh, but the sign-in is 12 hours and a minute old.
    sqlx::query(
        "UPDATE sessions SET created_at = now() - interval '12 hours 1 minute', last_seen = now() \
         WHERE oidc",
    )
    .execute(&pool)
    .await
    .unwrap();
    let me = send(&app, "GET", "/api/auth/me", &[("cookie", &pair)], None).await;
    assert_eq!(me.status, 401, "{:?}", me.body);
}

// The ID token's checks, one at a time.

fn provider(issuer: &str) -> Provider {
    Provider {
        issuer: issuer.into(),
        client_id: CLIENT.into(),
        admin_group: GROUP.into(),
        redirect_url: "https://sp.example/cb".into(),
        secret: SECRET.into(),
    }
}

#[test]
fn a_token_is_trusted_only_when_every_check_passes() {
    let issuer = "https://id.example";
    let fake = Fake {
        issuer: issuer.into(),
        key: Arc::new(key("k1")),
        seen: Arc::default(),
    };
    let keys: JwkSet = serde_json::from_value(json!({"keys": [fake.key.jwk]})).unwrap();
    let p = provider(issuer);
    let good = claims(&fake, "user-1", "n1", &[GROUP]);
    assert_eq!(
        oidc::verify(&sign(&fake.key, "k1", &good), &keys, &p, "n1")
            .unwrap()
            .sub,
        "user-1"
    );
    let changed = |field: &str, value: Value| {
        let mut c = good.clone();
        c[field] = value;
        sign(&fake.key, "k1", &c)
    };
    let now = chrono::Utc::now().timestamp();
    for (why, token) in [
        ("another nonce", sign(&fake.key, "k1", &good)),
        ("another audience", changed("aud", json!("someone-else"))),
        (
            "another issuer",
            changed("iss", json!("https://evil.example")),
        ),
        ("expired", changed("exp", json!(now - 3600))),
        ("an unknown key id", sign(&fake.key, "k2", &good)),
        ("another key", sign(&key("k1"), "k1", &good)),
    ] {
        let nonce = if why == "another nonce" { "n2" } else { "n1" };
        assert!(oidc::verify(&token, &keys, &p, nonce).is_err(), "{why}");
    }
    // A token signed with a shared secret is never accepted.
    let hs = jsonwebtoken::encode(
        &Header {
            kid: Some("k1".into()),
            ..Header::new(Algorithm::HS256)
        },
        &good,
        &EncodingKey::from_secret(b"guessable"),
    )
    .unwrap();
    assert!(oidc::verify(&hs, &keys, &p, "n1").is_err());
}
