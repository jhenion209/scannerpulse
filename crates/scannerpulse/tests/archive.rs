use std::fs;
use std::io::ErrorKind;

use chrono::NaiveDate;
use scannerpulse::archive::{
    self, Place, candidate, file_digest, finish_part, folder, part_path, rel_path, sha256,
    write_part,
};

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2031, 1, 15).unwrap()
}

#[test]
fn a_talkgroup_folder_is_its_number_and_name() {
    assert_eq!(folder(Some((127, "Alpha Fire"))), "127-Alpha Fire");
    assert_eq!(folder(Some((5, "Fire/EMS"))), "5-Fire_EMS");
    assert_eq!(folder(Some((5, ".."))), "5-..");
    assert_eq!(folder(None), "Unknown");
}

#[test]
fn the_archive_path_is_date_folder_and_name() {
    assert_eq!(
        rel_path(day(), "127-Alpha Fire", "a.mp3").unwrap(),
        "2031-01-15/127-Alpha Fire/a.mp3"
    );
}

#[test]
fn a_name_that_would_leave_its_folder_is_refused() {
    for (dir, name) in [
        ("..", "a.mp3"),
        (".", "a.mp3"),
        ("", "a.mp3"),
        ("a/b", "a.mp3"),
        ("127-Alpha", ".."),
        ("127-Alpha", "/etc/passwd"),
        ("127-Alpha", "a\0b.mp3"),
    ] {
        assert!(rel_path(day(), dir, name).is_err(), "{dir:?} {name:?}");
    }
}

#[test]
fn a_longer_copy_is_named_by_its_calls_start_and_its_own_rest() {
    assert_eq!(
        archive::copy_name(
            "20310115_120000SYSB_South_Bravo__TO_215_FROM_4021_1.mp3",
            "20310115_120001SYSB_North_Bravo__TO_215_FROM_4021.mp3"
        )
        .as_deref(),
        Some("20310115_120000SYSB_North_Bravo__TO_215_FROM_4021.mp3")
    );
    assert_eq!(
        archive::copy_name("2031-01-15_120000_a.mp3", "20310115_120001_b.mp3").as_deref(),
        Some("2031-01-15_120000_b.mp3")
    );
    assert_eq!(
        archive::copy_name("no-start.mp3", "20310115_120001_b.mp3"),
        None
    );
    assert_eq!(
        archive::copy_name("20310115_120000_a.mp3", "no-start.mp3"),
        None
    );
}

#[test]
fn a_taken_name_gets_a_number_before_the_extension() {
    assert_eq!(candidate("a.mp3", 0), "a.mp3");
    assert_eq!(candidate("a.mp3", 1), "a_1.mp3");
    assert_eq!(candidate("a.b.mp3", 2), "a.b_2.mp3");
    assert_eq!(candidate("noext", 1), "noext_1");
    assert_eq!(candidate(".hidden", 1), ".hidden_1");
}

#[test]
fn a_file_is_written_checked_and_then_named() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("2031-01-15/127-Alpha/a.mp3");
    let data = b"audio".to_vec();
    write_part(&path, &data, &sha256(&data)).unwrap();
    assert!(part_path(&path).exists());
    assert!(!path.exists());
    finish_part(&path).unwrap();
    assert_eq!(fs::read(&path).unwrap(), data);
    assert!(!part_path(&path).exists());
    assert_eq!(file_digest(&path).unwrap(), (sha256(&data), 5));
}

#[test]
fn a_copy_that_does_not_match_is_removed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.mp3");
    let err = write_part(&path, b"audio", &sha256(b"other")).unwrap_err();
    assert!(err.to_string().contains("does not match"), "{err}");
    assert!(!part_path(&path).exists());
}

#[test]
fn a_file_in_the_archive_is_never_replaced() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("a.mp3");
    fs::write(&path, b"first").unwrap();
    write_part(&path, b"second", &sha256(b"second")).unwrap();
    let err = finish_part(&path).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&path).unwrap(), b"first");
}

#[test]
fn the_place_is_an_exact_copy_or_the_first_free_name() {
    let root = tempfile::tempdir().unwrap();
    let dir = "2031-01-15/127-Alpha";
    let data = b"audio".to_vec();
    let digest = sha256(&data);
    let at = |name: &str| root.path().join(dir).join(name);

    // No folder yet.
    assert_eq!(
        archive::place(root.path(), dir, "a.mp3", 5, &digest).unwrap(),
        Place::Free(format!("{dir}/a.mp3"))
    );
    // Other calls took the name, one with another size and one with the same size.
    fs::create_dir_all(root.path().join(dir)).unwrap();
    fs::write(at("a.mp3"), b"other call").unwrap();
    fs::write(at("a_1.mp3"), b"other").unwrap();
    assert_eq!(
        archive::place(root.path(), dir, "a.mp3", 5, &digest).unwrap(),
        Place::Free(format!("{dir}/a_2.mp3"))
    );
    // The file itself is there, under a numbered name.
    fs::write(at("a_2.mp3"), &data).unwrap();
    assert_eq!(
        archive::place(root.path(), dir, "a.mp3", 5, &digest).unwrap(),
        Place::Copy(format!("{dir}/a_2.mp3"))
    );
}

#[test]
fn a_part_file_left_by_a_crash_is_deleted() {
    let root = tempfile::tempdir().unwrap();
    let dir = "2031-01-15/127-Alpha";
    fs::create_dir_all(root.path().join(dir)).unwrap();
    let leftover = part_path(&root.path().join(dir).join("a.mp3"));
    fs::write(&leftover, b"half").unwrap();
    assert_eq!(
        archive::place(root.path(), dir, "a.mp3", 5, &sha256(b"audio")).unwrap(),
        Place::Free(format!("{dir}/a.mp3"))
    );
    assert!(!leftover.exists());
}
