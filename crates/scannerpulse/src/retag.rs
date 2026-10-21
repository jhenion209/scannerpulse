//! A call's file rewritten to name its talkgroup as the database does: the
//! talkgroup token of its file name and its ID3 title, and, for a call filed
//! through a mapped spelling, its system in the tag. The audio is kept byte
//! for byte. The result is read back with the reader's own rules, which must
//! give the new talkgroup and system, so the archive keeps describing itself.
//! The start time, the site and the radio must read back unchanged.

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

/// The file `name`, holding `data`, rewritten to name `to`, and `system`
/// when given. Err says why it cannot be: a tag that cannot be rewritten, or
/// a file whose talkgroup comes from somewhere this does not rewrite (a
/// patch's title).
pub fn retag(
    name: &str,
    data: &[u8],
    to: &Talkgroup,
    system: Option<&str>,
) -> Result<Retagged, String> {
    let new_data = retag_tag(data, to, system)?.unwrap_or_else(|| data.to_vec());
    let new_name = rename(name, to);
    let before = reader::describe(name, &reader::tags(data));
    let after = reader::describe(&new_name, &reader::tags(&new_data));
    if let Some(system) = system
        && after.system.as_deref() != Some(system)
    {
        return Err(format!(
            "the file's tag names no system that can be changed to {system}"
        ));
    }
    if reader::start_time(name) != reader::start_time(&new_name)
        || (system.is_none() && before.system != after.system)
        || (&before.site, before.radio_id, &before.radio_alias)
            != (&after.site, after.radio_id, &after.radio_alias)
    {
        return Err("the rewrite would change more than the talkgroup".to_string());
    }
    let read = after.talkgroup;
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

/// The bytes with the ID3 title naming `to` and, when given, the system
/// named `system`; `None` when the file has no tag, or one in which neither
/// changes.
fn retag_tag(data: &[u8], to: &Talkgroup, system: Option<&str>) -> Result<Option<Vec<u8>>, String> {
    let len = mp3::id3v2_len(data);
    if len == 0 || len > data.len() {
        return Ok(None);
    }
    let Ok(mut tag) = id3::Tag::read_from2(Cursor::new(&data[..len])) else {
        return Ok(None);
    };
    let titled = retitle(&mut tag, to);
    let renamed = system.is_some_and(|system| resystem(&mut tag, system));
    if !titled && !renamed {
        return Ok(None);
    }
    let mut out = Vec::with_capacity(data.len() + 64);
    tag.write_to(&mut out, tag.version())
        .map_err(|error| format!("the tag cannot be written again: {error}"))?;
    out.extend_from_slice(&data[len..]);
    Ok(Some(out))
}

/// ID3v2.4 separates several values with NUL: only the first is the
/// recorder's. The first value, and the rest.
fn first_value(text: &str) -> (&str, Option<&str>) {
    match text.split_once('\0') {
        Some((first, rest)) => (first, Some(rest)),
        None => (text, None),
    }
}

/// `text` with its first value replaced by `first`.
fn with_first(first: String, rest: Option<&str>) -> String {
    match rest {
        Some(rest) => format!("{first}\0{rest}"),
        None => first,
    }
}

/// Names `to` in the title; false when the title names no talkgroup.
fn retitle(tag: &mut id3::Tag, to: &Talkgroup) -> bool {
    let Some(title) = tag.title().map(str::to_string) else {
        return false;
    };
    let (first, rest) = first_value(&title);
    let new_first = if reader::title_talkgroup(first).is_some() {
        // <number>"<name>" and whatever follows the closing quote.
        let after = first.trim().splitn(3, '"').nth(2).unwrap_or_default();
        format!("{}\"{}\"{after}", to.number, to.name)
    } else if reader::channel(first).is_some() {
        token(to)
    } else {
        return false;
    };
    tag.set_title(with_first(new_first, rest));
    true
}

/// Names `system` where the reader takes the system from: the TXXX "Group
/// Description", else TIT1, else the comment's `System:` field. False when
/// the tag names no system.
fn resystem(tag: &mut id3::Tag, system: &str) -> bool {
    let named = |text: &str| !first_value(text).0.trim().is_empty();
    let described = tag
        .extended_texts()
        .find(|text| text.description.eq_ignore_ascii_case("Group Description"))
        .cloned();
    if let Some(text) = described
        && named(&text.value)
    {
        let rest = first_value(&text.value).1;
        tag.add_frame(id3::frame::ExtendedText {
            value: with_first(system.to_string(), rest),
            description: text.description,
        });
        return true;
    }
    let grouping = tag
        .get("TIT1")
        .and_then(|frame| frame.content().text())
        .map(str::to_string);
    if let Some(grouping) = grouping
        && named(&grouping)
    {
        tag.set_text(
            "TIT1",
            with_first(system.to_string(), first_value(&grouping).1),
        );
        return true;
    }
    let Some(comment) = tag
        .comments()
        .find(|comment| comment.lang == "eng" && comment.description.is_empty())
        .cloned()
    else {
        return false;
    };
    let (first, rest) = first_value(&comment.text);
    let Some(found) = reader::COMMENT_SYSTEM
        .captures(first)
        .and_then(|found| found.get(1))
        .filter(|found| !found.as_str().trim().is_empty())
    else {
        return false;
    };
    let new_first = format!(
        "{}{system}{}",
        &first[..found.start()],
        &first[found.end()..]
    );
    tag.add_frame(id3::frame::Comment {
        text: with_first(new_first, rest),
        ..comment
    });
    true
}
