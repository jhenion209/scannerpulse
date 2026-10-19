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
