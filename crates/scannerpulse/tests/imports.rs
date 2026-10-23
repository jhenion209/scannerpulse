mod common;

use std::path::Path;

use chrono::{TimeDelta, Utc};
use common::seed_places;
use scannerpulse::recorders::{self, Refused};
use scannerpulse::store;
use sqlx::PgPool;

/// An import made by hand: a read-only location, a source of the kind
/// import, and the row that joins them. Returns the location and the source.
async fn import_by_hand(pool: &PgPool, name: &str, root: &Path) -> (i16, i16) {
    let location: i16 = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode) VALUES ($1, $2, 'readonly') RETURNING id",
    )
    .bind(name)
    .bind(root.display().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    let source: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path, enabled) VALUES ($1, 'import', $2, false) RETURNING id",
    )
    .bind(name)
    .bind(root.display().to_string())
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO imports (source_id, location_id) VALUES ($1, $2)")
        .bind(source)
        .bind(location)
        .execute(pool)
        .await
        .unwrap();
    (location, source)
}

#[sqlx::test(migrations = "./migrations")]
async fn an_imports_files_are_never_a_recorders(pool: PgPool) {
    seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let (_, source) = import_by_hand(&pool, "Old", Path::new("/srv/old")).await;
    sqlx::query("INSERT INTO systems (name) VALUES ('Metro')")
        .execute(&pool)
        .await
        .unwrap();
    let month_ago = Utc::now() - TimeDelta::days(30);
    for (rel, state, system) in [
        ("a.mp3", "waiting", None),
        ("b.mp3", "staged", Some("Metro")),
        ("c.mp3", "done", None),
        ("d.mp3", "rejected", None),
    ] {
        sqlx::query(
            "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, staged_system) \
             VALUES ($1, $2, $3, $3, $4, $5)",
        )
        .bind(source)
        .bind(rel)
        .bind(month_ago)
        .bind(state)
        .bind(system)
        .execute(&pool)
        .await
        .unwrap();
    }
    // The filer never takes them: the import reads its own files, and a
    // staged one is in the import's folder, not in the staging folder.
    assert!(store::waiting(&pool, 100, &[]).await.unwrap().is_empty());
    // They are the import's register, kept however old.
    assert_eq!(store::prune_done(&pool, 7).await.unwrap(), 0);
    // It is no recorder: not listed, not found by name, and its files are
    // not counted on Status.
    let mut conn = pool.acquire().await.unwrap();
    let listed: Vec<String> = recorders::list(&mut conn, month_ago)
        .await
        .unwrap()
        .into_iter()
        .map(|recorder| recorder.name)
        .collect();
    assert_eq!(listed, ["live"]);
    assert!(matches!(
        recorders::find(&mut conn, "Old").await,
        Err(Refused::NoSuch(_))
    ));
    match recorders::add(&mut conn, "Old", Path::new("/srv/in/old"), false, |f| {
        Ok(f.to_path_buf())
    })
    .await
    {
        Err(Refused::Refused(why)) => assert!(why.contains("imported as Old"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    let status = scannerpulse::status::read_within(&pool, std::time::Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(status["ingest"]["waiting"], 0);
    assert_eq!(status["ingest"]["rejected"], 0);
    assert_eq!(status["ingest"]["recent_rejected"], serde_json::json!([]));
    assert_eq!(status["failover"]["files_left_in_sources"], 0);
}
