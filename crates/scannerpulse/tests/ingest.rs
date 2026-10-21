mod common;

use std::fs;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use id3::TagLike;
use scannerpulse::health::HealthTimings;
use scannerpulse::ingest::{self, Config};
use scannerpulse::lease::{self, Timings};
use scannerpulse::recorders::{self, Change};
use sqlx::PgPool;

const NAME: &str = "20310115_120000SYSC_East_Charlie__TO_330_FROM_5150.mp3";

/// The default timings, shortened.
fn config(pool: &PgPool) -> Config {
    Config {
        database: (*pool.connect_options()).clone(),
        nodes: Vec::new(),
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
        db_limit: Duration::from_secs(30),
        api_key: None,
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
    // The file leaves its folder just after its call is committed.
    while source.path().join(NAME).exists() {
        assert!(Instant::now() < deadline, "the file stayed in its folder");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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

/// Starts ingest; the sender stops it.
fn start(
    config: Config,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), String>>,
) {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let daemon = tokio::spawn(ingest::run(config, async {
        let _ = stopped.await;
    }));
    (stop, daemon)
}

async fn until_held(pool: &PgPool) -> sqlx::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !lease::status(pool).await?.valid() {
        assert!(
            Instant::now() < deadline,
            "the lease was not taken within 10 s"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_connection_that_stops_answering_does_not_stop_the_renewals(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    let proxy = common::Proxy::start(&pool);
    let mut config = config(&pool);
    config.database = proxy.options(&pool);
    let (_stop, daemon) = start(config);
    until_held(&pool).await?;

    proxy.freeze();
    let frozen = Instant::now();
    // A renewal sent before the freeze has reached the server by now.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let frozen_at: DateTime<Utc> = sqlx::query_scalar("SELECT now()").fetch_one(&pool).await?;
    let limit = ingest::ROUND_LIMIT + Duration::from_secs(3);
    loop {
        let renewed: bool =
            sqlx::query_scalar("SELECT renewed_at > $1 FROM leases WHERE name = 'processing'")
                .bind(frozen_at)
                .fetch_one(&pool)
                .await?;
        if renewed {
            break;
        }
        assert!(
            frozen.elapsed() < limit,
            "the lease was not renewed within {limit:?} of the freeze"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    daemon.abort();
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn after_the_pool_stops_answering_a_new_file_is_filed(pool: PgPool) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    let proxy = common::Proxy::start(&pool);
    let mut config = config(&pool);
    config.database = proxy.options(&pool);
    config.db_limit = Duration::from_secs(1);
    let (_stop, daemon) = start(config);
    until_held(&pool).await?;

    proxy.freeze();
    let frozen = Instant::now();
    fs::write(source.path().join(NAME), charlie()).unwrap();
    let limit = Duration::from_secs(15);
    loop {
        let filed: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
            .fetch_one(&pool)
            .await?;
        if filed == 1 {
            break;
        }
        assert!(
            frozen.elapsed() < limit,
            "the file was not filed within {limit:?} of the freeze"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    daemon.abort();
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stop_ends_ingest_while_the_database_never_answers(pool: PgPool) -> sqlx::Result<()> {
    let port = common::silent_listener();
    let mut config = config(&pool);
    config.database = (*pool.connect_options())
        .clone()
        .host("127.0.0.1")
        .port(port);
    let (stop, daemon) = start(config);
    tokio::time::sleep(Duration::from_secs(1)).await;
    stop.send(()).unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), daemon)
        .await
        .expect("ingest did not end within 3 s of the stop");
    assert_eq!(result.unwrap(), Ok(()));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_filed_call_of_a_talkgroup_with_transcription_on_is_transcribed(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    let server = common::FakeAsr::start(vec![common::Answer::Text("Charlie Ops copies.")]);
    common::set_transcription(&pool, &server.url, 1000, 5).await;
    // SYSC/330 with transcription on before its first call.
    let system: i32 = sqlx::query_scalar("INSERT INTO systems (name) VALUES ('SYSC') RETURNING id")
        .fetch_one(&pool)
        .await?;
    sqlx::query("INSERT INTO talkgroups (system_id, number, name, transcribe) VALUES ($1, 330, 'Charlie Ops', true)")
        .bind(system)
        .execute(&pool)
        .await?;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let daemon = tokio::spawn(ingest::run(config(&pool), async {
        let _ = stopped.await;
    }));

    fs::write(source.path().join(NAME), charlie()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let text: String = loop {
        let found = sqlx::query_scalar("SELECT text FROM transcripts")
            .fetch_optional(&pool)
            .await?;
        if let Some(found) = found {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "the call was not transcribed within 20 s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    let state: String = sqlx::query_scalar("SELECT transcript_state FROM calls")
        .fetch_one(&pool)
        .await?;
    assert_eq!(
        (text.as_str(), state.as_str()),
        ("Charlie Ops copies.", "text")
    );
    assert!(server.requests()[0].contains("model-x"));
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM transcription_queue")
        .fetch_one(&pool)
        .await?;
    assert_eq!(left, 0);

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon)
        .await
        .expect("ingest did not stop within 10 s")
        .unwrap()
        .unwrap();
    Ok(())
}

#[test]
fn the_transcription_summary_names_every_count() {
    let mut heard = ingest::Heard {
        text: 3,
        no_speech: 1,
        kept: 0,
        retry: 2,
        failed: 1,
        server_failures: 4,
    };
    assert_eq!(
        heard.line("q"),
        "transcription: 3 transcribed, 1 no speech, 2 to try again, 1 failed, 4 server failures; q"
    );
    heard.kept = 1;
    assert!(
        heard
            .line("q")
            .contains("1 no speech, 1 kept a person's text, 2 to try again")
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_standby_does_not_overwrite_the_holder_s_server_state(pool: PgPool) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    let url = "http://127.0.0.1:9";
    common::set_transcription(&pool, url, 1000, 5).await;
    sqlx::query(
        "INSERT INTO transcription_servers (url, cooling_until, failed_rounds, updated_at) \
         VALUES ($1, now() + interval '10 minutes', 3, now())",
    )
    .bind(url)
    .execute(&pool)
    .await?;
    common::hold_lease(&pool, "host-b 200-1900000000").await;
    let (stop, daemon) = start(config(&pool));
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (rounds, cooling): (i32, bool) = sqlx::query_as(
        "SELECT failed_rounds, coalesce(cooling_until > now(), false) \
         FROM transcription_servers WHERE url = $1",
    )
    .bind(url)
    .fetch_one(&pool)
    .await?;
    assert_eq!((rounds, cooling), (3, true));
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon)
        .await
        .expect("ingest did not stop within 10 s")
        .unwrap()
        .unwrap();
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn maintenance_pauses_filing_and_keeps_the_lease(pool: PgPool) -> sqlx::Result<()> {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    set_zone(&pool).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('maintenance.enabled', 'true')")
        .execute(&pool)
        .await?;
    let (stop, daemon) = start(config(&pool));
    until_held(&pool).await?;
    fs::write(source.path().join(NAME), charlie()).unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    let filed: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&pool)
        .await?;
    assert_eq!(filed, 0, "nothing is filed during maintenance");
    assert!(source.path().join(NAME).exists());
    assert!(lease::status(&pool).await?.valid(), "the lease is kept");
    sqlx::query("UPDATE settings SET value = 'false' WHERE key = 'maintenance.enabled'")
        .execute(&pool)
        .await?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let filed: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
            .fetch_one(&pool)
            .await?;
        if filed == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "filing did not resume within 20 s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon)
        .await
        .expect("ingest did not stop within 10 s")
        .unwrap()
        .unwrap();
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rescan_request_is_a_notice_every_host_hears(pool: PgPool) -> sqlx::Result<()> {
    let mut listener = sqlx::postgres::PgListener::connect_with(&pool).await?;
    listener.listen(ingest::RESCAN_CHANNEL).await?;
    ingest::request_rescan(&pool).await?;
    let note = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect("the notice within 5 s")?;
    assert_eq!(note.channel(), ingest::RESCAN_CHANNEL);
    Ok(())
}

const LATER: &str = "20310115_120500SYSC_East_Charlie__TO_330_FROM_5150.mp3";

async fn calls(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Waits up to 20 s for `n` calls.
async fn until_calls(pool: &PgPool, n: i64) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while calls(pool).await < n {
        assert!(
            Instant::now() < deadline,
            "{n} call(s) not filed within 20 s"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Waits up to 20 s until host-a's watcher found `files` audio files in the
/// recorder's folder.
async fn until_counted(pool: &PgPool, source: i16, files: i32) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let found: Option<Option<i32>> = sqlx::query_scalar(
            "SELECT files FROM source_health WHERE host = 'host-a' AND source_id = $1",
        )
        .bind(source)
        .fetch_optional(pool)
        .await
        .unwrap();
        if found == Some(Some(files)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{files} file(s) not counted within 20 s: {found:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn stopped(
    stop: tokio::sync::oneshot::Sender<()>,
    daemon: tokio::task::JoinHandle<Result<(), String>>,
) {
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), daemon)
        .await
        .expect("ingest did not stop within 10 s")
        .unwrap()
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_added_while_ingest_runs_is_watched_without_a_restart(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, live) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_, live_id) = common::seed_places(&pool, root.path(), live.path()).await;
    set_zone(&pool).await;
    let (stop, daemon) = start(config(&pool));
    until_held(&pool).await?;
    let east = tempfile::tempdir().unwrap();
    let mut conn = pool.acquire().await?;
    let east_id = recorders::add(&mut conn, "east", east.path(), false, recorders::check_here)
        .await
        .unwrap();
    fs::write(east.path().join(NAME), charlie()).unwrap();
    until_calls(&pool, 1).await;
    let source: i16 = sqlx::query_scalar("SELECT source_id FROM calls")
        .fetch_one(&pool)
        .await?;
    assert_eq!(source, east_id);
    assert!(!east.path().join(NAME).exists(), "filed and deleted");
    until_counted(&pool, live_id, 0).await;
    until_counted(&pool, east_id, 0).await;
    stopped(stop, daemon).await;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_taken_in_when_asked_files_only_what_it_held_when_asked(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, live) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), live.path()).await;
    set_zone(&pool).await;
    let north = tempfile::tempdir().unwrap();
    fs::write(north.path().join(NAME), charlie()).unwrap();
    let mut conn = pool.acquire().await?;
    let north_id = recorders::add(
        &mut conn,
        "north",
        north.path(),
        true,
        recorders::check_here,
    )
    .await
    .unwrap();
    let (stop, daemon) = start(config(&pool));
    until_held(&pool).await?;
    until_counted(&pool, north_id, 1).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(calls(&pool).await, 0, "nothing is taken before it is asked");
    recorders::change(&mut conn, north_id, Change::BringIn)
        .await
        .unwrap();
    until_calls(&pool, 1).await;
    assert!(!north.path().join(NAME).exists());
    let noted: bool =
        sqlx::query_scalar("SELECT brought_in_at = bring_in_at FROM sources WHERE id = $1")
            .bind(north_id)
            .fetch_one(&pool)
            .await?;
    assert!(noted);
    fs::write(north.path().join(LATER), charlie()).unwrap();
    until_counted(&pool, north_id, 1).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        calls(&pool).await,
        1,
        "a file written since waits to be asked for"
    );
    assert!(north.path().join(LATER).exists());
    stopped(stop, daemon).await;
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn with_its_only_recorder_paused_ingest_runs_and_files_once_it_is_watched(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (root, live) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_, live_id) = common::seed_places(&pool, root.path(), live.path()).await;
    set_zone(&pool).await;
    let mut conn = pool.acquire().await?;
    recorders::change(&mut conn, live_id, Change::Pause)
        .await
        .unwrap();
    fs::write(live.path().join(NAME), charlie()).unwrap();
    let (stop, daemon) = start(config(&pool));
    until_held(&pool).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(calls(&pool).await, 0, "a paused recorder's file stays");
    recorders::change(&mut conn, live_id, Change::Watch)
        .await
        .unwrap();
    until_calls(&pool, 1).await;
    stopped(stop, daemon).await;
    Ok(())
}
