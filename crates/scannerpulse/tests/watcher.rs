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

#[test]
fn a_change_of_sources_forgets_the_files_of_a_source_gone_or_moved() {
    let (a, b, c) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    fs::write(a.path().join("a.mp3"), b"1").unwrap();
    fs::write(b.path().join("b.mp3"), b"1").unwrap();
    let mut watcher = Watcher::new(vec![source(1, a.path()), source(2, b.path())], WAIT);
    let t0 = Instant::now();
    watcher.rescan(t0);
    assert!(
        !watcher.set_sources(vec![source(1, a.path()), source(2, b.path())]),
        "the same folders are no change"
    );
    assert!(watcher.set_sources(vec![source(1, a.path()), source(2, c.path())]));
    assert_eq!(names(&watcher.settle(t0 + WAIT)), ["a.mp3"]);
    let paths: Vec<&Path> = watcher.paths().collect();
    assert_eq!(paths, [a.path(), c.path()]);
}

#[test]
fn a_rescan_says_what_each_source_holds_or_why_it_cannot_be_listed() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["a.mp3", "b.mp3", "notes.txt"] {
        fs::write(dir.path().join(name), b"1").unwrap();
    }
    let gone = dir.path().join("gone");
    let mut watcher = Watcher::new(vec![source(1, dir.path()), source(2, &gone)], WAIT);
    watcher.rescan(Instant::now());
    let listings = watcher.listings();
    assert_eq!(listings[0], (1, Ok(2)));
    assert_eq!(listings[1].0, 2);
    let why = listings[1].1.as_ref().unwrap_err();
    assert!(why.starts_with("cannot be listed"), "{why}");
}

#[test]
fn bringing_in_takes_the_files_a_folder_held_when_it_was_asked() {
    let dir = tempfile::tempdir().unwrap();
    let now = std::time::SystemTime::now();
    for (name, age) in [("old.mp3", 60), ("new.mp3", 0)] {
        let path = dir.path().join(name);
        fs::write(&path, b"12345").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - Duration::from_secs(age))
            .unwrap();
    }
    let found = scannerpulse::watcher::list_folder(dir.path())
        .unwrap()
        .found;
    let seen = scannerpulse::watcher::to_bring_in(&found, now - Duration::from_secs(30));
    let taken: Vec<(&str, i64)> = seen.iter().map(|s| (s.rel_path.as_str(), s.size)).collect();
    assert_eq!(taken, [("old.mp3", 5)]);
}

static SLOW_LISTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A recorder's share that has stopped answering: listing the folder `hung`
/// takes a second.
fn slow_list(path: &Path) -> std::io::Result<scannerpulse::watcher::Listing> {
    if path.ends_with("hung") {
        SLOW_LISTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(1));
    }
    scannerpulse::watcher::list_folder(path)
}

#[tokio::test]
async fn a_folder_that_does_not_answer_holds_up_no_other_and_is_left_until_it_does() {
    use scannerpulse::watcher::{Hung, list_all};
    use std::sync::atomic::Ordering;
    let (good, parent) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    fs::write(good.path().join("a.mp3"), b"12345").unwrap();
    let hung_folder = parent.path().join("hung");
    fs::create_dir(&hung_folder).unwrap();
    let folders = vec![(1, good.path().to_path_buf()), (2, hung_folder.clone())];
    let mut hung = Hung::default();
    let started = Instant::now();
    let listed = list_all(
        folders.clone(),
        slow_list,
        &mut hung,
        Duration::from_millis(200),
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_millis(800),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(listed[0].0, 1);
    assert_eq!(listed[0].1.as_ref().unwrap().found.len(), 1);
    let why = listed[1].1.as_ref().unwrap_err().to_string();
    assert!(why.contains("did not answer within"), "{why}");
    let again = list_all(
        folders.clone(),
        slow_list,
        &mut hung,
        Duration::from_millis(200),
    )
    .await;
    let why = again[1].1.as_ref().unwrap_err().to_string();
    assert!(why.contains("still does not answer"), "{why}");
    assert_eq!(
        SLOW_LISTS.load(Ordering::SeqCst),
        1,
        "not listed again while it hangs"
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let answered = list_all(folders, slow_list, &mut hung, Duration::from_secs(5)).await;
    assert!(answered[1].1.is_ok(), "listed again once its thread ended");
}
