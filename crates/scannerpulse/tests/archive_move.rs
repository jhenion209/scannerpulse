//! The archive pointed at its new folder: the primary location only.

use scannerpulse::setup::archive_move::{current, point_at};
use sqlx::PgPool;
use std::path::{Path, PathBuf};

#[sqlx::test(migrations = "./migrations")]
async fn the_archive_is_pointed_at_its_new_folder(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    assert_eq!(current(&mut conn).await.unwrap(), None);
    scannerpulse::setup::folders::record(
        &mut conn,
        Path::new("/srv/recordings"),
        Path::new("/srv/archive"),
    )
    .await
    .unwrap();
    // A second location, read-only, keeps its own root.
    sqlx::query(
        "INSERT INTO locations (name, root, mode) VALUES ('old calls', '/srv/old', 'readonly')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    assert_eq!(
        current(&mut conn).await.unwrap(),
        Some(PathBuf::from("/srv/archive"))
    );
    assert_eq!(
        point_at(&mut conn, Path::new("/srv/shared/archive"))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        current(&mut conn).await.unwrap(),
        Some(PathBuf::from("/srv/shared/archive"))
    );
    let other: String = sqlx::query_scalar("SELECT root FROM locations WHERE name = 'old calls'")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(other, "/srv/old");
}

#[test]
fn a_move_or_a_purge_between_the_check_and_the_switch_leaves_no_call_without_its_file() {
    use scannerpulse::setup::archive_move::compare;
    let dir = tempfile::tempdir().unwrap();
    let (old, new) = (dir.path().join("old"), dir.path().join("new"));
    let put = |root: &Path, rel: &str, body: &str| {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, body).unwrap();
        path
    };
    // The person's copy, checked: two calls' files.
    for root in [&old, &new] {
        put(root, "2026-09-01/1/a.mp3", "aaa");
        put(root, "2026-09-01/1/b.mp3", "bbb");
    }
    // Then, before the switch: a move took b's call to another location
    // (a purge would delete it the same way), and a move back brought c's,
    // its file's time kept from the day it was filed.
    std::fs::remove_file(old.join("2026-09-01/1/b.mp3")).unwrap();
    let c = put(&old, "2026-08-01/2/c.mp3", "ccc");
    let filed = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_785_000_000);
    std::fs::File::options()
        .write(true)
        .open(&c)
        .unwrap()
        .set_modified(filed)
        .unwrap();
    // The switch compares the whole archive again: it copies c, whatever its
    // time, and b's file is left behind in the new folder, read by nothing.
    let compared = compare(&old, &new, |_| {}).unwrap();
    assert_eq!(compared.missing, vec!["2026-08-01/2/c.mp3".to_string()]);
    assert!(compared.differ.is_empty());
    assert_eq!(compared.files, 2);
    assert!(new.join("2026-09-01/1/b.mp3").exists());
}
