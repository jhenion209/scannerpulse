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
