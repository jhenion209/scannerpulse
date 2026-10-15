mod common;

use scannerpulse::reader::{self, Talkgroup};
use scannerpulse::retag::retag;

fn tg(number: i64, name: &str) -> Talkgroup {
    Talkgroup {
        number,
        name: name.to_string(),
    }
}

/// A recorder's tag: the system, and the title when given.
fn tag(title: Option<&str>) -> id3::Tag {
    use id3::TagLike;
    let mut tag = id3::Tag::new();
    if let Some(title) = title {
        tag.set_title(title);
    }
    tag.set_artist("711351");
    tag.set_text("TIT1", "SYSA");
    tag
}

fn audio(data: &[u8]) -> &[u8] {
    &data[scannerpulse::mp3::id3v2_len(data)..]
}

#[test]
fn a_quoted_title_is_rewritten_and_the_audio_kept() {
    let name = "20310115_120000SYSA_SYSA-P25_T-SYSA-North__TO_127_FROM_711351.mp3";
    let data = common::tagged(&tag(Some("127\"Alpha Fire\"")), 5);
    let done = retag(name, &data, &tg(127, "Alpha / Fire")).unwrap();
    assert_eq!(done.name, name, "this name carries no talkgroup token");
    assert_eq!(
        reader::tags(&done.data).title.as_deref(),
        Some("127\"Alpha / Fire\"")
    );
    assert_eq!(audio(&done.data), audio(&data));
    assert_eq!(
        reader::describe(&done.name, &reader::tags(&done.data)).talkgroup,
        Some(tg(127, "Alpha / Fire"))
    );
}

#[test]
fn the_name_token_is_rewritten_when_the_title_says_nothing() {
    let name = "2031-01-15_120000_127-Alpha Fire_711351-_SYSA-North_SYSA.mp3";
    let data = common::tagged(&tag(None), 3);
    let done = retag(name, &data, &tg(127, "Alpha Fire 2")).unwrap();
    assert_eq!(
        done.name,
        "2031-01-15_120000_127-Alpha Fire 2_711351-_SYSA-North_SYSA.mp3"
    );
    assert_eq!(done.data, data, "no title to change, so no byte changes");
}

#[test]
fn a_merge_changes_the_number_in_both_places() {
    let name = "2031-01-15_120000_127-Alpha_711351-_SYSA-North_SYSA.mp3";
    let data = common::tagged(&tag(Some("127\"Alpha\" (patched)")), 2);
    let done = retag(name, &data, &tg(128, "Bravo")).unwrap();
    assert_eq!(
        done.name,
        "2031-01-15_120000_128-Bravo_711351-_SYSA-North_SYSA.mp3"
    );
    assert_eq!(
        reader::tags(&done.data).title.as_deref(),
        Some("128\"Bravo\" (patched)")
    );
}

#[test]
fn every_value_after_the_first_is_kept() {
    let name = "20310115_120000.mp3";
    let data = common::tagged(&tag(Some("127\"Alpha\"\u{0}128\"Other\"")), 1);
    let done = retag(name, &data, &tg(127, "Alpha 2")).unwrap();
    use id3::TagLike;
    let read = id3::Tag::read_from2(std::io::Cursor::new(&done.data)).unwrap();
    assert_eq!(read.title(), Some("127\"Alpha 2\"\u{0}128\"Other\""));
}

#[test]
fn a_file_whose_talkgroup_cannot_be_rewritten_is_refused() {
    // A patch's title names the talkgroup only by its first number.
    let patch = common::tagged(&tag(Some("P:127,128 \"Alpha\"")), 1);
    assert!(retag("20310115_120000.mp3", &patch, &tg(127, "Alpha 2")).is_err());
    // The name token cannot hold a `_`, and there is no title to carry it.
    let name = "2031-01-15_120000_127-Alpha_711351-_SYSA-North_SYSA.mp3";
    let plain = common::tagged(&tag(None), 1);
    let refused = retag(name, &plain, &tg(127, "Alpha_2")).unwrap_err();
    assert!(refused.contains("127"), "{refused}");
    // A file without a system names no talkgroup at all.
    assert!(retag("20310115_120000.mp3", &common::frames(2), &tg(127, "A")).is_err());
}
