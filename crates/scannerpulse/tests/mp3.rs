mod common;

use common::{frame, frames, id3v2_header};
use scannerpulse::mp3::{Stream, audio, id3v2_len, stream};

#[test]
fn the_frames_add_up_to_the_length() {
    assert_eq!(
        stream(&frames(25)),
        Some(Stream {
            frames: 25,
            duration_ms: 900
        })
    );
}

#[test]
fn a_file_without_a_frame_has_no_length() {
    assert_eq!(stream(b""), None);
    assert_eq!(stream(b"this is not audio at all"), None);
    let mut tag_only = id3v2_header(20);
    tag_only.extend_from_slice(&[0u8; 20]);
    assert_eq!(stream(&tag_only), None);
}

// Frames inside the tag would be counted if the walk did not skip it.
#[test]
fn the_id3v2_tag_is_skipped() {
    let body = frames(3);
    let mut data = id3v2_header(body.len());
    data.extend_from_slice(&body);
    assert_eq!(id3v2_len(&data), 10 + body.len());
    data.extend_from_slice(&frames(10));
    assert_eq!(stream(&data).map(|s| s.frames), Some(10));
}

#[test]
fn a_tag_that_claims_more_than_the_file_holds_is_not_a_crash() {
    let mut data = id3v2_header(1_000_000);
    data.extend_from_slice(&frames(2));
    assert_eq!(stream(&data), None);
}

#[test]
fn bytes_before_the_first_frame_are_passed_over() {
    let mut data = vec![0x00, 0xFF, 0x12, 0xFF, 0xF3, 0x00, 0x41];
    data.extend_from_slice(&frames(5));
    assert_eq!(stream(&data).map(|s| s.frames), Some(5));
}

#[test]
fn an_id3v1_tag_at_the_end_ends_the_walk() {
    let mut data = frames(5);
    data.extend_from_slice(b"TAG");
    data.extend_from_slice(&[0x20; 125]);
    assert_eq!(stream(&data).map(|s| s.frames), Some(5));
}

#[test]
fn a_cut_off_last_frame_is_not_counted() {
    let mut data = frames(5);
    data.extend_from_slice(&frame()[..50]);
    assert_eq!(stream(&data).map(|s| s.frames), Some(5));
}

#[test]
fn an_info_frame_holds_no_audio() {
    let mut info = frame();
    // MPEG-2, mono, no CRC: the side information ends 13 bytes into the frame.
    info[13..17].copy_from_slice(b"Info");
    let mut data = info;
    data.extend_from_slice(&frames(5));
    assert_eq!(
        stream(&data),
        Some(Stream {
            frames: 5,
            duration_ms: 180
        })
    );
}

/// `n` frames with this header, each `len` bytes long.
fn frames_of(header: [u8; 4], len: usize, n: usize) -> Vec<u8> {
    let mut one = vec![0u8; len];
    one[..4].copy_from_slice(&header);
    one.repeat(n)
}

#[test]
fn the_frame_length_follows_the_version_and_layer() {
    // (header, frame length, length of 3 frames in ms)
    let cases: [([u8; 4], usize, u64); 4] = [
        // MPEG-1 layer III, 128 kbit/s, 44.1 kHz: 1152 samples.
        ([0xFF, 0xFB, 0x90, 0x00], 417, 78),
        // MPEG-1 layer II, 192 kbit/s, 48 kHz: 1152 samples.
        ([0xFF, 0xFD, 0xA4, 0x00], 576, 72),
        // MPEG-1 layer I, 384 kbit/s, 44.1 kHz: 384 samples, length in 4-byte slots.
        ([0xFF, 0xFF, 0xC0, 0x00], 416, 26),
        // MPEG-2.5 layer III, 8 kbit/s, 8 kHz: 576 samples.
        ([0xFF, 0xE3, 0x18, 0xC0], 72, 216),
    ];
    for (header, len, ms) in cases {
        let got = stream(&frames_of(header, len, 3));
        assert_eq!(
            got,
            Some(Stream {
                frames: 3,
                duration_ms: ms
            }),
            "{header:02x?}"
        );
    }
}

#[test]
fn a_padded_frame_is_one_byte_longer() {
    // MPEG-1 layer III, 128 kbit/s, 44.1 kHz, with the padding bit.
    let mut data = frames_of([0xFF, 0xFB, 0x92, 0x00], 418, 2);
    data.extend_from_slice(&frames_of([0xFF, 0xFB, 0x90, 0x00], 417, 2));
    assert_eq!(stream(&data).map(|s| s.frames), Some(4));
}

#[test]
fn the_audio_is_the_frames_without_the_tags() {
    let mut data = id3v2_header(30);
    data.extend_from_slice(&[0u8; 30]);
    data.extend_from_slice(&frames(4));
    // An ID3v1 tag at the end: "TAG" and 125 bytes.
    data.extend_from_slice(b"TAG");
    data.extend_from_slice(&[b' '; 125]);
    assert_eq!(audio(&data), Some(&frames(4)[..]));
    assert_eq!(audio(b"not audio"), None);
    // Two calls' audio, one after the other, is one stream of both.
    let mut joined = audio(&data).unwrap().to_vec();
    joined.extend_from_slice(audio(&frames(3)).unwrap());
    assert_eq!(stream(&joined).map(|s| s.frames), Some(7));
}
