use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use scannerpulse::store::Source;
use scannerpulse::watcher::{Batch, Watcher};

const WAIT: Duration = Duration::from_secs(2);

fn source(id: i16, dir: &Path) -> Source {
    Source {
        id,
        name: format!("source {id}"),
        path: dir.to_path_buf(),
    }
}

/// The paths in the batches, sorted.
fn names(batches: &[Batch]) -> Vec<String> {
    let mut names: Vec<String> = batches
        .iter()
        .flat_map(|batch| batch.files.iter().map(|file| file.rel_path.clone()))
        .collect();
    names.sort();
    names
}

#[test]
fn a_new_file_is_taken_once_it_stopped_changing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.mp3"), b"12345").unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    assert!(watcher.rescan(t0).is_empty());
    assert!(watcher.settle(t0 + Duration::from_secs(1)).is_empty());
    let got = watcher.settle(t0 + WAIT);
    assert_eq!(names(&got), ["a.mp3"]);
    assert_eq!((got[0].source_id, got[0].files[0].size), (3, 5));
}

#[test]
fn a_file_still_being_written_waits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    fs::write(&path, b"12").unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    fs::write(&path, b"12345").unwrap();
    assert!(watcher.settle(t0 + WAIT).is_empty());
    let got = watcher.settle(t0 + 2 * WAIT);
    assert_eq!(got[0].files[0].size, 5);
}

#[test]
fn only_mp3_files_outside_hidden_folders_are_taken() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "a.mp3",
        "b.MP3",
        "c.wav",
        ".d.mp3",
        ".hidden/e.mp3",
        "sub/f.mp3",
        "sub/deeper/g.mp3",
        "a.mp3.part",
    ] {
        let path = dir.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    assert_eq!(
        names(&watcher.settle(t0 + WAIT)),
        ["a.mp3", "b.MP3", "sub/deeper/g.mp3", "sub/f.mp3"]
    );
}

#[test]
fn a_recorded_file_is_sent_again_on_a_rescan_or_when_it_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    fs::write(&path, b"12345").unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    let got = watcher.settle(t0 + WAIT);
    watcher.recorded(&got);
    assert!(watcher.settle(t0 + 2 * WAIT).is_empty());
    // A rescan refreshes it: it is still in the source.
    assert_eq!(names(&watcher.rescan(t0 + 3 * WAIT)), ["a.mp3"]);
    // A change starts its wait again.
    fs::write(&path, b"123456").unwrap();
    let t1 = t0 + 4 * WAIT;
    watcher.touch(&path, t1);
    assert!(watcher.settle(t1).is_empty());
    assert_eq!(watcher.settle(t1 + WAIT)[0].files[0].size, 6);
}

#[test]
fn an_event_brings_a_file_in_before_the_next_rescan() {
    let dir = tempfile::tempdir().unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    assert!(watcher.rescan(t0).is_empty());
    let path = dir.path().join("b.mp3");
    fs::write(&path, b"x").unwrap();
    watcher.touch(&path, t0);
    // Events for other files are ignored.
    fs::write(dir.path().join("c.wav"), b"x").unwrap();
    watcher.touch(&dir.path().join("c.wav"), t0);
    watcher.touch(Path::new("/elsewhere/d.mp3"), t0);
    assert_eq!(names(&watcher.settle(t0 + WAIT)), ["b.mp3"]);
}

#[test]
fn an_event_for_a_hidden_or_linked_file_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let hidden = dir.path().join(".hidden/e.mp3");
    fs::create_dir_all(hidden.parent().unwrap()).unwrap();
    fs::write(&hidden, b"x").unwrap();
    let target = elsewhere.path().join("t.mp3");
    fs::write(&target, b"x").unwrap();
    let link = dir.path().join("link.mp3");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.touch(&hidden, t0);
    watcher.touch(&link, t0);
    assert!(watcher.settle(t0 + WAIT).is_empty());
}

#[test]
fn a_file_gone_from_its_source_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp3");
    fs::write(&path, b"x").unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    let got = watcher.settle(t0 + WAIT);
    watcher.recorded(&got);
    fs::remove_file(&path).unwrap();
    assert!(watcher.rescan(t0 + 2 * WAIT).is_empty());
}

#[test]
fn a_source_that_cannot_be_listed_refreshes_nothing() {
    let parent = tempfile::tempdir().unwrap();
    let dir = parent.path().join("share");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("a.mp3"), b"x").unwrap();
    let missing = parent.path().join("missing");
    let mut watcher = Watcher::new(vec![source(3, &dir), source(4, &missing)], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    let got = watcher.settle(t0 + WAIT);
    assert_eq!(names(&got), ["a.mp3"]);
    watcher.recorded(&got);
    // The share goes away: its file is neither refreshed nor forgotten.
    fs::rename(&dir, parent.path().join("away")).unwrap();
    assert!(watcher.rescan(t0 + 2 * WAIT).is_empty());
    fs::rename(parent.path().join("away"), &dir).unwrap();
    assert_eq!(names(&watcher.rescan(t0 + 3 * WAIT)), ["a.mp3"]);
}

// macOS refuses such names, so this runs where ingest runs.
#[cfg(target_os = "linux")]
#[test]
fn a_name_that_is_not_utf8_is_left_alone() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let odd = std::ffi::OsStr::from_bytes(b"20310115_120000_\xff.mp3");
    fs::write(dir.path().join(odd), b"x").unwrap();
    fs::write(dir.path().join("a.mp3"), b"x").unwrap();
    let mut watcher = Watcher::new(vec![source(3, dir.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    assert_eq!(names(&watcher.settle(t0 + WAIT)), ["a.mp3"]);
}
