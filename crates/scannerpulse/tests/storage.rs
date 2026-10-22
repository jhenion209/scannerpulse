mod common;

use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use common::{insert_call, seed_places};
use scannerpulse::storage::{self, Refused};
use sqlx::PgPool;

/// A check that accepts any folder as it is written.
fn any(folder: &Path) -> Result<PathBuf, String> {
    Ok(folder.to_path_buf())
}

fn refused<T: std::fmt::Debug>(result: Result<T, Refused>) -> String {
    match result {
        Err(Refused::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn beat(pool: &PgPool, host: &str) {
    sqlx::query(
        "INSERT INTO leases (name, holder, expires_at, renewed_at) \
         VALUES ('ingest:' || $1, $1, now() + interval '15 seconds', now())",
    )
    .bind(host)
    .execute(pool)
    .await
    .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_location_is_added_apart_from_every_folder_in_use(pool: PgPool) {
    seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('purge.holding_path', '\"/srv/held\"')")
        .execute(&pool)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let backups = Some(Path::new("/srv/backups"));
    let cold = storage::add(&mut conn, "Cold 2024", Path::new("/srv/cold"), backups, any)
        .await
        .unwrap();
    for (name, folder, expected) in [
        ("", "/srv/x", "the name is empty"),
        (
            "Cold 2024",
            "/srv/x",
            "a location named Cold 2024 exists already",
        ),
        ("x", "/srv/archive/cold", "the main archive, /srv/archive"),
        ("x", "/srv/cold/sub", "the location Cold 2024, /srv/cold"),
        (
            "x",
            "/srv/in",
            "the folder of the recorder live, /srv/in/live",
        ),
        ("x", "/srv/held/x", "the purge's holding folder, /srv/held"),
        ("x", "/srv/backups", "the backups' folder, /srv/backups"),
    ] {
        let why = refused(storage::add(&mut conn, name, Path::new(folder), backups, any).await);
        assert!(why.contains(expected), "{name} {folder}: {why}");
    }
    // A link may lead into a folder in use: the resolved folder is checked too.
    let why = refused(
        storage::add(&mut conn, "x", Path::new("/srv/link"), backups, |_| {
            Ok(PathBuf::from("/srv/archive/inside"))
        })
        .await,
    );
    assert!(why.contains("the main archive"), "{why}");
    let listed = storage::list(&mut conn).await.unwrap();
    let names: Vec<(&str, &str, &str, bool)> = listed
        .iter()
        .map(|l| (l.name.as_str(), l.root.as_str(), l.mode.as_str(), l.primary))
        .collect();
    assert_eq!(
        names,
        [
            ("primary", "/srv/archive", "managed", true),
            ("Cold 2024", "/srv/cold", "managed", false),
        ]
    );
    assert_eq!(storage::find(&mut conn, "Cold 2024").await.unwrap(), cold);
}

#[sqlx::test(migrations = "./migrations")]
async fn only_an_empty_location_that_is_not_the_main_archive_is_removed(pool: PgPool) {
    let places = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    let why = refused(storage::remove(&mut conn, places.0).await);
    assert!(why.contains("main archive"), "{why}");
    let cold = storage::add(&mut conn, "Cold", Path::new("/srv/cold"), None, any)
        .await
        .unwrap();
    let at = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    let (call, _) = insert_call(
        &pool,
        (cold, places.1),
        "2025-03-01/1-A/a.mp3",
        at,
        2000,
        false,
    )
    .await;
    let why = refused(storage::remove(&mut conn, cold).await);
    assert!(why.contains("still holds calls"), "{why}");
    sqlx::query("UPDATE calls SET location_id = $1 WHERE id = $2")
        .bind(places.0)
        .bind(call)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO archive_leftovers (location_id, rel_path) VALUES ($1, 'x.mp3')")
        .bind(cold)
        .execute(&pool)
        .await
        .unwrap();
    let why = refused(storage::remove(&mut conn, cold).await);
    assert!(why.contains("still being deleted"), "{why}");
    sqlx::query("DELETE FROM archive_leftovers")
        .execute(&pool)
        .await
        .unwrap();
    let job: i64 = sqlx::query_scalar(
        "INSERT INTO jobs (kind, params, started_by) VALUES ('move', jsonb_build_object('to', $1::int), 'x') RETURNING id",
    )
    .bind(cold)
    .fetch_one(&pool)
    .await
    .unwrap();
    let why = refused(storage::remove(&mut conn, cold).await);
    assert!(
        why.contains(&format!("job {job} moves calls into Cold")),
        "{why}"
    );
    sqlx::query("UPDATE jobs SET state = 'stopped'")
        .execute(&pool)
        .await
        .unwrap();
    storage::record_health(&pool, "host-a", &[(cold, Ok((10, 20)))])
        .await
        .unwrap();
    assert_eq!(storage::remove(&mut conn, cold).await.unwrap(), "Cold");
    assert!(matches!(
        storage::find(&mut conn, "Cold").await,
        Err(Refused::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn each_host_says_whether_it_can_use_each_location(pool: PgPool) {
    let places = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    let cold = storage::add(&mut conn, "Cold", Path::new("/srv/cold"), None, any)
        .await
        .unwrap();
    assert_eq!(
        storage::to_check(&pool).await.unwrap(),
        [
            (places.0, PathBuf::from("/srv/archive"), false),
            (cold, PathBuf::from("/srv/cold"), true),
        ]
    );
    let why = storage::unusable(&mut conn, cold, "Cold")
        .await
        .unwrap()
        .unwrap();
    assert!(why.contains("no ingest runs"), "{why}");
    beat(&pool, "host-a").await;
    beat(&pool, "host-b").await;
    storage::record_health(
        &pool,
        "host-a",
        &[(places.0, Ok((5, 50))), (cold, Ok((1000, 2000)))],
    )
    .await
    .unwrap();
    let why = storage::unusable(&mut conn, cold, "Cold")
        .await
        .unwrap()
        .unwrap();
    assert!(why.contains("host-b has not checked Cold yet"), "{why}");
    storage::record_health(&pool, "host-b", &[(cold, Err("it is not mounted".into()))])
        .await
        .unwrap();
    let why = storage::unusable(&mut conn, cold, "Cold")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(why, "host-b cannot use Cold: it is not mounted");
    let listed = storage::list(&mut conn).await.unwrap();
    let seen: Vec<(&str, &str, bool, Option<i64>)> = listed
        .iter()
        .flat_map(|l| {
            l.hosts
                .iter()
                .map(move |h| (l.name.as_str(), h.host.as_str(), h.ok, h.free_bytes))
        })
        .collect();
    assert_eq!(
        seen,
        [
            ("primary", "host-a", true, Some(5)),
            ("Cold", "host-a", true, Some(1000)),
            ("Cold", "host-b", false, None),
        ]
    );
    storage::record_health(&pool, "host-b", &[(cold, Ok((900, 2000)))])
        .await
        .unwrap();
    assert_eq!(
        storage::unusable(&mut conn, cold, "Cold").await.unwrap(),
        None
    );
    // A word older than five minutes is no word.
    sqlx::query("UPDATE location_health SET checked_at = now() - interval '10 minutes' WHERE host = 'host-b'")
        .execute(&pool)
        .await
        .unwrap();
    let why = storage::unusable(&mut conn, cold, "Cold")
        .await
        .unwrap()
        .unwrap();
    assert!(
        why.contains("host-b last checked Cold 10 minutes ago"),
        "{why}"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn each_location_s_calls_are_counted_with_a_sample_of_their_files(pool: PgPool) {
    let places = seed_places(&pool, Path::new("/srv/archive"), Path::new("/srv/in/live")).await;
    let mut conn = pool.acquire().await.unwrap();
    let cold = storage::add(&mut conn, "Cold", Path::new("/srv/cold"), None, any)
        .await
        .unwrap();
    let first = Utc.with_ymd_and_hms(2025, 3, 1, 12, 0, 0).unwrap();
    for n in 0..3 {
        insert_call(
            &pool,
            (places.0, places.1),
            &format!("2025-03-01/1-A/{n}.mp3"),
            first + chrono::Duration::minutes(n),
            2000,
            false,
        )
        .await;
    }
    insert_call(
        &pool,
        (cold, places.1),
        "2024-01-01/1-A/c.mp3",
        first,
        2000,
        false,
    )
    .await;
    let mut counted = storage::counts(&pool).await.unwrap();
    counted.sort_by_key(|c| c.location_id);
    assert_eq!(counted.len(), 2);
    assert_eq!(counted[0].calls, 3);
    assert_eq!(counted[0].first_call, Some(first));
    assert_eq!(
        counted[0].last_call,
        Some(first + chrono::Duration::minutes(2))
    );
    assert!(!counted[0].sample.is_empty());
    assert!(
        counted[0]
            .sample
            .iter()
            .all(|rel| rel.starts_with("2025-03-01/1-A/"))
    );
    assert_eq!(counted[1].calls, 1);
}
