mod common;

use std::fs;
use std::time::{Duration, Instant};

use id3::TagLike;
use scannerpulse::health::HealthTimings;
use scannerpulse::ingest::{self, Config};
use scannerpulse::lease::{self, Timings};
use sqlx::PgPool;

const NAME: &str = "20310115_120000SYSC_East_Charlie__TO_330_FROM_5150.mp3";

/// The spec's timings, shortened.
fn config(pool: &PgPool) -> Config {
    Config {
        database: (*pool.connect_options()).clone(),
        host: "host-a".to_string(),
        preferred: true,
        lease: Timings {
            renew: Duration::from_millis(200),
            ttl: Duration::from_secs(3),
            grace: Duration::from_secs(1),
            return_after: Duration::from_secs(120),
            margin: Duration::from_millis(500),
        },
        health: HealthTimings {
            every: Duration::from_millis(200),
            timeout: Duration::from_secs(1),
            fail_after: Duration::from_secs(60),
        },
        stable_for: Duration::from_millis(300),
    }
}

async fn set_zone(pool: &PgPool) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES \
         ('time_zone', '\"America/Chicago\"'), ('ingest.rescan_seconds', '1')",
    )
    .execute(pool)
    .await
    .unwrap();
}

fn charlie() -> Vec<u8> {
    let mut tag = id3::Tag::new();
    tag.set_title("330\"Charlie Ops\"");
    tag.set_artist("5150");
    tag.set_text("TIT1", "SYSC");
    common::tagged(&tag, 28)
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_put_into_the_source_is_filed_and_the_lease_released_at_the_stop(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let daemon = tokio::spawn(ingest::run(config(&pool), async {
        let _ = stopped.await;
    }));

    fs::write(source.path().join(NAME), charlie()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let rel_path: String = loop {
        let found: Option<String> = sqlx::query_scalar("SELECT rel_path FROM calls")
            .fetch_optional(&pool)
            .await?;
        if let Some(rel_path) = found {
            break rel_path;
        }
        assert!(
            Instant::now() < deadline,
            "the file was not filed within 20 s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(rel_path, format!("2031-01-15/330-Charlie Ops/{NAME}"));
    assert_eq!(fs::read(root.path().join(&rel_path)).unwrap(), charlie());
    assert!(!source.path().join(NAME).exists());
    let status = lease::status(&pool).await?;
    assert!(status.valid());
    assert!(status.holder.unwrap().starts_with("host-a "));

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon)
        .await
        .expect("ingest did not stop within 10 s")
        .unwrap()
        .unwrap();
    let status = lease::status(&pool).await?;
    assert!(!status.valid(), "a clean stop releases the lease");
    assert!(status.heartbeats.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn without_a_time_zone_ingest_does_not_start(pool: PgPool) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        ingest::run(config(&pool), std::future::pending()),
    )
    .await
    .expect("a setup error must stop ingest at once");
    let err = result.unwrap_err();
    assert!(err.contains("time_zone"), "{err}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn without_an_archive_ingest_does_not_start(pool: PgPool) -> sqlx::Result<()> {
    set_zone(&pool).await;
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        ingest::run(config(&pool), std::future::pending()),
    )
    .await
    .expect("a setup error must stop ingest at once");
    let err = result.unwrap_err();
    assert!(err.contains("primary location"), "{err}");
    Ok(())
}
