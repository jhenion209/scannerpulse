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

#[sqlx::test(migrations = "./migrations")]
async fn setup_run_again_watches_its_recorder_whatever_was_done_to_it(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let (recordings, archive) = (Path::new("/srv/recordings"), Path::new("/srv/archive"));
    record(&mut conn, recordings, archive).await.unwrap();
    sqlx::query("UPDATE sources SET enabled = false, intake = 'asked', removed_at = now()")
        .execute(&mut *conn)
        .await
        .unwrap();
    record(&mut conn, recordings, archive).await.unwrap();
    let row: (bool, String, bool) =
        sqlx::query_as("SELECT enabled, intake, removed_at IS NULL FROM sources")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(row, (true, "watched".into(), true));
}
