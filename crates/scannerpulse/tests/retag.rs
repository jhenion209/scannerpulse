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
fn the_rest_of_what_the_reader_reads_is_kept() {
    use id3::TagLike;
    let name = "2031-01-15_120000_127-Alpha_711351-_SYSA-North_SYSA.mp3";
    let mut recorder = tag(Some("127\"Alpha\""));
    recorder.set_artist("711351-Unit 5");
    recorder.add_frame(id3::frame::Comment {
        lang: "eng".to_string(),
        description: String::new(),
        text: "System:SYSA;Site:North;".to_string(),
    });
    let data = common::tagged(&recorder, 2);
    let before = reader::describe(name, &reader::tags(&data));
    let done = retag(name, &data, &tg(128, "Bravo")).unwrap();
    let after = reader::describe(&done.name, &reader::tags(&done.data));
    assert_eq!(after.site.as_deref(), Some("North"));
    assert_eq!(after.radio_id, Some(711351));
    assert_eq!(after.radio_alias.as_deref(), Some("Unit 5"));
    assert_eq!(after.site, before.site);
    assert_eq!(after.radio_id, before.radio_id);
    assert_eq!(after.radio_alias, before.radio_alias);
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

#[test]
fn older_tags_and_an_id3v1_tag_are_kept() {
    for version in [id3::Version::Id3v23, id3::Version::Id3v22] {
        let mut data = Vec::new();
        if tag(Some("127\"Alpha\""))
            .write_to(&mut data, version)
            .is_err()
        {
            continue;
        }
        data.extend_from_slice(&common::frames(3));
        // An ID3v1 tag at the end: "TAG" and 125 bytes.
        data.extend_from_slice(b"TAG");
        data.extend_from_slice(&[b'x'; 125]);
        match retag("20310115_120000.mp3", &data, &tg(127, "Alpha 2")) {
            Ok(done) => {
                assert_eq!(
                    audio(&done.data),
                    audio(&data),
                    "{version:?}: audio and v1 tag kept"
                );
                assert_eq!(
                    reader::tags(&done.data).title.as_deref(),
                    Some("127\"Alpha 2\""),
                    "{version:?}"
                );
            }
            // A tag the crate cannot write again refuses the file; it is never half written.
            Err(why) => assert_eq!(version, id3::Version::Id3v22, "{why}"),
        }
    }
}
