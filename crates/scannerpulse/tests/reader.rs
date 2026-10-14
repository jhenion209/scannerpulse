mod common;

use chrono::{DateTime, NaiveDate, Utc};
use id3::TagLike;
use scannerpulse::localtime::TimeHow;
use scannerpulse::reader::{self, Described, ReadError, Tags, Talkgroup};

const CHICAGO: chrono_tz::Tz = chrono_tz::America::Chicago;

/// A file name of the kind the recorder writes (invented values).
const NAME: &str = "20310115_120000SYSA_North_Alpha__TO_127_FROM_711351.mp3";

fn s(value: &str) -> Option<String> {
    Some(value.to_string())
}

fn tg(number: i64, name: &str) -> Option<Talkgroup> {
    Some(Talkgroup {
        number,
        name: name.to_string(),
    })
}

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

/// The tags of a typical file from the recorder.
fn recorder_tags() -> Tags {
    Tags {
        title: s("127\"Alpha Fire\""),
        artist: s("711351"),
        group_description: None,
        grouping: s("SYSA"),
        comment: s("Date:2031-01-15 12:00:00.000;System:SYSA;Site:North;Name:Alpha Fire;"),
    }
}

#[test]
fn a_recorder_file_names_its_system_site_talkgroup_and_radio() {
    assert_eq!(
        reader::describe(NAME, &recorder_tags()),
        Described {
            system: s("SYSA"),
            site: s("North"),
            talkgroup: tg(127, "Alpha Fire"),
            title: None,
            radio_id: Some(711351),
            radio_alias: None,
        }
    );
}

#[test]
fn the_system_comes_from_the_group_description_then_tit1_then_the_comment() {
    let mut tags = recorder_tags();
    tags.group_description = s(" SYSG ");
    tags.grouping = s("SYST");
    tags.comment = s("System:SYSC;Site:North;");
    assert_eq!(reader::describe(NAME, &tags).system, s("SYSG"));
    tags.group_description = None;
    assert_eq!(reader::describe(NAME, &tags).system, s("SYST"));
    tags.grouping = None;
    assert_eq!(reader::describe(NAME, &tags).system, s("SYSC"));
}

#[test]
fn without_a_system_the_title_keeps_the_talkgroup() {
    let tags = Tags {
        title: s("127\"Alpha Fire\""),
        artist: s("711351"),
        comment: s("Site:North;"),
        ..Tags::default()
    };
    assert_eq!(
        reader::describe(NAME, &tags),
        Described {
            title: s("127\"Alpha Fire\""),
            radio_id: Some(711351),
            ..Described::default()
        }
    );
}

#[test]
fn a_channel_token_in_the_title_gives_the_talkgroup_and_the_title_is_kept() {
    let tags = Tags {
        title: s("473-Bravo EMS"),
        grouping: s("SYSA"),
        ..Tags::default()
    };
    let got = reader::describe(NAME, &tags);
    assert_eq!(got.talkgroup, tg(473, "Bravo EMS"));
    assert_eq!(got.title, s("473-Bravo EMS"));
}

#[test]
fn without_a_title_the_channel_in_the_file_name_gives_the_talkgroup() {
    let tags = Tags {
        grouping: s("SYSA"),
        ..Tags::default()
    };
    let got = reader::describe("2031-01-15_120000_215-Charlie PD_4021_SYSA.mp3", &tags);
    assert_eq!(got.talkgroup, tg(215, "Charlie PD"));
    assert_eq!(got.title, None);
    // The recorder's own names have no channel token in their third field.
    assert_eq!(reader::describe(NAME, &tags).talkgroup, None);
}

#[test]
fn a_patch_title_gives_its_first_number_and_name() {
    for title in ["P:61012, 61013 \"Delta Patch\"", "P:61012\"Delta Patch\""] {
        let tags = Tags {
            title: s(title),
            grouping: s("SYSA"),
            ..Tags::default()
        };
        let got = reader::describe(NAME, &tags);
        assert_eq!(got.talkgroup, tg(61012, "Delta Patch"), "{title}");
        assert_eq!(got.title, s(title));
    }
}

#[test]
fn a_number_too_long_for_a_bigint_gives_no_talkgroup() {
    let nineteen = "1234567890123456789";
    for title in [
        format!("{nineteen}\"Echo\""),
        format!("P:{nineteen} \"Echo\""),
        format!("{nineteen}-Echo"),
    ] {
        let tags = Tags {
            title: Some(title.clone()),
            grouping: s("SYSA"),
            ..Tags::default()
        };
        let got = reader::describe(NAME, &tags);
        assert_eq!(got.talkgroup, None, "{title}");
        assert_eq!(got.title, Some(title));
    }
}

#[test]
fn the_artist_gives_the_radio_id_and_alias() {
    let cases: [(&str, Option<i64>, Option<&str>); 6] = [
        ("4021", Some(4021), None),
        ("4021-Engine 5", Some(4021), Some("Engine 5")),
        ("4021 Engine 5", Some(4021), Some("Engine 5")),
        ("4021-", Some(4021), None),
        ("Engine 5", None, Some("Engine 5")),
        ("1234567890123456789", None, Some("1234567890123456789")),
    ];
    for (artist, id, alias) in cases {
        let tags = Tags {
            artist: s(artist),
            grouping: s("SYSA"),
            ..Tags::default()
        };
        let got = reader::describe(NAME, &tags);
        assert_eq!(
            (got.radio_id, got.radio_alias.as_deref()),
            (id, alias),
            "{artist}"
        );
    }
}

#[test]
fn the_start_time_is_at_the_head_of_the_name() {
    let at = |y, mo, d, h, mi, s| {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, s)
    };
    assert_eq!(reader::start_time(NAME), at(2031, 1, 15, 12, 0, 0));
    assert_eq!(
        reader::start_time("2031-01-15_235959_215-Charlie PD.mp3"),
        at(2031, 1, 15, 23, 59, 59)
    );
    for name in [
        "recording.mp3",
        "20311399_120000.mp3",
        "20310115_250000.mp3",
        "20310115-120000.mp3",
        "x20310115_120000.mp3",
    ] {
        assert_eq!(reader::start_time(name), None, "{name}");
    }
}

fn recorder_tag() -> id3::Tag {
    let mut tag = id3::Tag::new();
    tag.set_title("127\"Alpha Fire\"");
    tag.set_artist("711351");
    tag.set_text("TIT1", "SYSA");
    tag.add_frame(id3::frame::Comment {
        lang: "eng".to_string(),
        description: String::new(),
        text: "System:SYSA;Site:North;".to_string(),
    });
    tag
}

/// What `recorder_tag` reads back as.
fn recorder_tags_read() -> Tags {
    Tags {
        comment: s("System:SYSA;Site:North;"),
        ..recorder_tags()
    }
}

#[test]
fn the_tag_is_read_back_from_the_bytes() {
    assert_eq!(
        reader::tags(&common::tagged(&recorder_tag(), 1)),
        recorder_tags_read()
    );
}

#[test]
fn a_file_is_read_whole() {
    let data = common::tagged(&recorder_tag(), 28);
    let mtime = utc("2031-01-15T18:00:05Z");
    let call = reader::read_bytes(NAME, &data, mtime, CHICAGO).unwrap();
    assert_eq!(call.started_at, utc("2031-01-15T18:00:00Z"));
    assert_eq!(call.time_how, TimeHow::Plain);
    assert_eq!(call.duration_ms, 1008);
    assert_eq!(call.file_mtime, mtime);
    assert_eq!(
        call.described,
        reader::describe(NAME, &recorder_tags_read())
    );
}

#[test]
fn only_the_first_value_of_a_text_frame_counts() {
    let mut tag = recorder_tag();
    tag.set_title("127\"Alpha Fire\"\u{0}128\"Other\"");
    let tags = reader::tags(&common::tagged(&tag, 1));
    assert_eq!(tags.title, s("127\"Alpha Fire\""));
}

#[test]
fn a_damaged_tag_is_read_as_no_tag() {
    let body = b"TIT2\x7f\x7f\x7f\x7f\x00\x00not a frame".to_vec();
    let mut data = common::id3v2_header(body.len());
    data.extend_from_slice(&body);
    data.extend_from_slice(&common::frames(28));
    let call = reader::read_bytes(NAME, &data, utc("2031-01-15T18:00:05Z"), CHICAGO).unwrap();
    assert_eq!(call.described, Described::default());
    assert_eq!(call.duration_ms, 1008);
}

#[test]
fn a_file_without_a_date_or_a_frame_is_rejected() {
    let mtime = utc("2031-01-15T18:00:05Z");
    let err = reader::read_bytes("recording.mp3", &common::frames(5), mtime, CHICAGO).unwrap_err();
    assert!(err.contains("date"), "{err}");
    let err = reader::read_bytes(NAME, b"not audio", mtime, CHICAGO).unwrap_err();
    assert!(err.contains("frame"), "{err}");
}

#[test]
fn a_file_too_large_is_rejected_unread() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(NAME);
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(reader::MAX_FILE_BYTES + 1).unwrap();
    match reader::read_path(&path, CHICAGO) {
        Err(ReadError::Rejected(why)) => assert!(why.contains("larger"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_missing_file_is_an_error_to_retry() {
    let dir = tempfile::tempdir().unwrap();
    match reader::read_path(&dir.path().join(NAME), CHICAGO) {
        Err(ReadError::Io(error)) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
        other => panic!("{other:?}"),
    }
}
