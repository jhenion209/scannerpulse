use std::path::Path;

use scannerpulse::setup::folders::record;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn the_folders_are_recorded_once_and_a_second_setup_updates_them(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    record(
        &mut conn,
        Path::new("/srv/recordings"),
        Path::new("/srv/archive"),
    )
    .await
    .unwrap();
    record(
        &mut conn,
        Path::new("/srv/recordings2"),
        Path::new("/srv/archive2"),
    )
    .await
    .unwrap();

    let locations: Vec<(String, String, bool, String)> =
        sqlx::query_as("SELECT name, root, is_primary, mode FROM locations")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        locations,
        [(
            "archive".into(),
            "/srv/archive2".into(),
            true,
            "managed".into()
        )]
    );
    let sources: Vec<(String, String, String, bool)> =
        sqlx::query_as("SELECT name, path, kind, enabled FROM sources")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        sources,
        [(
            "recordings".into(),
            "/srv/recordings2".into(),
            "sdrtrunk".into(),
            true
        )]
    );
}
