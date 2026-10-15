//! A call's file rewritten to name its talkgroup as the database does: the
//! talkgroup token of its file name and its ID3 title. The audio is kept byte
//! for byte. The result is read back with the reader's own rules, which must
//! give the new talkgroup, so the archive keeps describing itself.

use std::io::Cursor;

use id3::TagLike;

use crate::mp3;
use crate::reader::{self, Talkgroup};

/// The file's new name and bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retagged {
    pub name: String,
    pub data: Vec<u8>,
}

/// The file `name`, holding `data`, rewritten to name `to`. Err says why it
/// cannot be: a tag that cannot be rewritten, or a file whose talkgroup comes
/// from somewhere this does not rewrite (a patch's title).
pub fn retag(name: &str, data: &[u8], to: &Talkgroup) -> Result<Retagged, String> {
    let new_data = retag_title(data, to)?.unwrap_or_else(|| data.to_vec());
    let new_name = rename(name, to);
    let read = reader::describe(&new_name, &reader::tags(&new_data)).talkgroup;
    if read.as_ref() != Some(to) {
        return Err(match read {
            Some(found) => format!(
                "the file would still name {}\"{}\"",
                found.number, found.name
            ),
            None => "the file names no talkgroup that can be corrected".to_string(),
        });
    }
    Ok(Retagged {
        name: new_name,
        data: new_data,
    })
}

/// The talkgroup token of a file name, written `<number>-<name>`; a `_` or
/// `/` in the name, which the token cannot hold, becomes `-`.
fn token(to: &Talkgroup) -> String {
    format!("{}-{}", to.number, to.name.replace(['_', '/'], "-"))
}

/// The name with its third `_` field replaced, when that field is a
/// talkgroup token; otherwise the name as it is.
fn rename(name: &str, to: &Talkgroup) -> String {
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) => (stem, Some(extension)),
        None => (name, None),
    };
    let mut fields: Vec<String> = stem.split('_').map(str::to_string).collect();
    if fields.len() > 2 && reader::channel(&fields[2]).is_some() {
        fields[2] = token(to);
    }
    let stem = fields.join("_");
    match extension {
        Some(extension) => format!("{stem}.{extension}"),
        None => stem,
    }
}

/// The bytes with the ID3 title naming `to`; `None` when the file has no
/// tag, or a title that names no talkgroup.
fn retag_title(data: &[u8], to: &Talkgroup) -> Result<Option<Vec<u8>>, String> {
    let len = mp3::id3v2_len(data);
    if len == 0 || len > data.len() {
        return Ok(None);
    }
    let Ok(mut tag) = id3::Tag::read_from2(Cursor::new(&data[..len])) else {
        return Ok(None);
    };
    let Some(title) = tag.title().map(str::to_string) else {
        return Ok(None);
    };
    // ID3v2.4 separates several values with NUL: only the first is the
    // recorder's.
    let (first, rest) = match title.split_once('\0') {
        Some((first, rest)) => (first, Some(rest)),
        None => (title.as_str(), None),
    };
    let new_first = if reader::title_talkgroup(first).is_some() {
        // <number>"<name>" and whatever follows the closing quote.
        let after = first.trim().splitn(3, '"').nth(2).unwrap_or_default();
        format!("{}\"{}\"{after}", to.number, to.name)
    } else if reader::channel(first).is_some() {
        token(to)
    } else {
        return Ok(None);
    };
    tag.set_title(match rest {
        Some(rest) => format!("{new_first}\0{rest}"),
        None => new_first,
    });
    let mut out = Vec::with_capacity(data.len() + 64);
    tag.write_to(&mut out, tag.version())
        .map_err(|error| format!("the tag cannot be written again: {error}"))?;
    out.extend_from_slice(&data[len..]);
    Ok(Some(out))
}
