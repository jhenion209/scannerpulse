//! Shared by the integration tests. All values are invented.
#![allow(dead_code)]

use std::path::Path;

use sqlx::PgPool;

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
