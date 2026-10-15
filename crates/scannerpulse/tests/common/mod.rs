//! Shared by the integration tests. All values are invented.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use http_body_util::BodyExt;
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use tower::ServiceExt;

/// The primary location and one source; returns their ids.
pub async fn seed_places(pool: &PgPool, root: &Path, source: &Path) -> (i16, i16) {
    let location: i16 = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode, is_primary) \
         VALUES ('primary', $1, 'managed', true) RETURNING id",
    )
    .bind(root.to_str().unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    let source: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path) VALUES ('live', 'sdrtrunk', $1) RETURNING id",
    )
    .bind(source.to_str().unwrap())
    .fetch_one(pool)
    .await
    .unwrap();
    (location, source)
}

/// Gives the processing lease to `holder` for a minute.
pub async fn hold_lease(pool: &PgPool, holder: &str) {
    sqlx::query(
        "INSERT INTO leases (name, holder, expires_at, renewed_at) \
         VALUES ('processing', $1, now() + interval '1 minute', now()) \
         ON CONFLICT (name) DO UPDATE SET holder = EXCLUDED.holder, expires_at = EXCLUDED.expires_at",
    )
    .bind(holder)
    .execute(pool)
    .await
    .unwrap();
}

/// The header of an MPEG-2 layer III frame: 32 kbit/s, 16 kHz, mono, no CRC.
/// The frame is 144 bytes long and holds 576 samples, 36 ms.
pub const FRAME_HEADER: [u8; 4] = [0xFF, 0xF3, 0x48, 0xC4];
pub const FRAME_LEN: usize = 144;

/// One silent frame of that kind.
pub fn frame() -> Vec<u8> {
    let mut frame = vec![0u8; FRAME_LEN];
    frame[..4].copy_from_slice(&FRAME_HEADER);
    frame
}

/// `n` such frames, 36 ms each.
pub fn frames(n: usize) -> Vec<u8> {
    frame().repeat(n)
}

/// A file: the tag, then `n` frames.
pub fn tagged(tag: &id3::Tag, n: usize) -> Vec<u8> {
    let mut data = Vec::new();
    tag.write_to(&mut data, id3::Version::Id3v24).unwrap();
    data.extend_from_slice(&frames(n));
    data
}

/// An ID3v2.4 header for a tag body of `body` bytes, which the caller appends.
pub fn id3v2_header(body: usize) -> Vec<u8> {
    let size = [
        ((body >> 21) & 0x7F) as u8,
        ((body >> 14) & 0x7F) as u8,
        ((body >> 7) & 0x7F) as u8,
        (body & 0x7F) as u8,
    ];
    let mut header = b"ID3\x04\x00\x00".to_vec();
    header.extend_from_slice(&size);
    header
}

/// A TCP proxy to the test database that can freeze: the connections open at
/// the freeze keep reading but drop the bytes and never answer, as through a
/// network that lost its way; connections opened later forward normally. TLS
/// passes through unchanged.
pub struct Proxy {
    port: u16,
    freezes: Arc<AtomicU64>,
}

impl Proxy {
    pub fn start(pool: &PgPool) -> Proxy {
        let options = pool.connect_options();
        let upstream = format!("{}:{}", options.get_host(), options.get_port());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let freezes = Arc::new(AtomicU64::new(0));
        let counter = freezes.clone();
        thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { continue };
                let Ok(server) = TcpStream::connect(&upstream) else {
                    continue;
                };
                let born = counter.load(Ordering::SeqCst);
                for (from, to) in [
                    (client.try_clone().unwrap(), server.try_clone().unwrap()),
                    (server, client),
                ] {
                    let counter = counter.clone();
                    thread::spawn(move || forward(from, to, &counter, born));
                }
            }
        });
        Proxy { port, freezes }
    }

    /// The test pool's options, through the proxy.
    pub fn options(&self, pool: &PgPool) -> PgConnectOptions {
        (*pool.connect_options())
            .clone()
            .host("127.0.0.1")
            .port(self.port)
    }

    /// Every connection open now stops answering.
    pub fn freeze(&self) {
        self.freezes.fetch_add(1, Ordering::SeqCst);
    }
}

fn forward(mut from: TcpStream, mut to: TcpStream, freezes: &AtomicU64, born: u64) {
    let mut buf = [0u8; 16384];
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if freezes.load(Ordering::SeqCst) > born {
            continue;
        }
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(Shutdown::Write);
}

/// A listener that accepts connections and never says a word; its port.
pub fn silent_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut held = Vec::new();
        for stream in listener.incoming() {
            held.push(stream);
        }
    });
    port
}

/// What the fake transcription server answers on one connection.
#[derive(Debug, Clone)]
pub enum Answer {
    /// 200 with `{"text": ...}`.
    Text(&'static str),
    /// This status, with this body.
    Status(u16, &'static str),
    /// 200 with this body as it is.
    Raw(&'static str),
    /// Reads the request and never answers.
    Silent,
}

/// A fake OpenAI-compatible transcription server. Each connection gets the
/// next answer of the script, and the last answer repeats. It keeps every
/// request it read.
pub struct FakeAsr {
    pub url: String,
    requests: Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
}

impl FakeAsr {
    pub fn start(script: Vec<Answer>) -> FakeAsr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = requests.clone();
        thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { continue };
                let answer = script[n.min(script.len() - 1)].clone();
                let kept = kept.clone();
                thread::spawn(move || answer_one(stream, &answer, &kept));
            }
        });
        FakeAsr {
            url: format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
            requests,
        }
    }

    /// The requests read so far, head and body.
    pub fn requests(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| String::from_utf8_lossy(r).into_owned())
            .collect()
    }
}

fn answer_one(mut stream: TcpStream, answer: &Answer, kept: &std::sync::Mutex<Vec<Vec<u8>>>) {
    let mut request = Vec::new();
    let mut buf = [0u8; 8192];
    let body_starts = loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        request.extend_from_slice(&buf[..n]);
        if let Some(at) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&request[..body_starts]).to_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    while request.len() < body_starts + length {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&buf[..n]),
        }
    }
    kept.lock().unwrap().push(request);
    let (status, body) = match answer {
        Answer::Text(text) => (200, serde_json::json!({ "text": text }).to_string()),
        Answer::Status(status, body) => (*status, body.to_string()),
        Answer::Raw(body) => (200, body.to_string()),
        Answer::Silent => {
            thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
    };
    let reply = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(reply.as_bytes());
}

/// A transcription URL where nothing listens.
pub fn refused_url() -> String {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    format!("http://127.0.0.1:{port}/v1/audio/transcriptions")
}

/// A fence whose lease the test can take away.
pub struct HeldFence {
    pub held: std::sync::atomic::AtomicBool,
    pub holder: String,
}

impl HeldFence {
    pub fn new(holder: &str) -> Arc<HeldFence> {
        Arc::new(HeldFence {
            held: std::sync::atomic::AtomicBool::new(true),
            holder: holder.to_string(),
        })
    }
}

impl scannerpulse::filer::Fence for HeldFence {
    fn held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
    fn holder(&self) -> &str {
        &self.holder
    }
}

/// System SYSA, its talkgroup 127 (transcription on or off) and one call of
/// it at `started_at`, `duration_ms` long, whose file is `rel_path` in the
/// location. Returns the call's id and the talkgroup's id.
pub async fn insert_call(
    pool: &PgPool,
    places: (i16, i16),
    rel_path: &str,
    started_at: chrono::DateTime<chrono::Utc>,
    duration_ms: i32,
    transcribe: bool,
) -> (i64, i32) {
    scannerpulse::store::ensure_partition(pool, started_at)
        .await
        .unwrap();
    let system: i32 = sqlx::query_scalar(
        "INSERT INTO systems (name) VALUES ('SYSA') \
         ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let talkgroup: i32 = sqlx::query_scalar(
        "INSERT INTO talkgroups (system_id, number, name, transcribe) VALUES ($1, 127, 'Alpha Fire', $2) \
         ON CONFLICT (system_id, number) DO UPDATE SET transcribe = EXCLUDED.transcribe RETURNING id",
    )
    .bind(system)
    .bind(transcribe)
    .fetch_one(pool)
    .await
    .unwrap();
    let (location, source) = places;
    let call: i64 = sqlx::query_scalar(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
         VALUES ($1, $2, $3, $4, $5, $6) RETURNING id",
    )
    .bind(started_at)
    .bind(duration_ms)
    .bind(talkgroup)
    .bind(source)
    .bind(location)
    .bind(rel_path)
    .fetch_one(pool)
    .await
    .unwrap();
    (call, talkgroup)
}

/// Turns transcription on with one server and the given limits.
pub async fn set_transcription(pool: &PgPool, url: &str, min_ms: i32, timeout_seconds: u64) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES \
         ('transcription.enabled', 'true'), \
         ('transcription.servers', jsonb_build_array(jsonb_build_object('url', $1::text, 'weight', 2))), \
         ('transcription.model', '\"model-x\"'), \
         ('transcription.min_duration_ms', to_jsonb($2::int)), \
         ('transcription.request_timeout_seconds', to_jsonb($3::bigint)) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(url)
    .bind(min_ms)
    .bind(timeout_seconds as i64)
    .execute(pool)
    .await
    .unwrap();
}

// The web tests' helpers.

/// A talkgroup of `system`; returns its id.
pub async fn talkgroup(pool: &PgPool, system: &str, number: i64) -> i32 {
    let system: i32 = sqlx::query_scalar(
        "INSERT INTO systems (name) VALUES ($1) \
         ON CONFLICT (name) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(system)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO talkgroups (system_id, number, name) VALUES ($1, $2, 'Group ' || $2) \
         ON CONFLICT (system_id, number) DO UPDATE SET name = EXCLUDED.name RETURNING id",
    )
    .bind(system)
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A call of `talkgroup` at `at`, with a machine transcript when `text` is set.
pub async fn call(
    pool: &PgPool,
    talkgroup: i32,
    at: chrono::DateTime<chrono::Utc>,
    text: Option<&str>,
) -> i64 {
    let places: (i16, i16) = match sqlx::query_as(
        "SELECT (SELECT id FROM locations LIMIT 1), (SELECT id FROM sources LIMIT 1) \
         WHERE EXISTS (SELECT 1 FROM locations)",
    )
    .fetch_optional(pool)
    .await
    .unwrap()
    {
        Some(found) => found,
        None => {
            seed_places(
                pool,
                std::path::Path::new("/nonexistent/archive"),
                std::path::Path::new("/nonexistent/in"),
            )
            .await
        }
    };
    scannerpulse::store::ensure_partition(pool, at)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO calls (started_at, duration_ms, talkgroup_id, source_id, location_id, rel_path) \
         VALUES ($1, 2000, $2, $3, $4, gen_random_uuid()::text || '.mp3') RETURNING id",
    )
    .bind(at)
    .bind(talkgroup)
    .bind(places.1)
    .bind(places.0)
    .fetch_one(pool)
    .await
    .unwrap();
    if let Some(text) = text {
        sqlx::query(
            "INSERT INTO transcripts (call_id, started_at, text, origin) VALUES ($1, $2, $3, 'machine')",
        )
        .bind(id)
        .bind(at)
        .bind(text)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

pub async fn get(pool: &PgPool, uri: &str) -> (axum::http::StatusCode, serde_json::Value) {
    let app = scannerpulse::web::router(scannerpulse::web::AppState::new(
        pool.clone(),
        (*pool.connect_options()).clone(),
    ));
    let response = app
        .oneshot(
            axum::http::Request::get(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let answer = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    if status.as_u16() >= 400 {
        assert!(
            answer["error"].is_string() && answer["detail"].is_string(),
            "{uri}: an error is {{error, detail}}, not {:?}",
            String::from_utf8_lossy(&body)
        );
    }
    (status, answer)
}

/// The address every request of `app` comes from.
pub const PEER: [u8; 4] = [198, 51, 100, 7];

/// The router, as a host serves it, with requests from `PEER`.
pub fn app(pool: &PgPool) -> axum::Router {
    scannerpulse::web::router(scannerpulse::web::AppState::new(
        pool.clone(),
        (*pool.connect_options()).clone(),
    ))
    .layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from((PEER, 40_000)),
    ))
}

/// What the router answered: the status, the headers and the JSON body
/// (`Null` when there is none).
pub struct Reply {
    pub status: axum::http::StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: serde_json::Value,
}

/// Sends one request with `headers` and, when given, a JSON body. Every
/// error must be `{error, detail}`.
pub async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Option<serde_json::Value>,
) -> Reply {
    let mut request = axum::http::Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = match body {
        Some(body) => request
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string())),
        None => request.body(axum::body::Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    if status.as_u16() >= 400 {
        assert!(
            body["error"].is_string() && body["detail"].is_string(),
            "{method} {uri}: an error is {{error, detail}}, not {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
    Reply {
        status,
        headers,
        body,
    }
}

/// An admin account named `login`, with the password `password`.
pub async fn admin(pool: &PgPool, login: &str, password: &str) {
    let mut conn = pool.acquire().await.unwrap();
    scannerpulse::accounts::add(&mut conn, login, password)
        .await
        .unwrap();
}

/// Signs in; returns the `Cookie` header value and the CSRF token.
pub async fn sign_in(app: &axum::Router, login: &str, password: &str) -> (String, String) {
    let answer = send(
        app,
        "POST",
        "/api/auth/login",
        &[],
        Some(serde_json::json!({"login": login, "password": password})),
    )
    .await;
    assert_eq!(answer.status, 200, "{:?}", answer.body);
    let cookie = answer.headers["set-cookie"].to_str().unwrap();
    let pair = cookie.split(';').next().unwrap().to_string();
    (pair, answer.body["csrf"].as_str().unwrap().to_string())
}
