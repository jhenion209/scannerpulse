//! The length of an MPEG audio stream, from its frames.
//!
//! Every frame of MPEG-1, MPEG-2 and MPEG-2.5 audio, layers I to III, starts
//! with a four-byte header that gives the frame's length in bytes and how many
//! samples it holds. The stream's length is the sum over its frames.

/// What the frames of a stream add up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream {
    /// Audio frames, not counting a leading Xing, Info or VBRI frame.
    pub frames: u64,
    /// The length in milliseconds, rounded.
    pub duration_ms: u64,
}

/// The length of the ID3v2 tag that starts `data`, its header and footer
/// included; 0 when `data` does not start with one.
pub fn id3v2_len(data: &[u8]) -> usize {
    if data.len() < 10 || &data[..3] != b"ID3" {
        return 0;
    }
    let size = &data[6..10];
    // The size is "syncsafe": seven bits per byte, the top bit always clear.
    if size.iter().any(|byte| byte & 0x80 != 0) {
        return 0;
    }
    let body = size
        .iter()
        .fold(0usize, |n, byte| (n << 7) | usize::from(*byte));
    let footer = if data[5] & 0x10 != 0 { 10 } else { 0 };
    10 + body + footer
}

/// The frames after any ID3v2 tags at the start of `data`; `None` when there is
/// no audio frame.
pub fn stream(data: &[u8]) -> Option<Stream> {
    let mut start = 0;
    while let Some(rest) = data.get(start..) {
        match id3v2_len(rest) {
            0 => break,
            len => start += len,
        }
    }
    let mut pos = first_frame(data, start)?;
    if let Some(first) = header(&data[pos..]) {
        if is_info_frame(&data[pos..], &first) {
            pos += first.len;
        }
    }
    let mut frames = 0u64;
    let mut seconds = 0f64;
    while let Some(frame) = data.get(pos..).and_then(header) {
        // A frame cut off by the end of the file holds no whole audio.
        if pos + frame.len > data.len() {
            break;
        }
        frames += 1;
        seconds += f64::from(frame.samples) / f64::from(frame.sample_rate);
        pos += frame.len;
    }
    // Anything after the last frame (an ID3v1 or APE tag, padding) ends the walk.
    if frames == 0 {
        return None;
    }
    Some(Stream {
        frames,
        duration_ms: (seconds * 1000.0).round() as u64,
    })
}

#[derive(Debug, Clone, Copy)]
struct Header {
    len: usize,
    samples: u32,
    sample_rate: u32,
    mpeg1: bool,
    layer3: bool,
    mono: bool,
    crc: bool,
}

/// The first position from `start` that holds a frame header followed by
/// another one, or by the end of the data. Two in a row make a chance match in
/// other bytes unlikely.
fn first_frame(data: &[u8], start: usize) -> Option<usize> {
    (start..data.len()).find(|&pos| match header(&data[pos..]) {
        None => false,
        Some(frame) => {
            let next = pos + frame.len;
            next == data.len() || data.get(next..).and_then(header).is_some()
        }
    })
}

fn header(bytes: &[u8]) -> Option<Header> {
    let [b0, b1, b2, b3]: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    if b0 != 0xFF || b1 & 0xE0 != 0xE0 {
        return None;
    }
    let version = (b1 >> 3) & 3; // 0: MPEG-2.5, 1: reserved, 2: MPEG-2, 3: MPEG-1
    let layer = (b1 >> 1) & 3; // 0: reserved, 1: III, 2: II, 3: I
    let bitrate_index = usize::from(b2 >> 4); // 0: free format, 15: not allowed
    let rate_index = usize::from((b2 >> 2) & 3); // 3: reserved
    if version == 1
        || layer == 0
        || bitrate_index == 0
        || bitrate_index == 15
        || rate_index == 3
        || b3 & 3 == 2
    {
        return None;
    }
    let mpeg1 = version == 3;
    let kbps: [u32; 15] = match (mpeg1, layer) {
        (true, 3) => [
            0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
        ],
        (true, 2) => [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
        ],
        (true, _) => [
            0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
        ],
        (false, 3) => [
            0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
        ],
        (false, _) => [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    };
    let bitrate = kbps[bitrate_index] * 1000;
    let sample_rate = [44100, 48000, 32000][rate_index]
        >> match version {
            3 => 0,
            2 => 1,
            _ => 2,
        };
    let padding = u32::from((b2 >> 1) & 1);
    let (len, samples) = match layer {
        3 => ((12 * bitrate / sample_rate + padding) * 4, 384),
        2 => (144 * bitrate / sample_rate + padding, 1152),
        _ if mpeg1 => (144 * bitrate / sample_rate + padding, 1152),
        _ => (72 * bitrate / sample_rate + padding, 576),
    };
    Some(Header {
        len: len as usize,
        samples,
        sample_rate,
        mpeg1,
        layer3: layer == 1,
        mono: b3 >> 6 == 3,
        crc: b1 & 1 == 0,
    })
}

/// A Xing, Info or VBRI frame describes the stream and holds no audio.
fn is_info_frame(frame: &[u8], header: &Header) -> bool {
    if !header.layer3 {
        return false;
    }
    let side_info = match (header.mpeg1, header.mono) {
        (true, true) => 17,
        (true, false) => 32,
        (false, true) => 9,
        (false, false) => 17,
    };
    let at = 4 + if header.crc { 2 } else { 0 } + side_info;
    let word = |at: usize| frame.get(at..at + 4);
    matches!(word(at), Some(b"Xing") | Some(b"Info")) || word(36) == Some(b"VBRI")
}
