//! Shared by the integration tests. All values are invented.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;

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
