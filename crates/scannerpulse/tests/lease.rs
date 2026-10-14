mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use chrono::Utc;
use scannerpulse::lease::{self, Heartbeat, Lease, Status, Timings};
use scannerpulse::reader;
use scannerpulse::store::{self, NewCall, RecordError, Seen};
use sqlx::{Connection, PgPool};
use tokio::time::sleep;

/// The default timings, shortened so that the tests run in seconds. A round
/// takes up to half a second when the tests run side by side, so the waits
/// leave that much room.
fn quick() -> Timings {
    Timings {
        renew: Duration::from_millis(200),
        ttl: Duration::from_secs(2),
        grace: Duration::from_millis(1500),
        return_after: Duration::from_secs(1),
        margin: Duration::from_millis(500),
    }
}

fn host(name: &str, preferred: bool) -> Lease {
    Lease::with_token(name, "1-1", preferred, quick())
}

async fn holder(pool: &PgPool) -> String {
    sqlx::query_scalar("SELECT holder FROM leases WHERE name = 'processing'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_first_host_takes_a_free_lease_and_the_other_does_not(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (a, b) = (host("a", false), host("b", false));
    let (mut ca, mut cb) = (pool.acquire().await?, pool.acquire().await?);
    a.tick(&mut ca, true).await?;
    b.tick(&mut cb, true).await?;
    assert!(a.held() && a.should_work());
    assert!(!b.held());
    assert_eq!(holder(&pool).await, "a 1-1");
    let status = lease::status(&pool).await?;
    assert!(status.valid());
    let hosts: Vec<&str> = status.heartbeats.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(hosts, ["a", "b"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_preferred_host_takes_an_expired_lease_at_once_and_the_other_waits_the_grace(
    pool: PgPool,
) -> sqlx::Result<()> {
    let t = quick();
    let (mut c1, mut c2) = (pool.acquire().await?, pool.acquire().await?);
    let other = host("a", false);
    other.tick(&mut c1, true).await?;
    // It stops renewing; the preferred host takes the lease as soon as it expires.
    sleep(t.ttl + Duration::from_millis(200)).await;
    let preferred = host("p", true);
    preferred.tick(&mut c2, true).await?;
    assert!(preferred.held());

    // Now the preferred host stops; the other must wait the grace after the expiry.
    sleep(t.ttl + Duration::from_millis(200)).await;
    other.tick(&mut c1, true).await?;
    assert!(!other.held(), "within the grace");
    sleep(t.grace).await;
    other.tick(&mut c1, true).await?;
    assert!(other.held());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_clean_stop_lets_the_other_host_take_the_lease_at_its_next_round(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (a, b) = (host("a", true), host("b", false));
    let (mut ca, mut cb) = (pool.acquire().await?, pool.acquire().await?);
    a.tick(&mut ca, true).await?;
    b.tick(&mut cb, true).await?;
    assert!(!b.held());
    a.release(&mut ca).await?;
    assert!(!a.held());
    b.tick(&mut cb, true).await?;
    assert!(b.held(), "a released lease needs no grace");
    let hosts: Vec<String> = lease::status(&pool)
        .await?
        .heartbeats
        .into_iter()
        .map(|h| h.host)
        .collect();
    assert_eq!(hosts, ["b"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_lease_goes_back_to_the_preferred_host_after_it_ran_without_a_gap(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (other, preferred) = (host("n", false), host("p", true));
    let (mut cn, mut cp) = (pool.acquire().await?, pool.acquire().await?);
    other.tick(&mut cn, true).await?;
    preferred.tick(&mut cp, true).await?;
    assert!(other.held() && !preferred.held());
    other.tick(&mut cn, true).await?;
    assert!(!other.yielding());
    sleep(quick().return_after + Duration::from_millis(100)).await;
    preferred.tick(&mut cp, true).await?;
    other.tick(&mut cn, true).await?;
    assert!(other.yielding());
    assert!(
        other.held() && !other.should_work(),
        "it finishes its file, then stops"
    );
    assert!(other.hand_over(&mut cn).await?);
    assert!(!other.held());
    preferred.tick(&mut cp, true).await?;
    assert!(preferred.held());
    assert_eq!(holder(&pool).await, "p 1-1");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_restarted_preferred_host_starts_the_count_again(pool: PgPool) -> sqlx::Result<()> {
    // Heartbeats that outlast the waits below.
    let t = Timings {
        ttl: Duration::from_secs(5),
        return_after: Duration::from_secs(2),
        ..quick()
    };
    let other = Lease::with_token("n", "1-1", false, t);
    let (mut cn, mut cp) = (pool.acquire().await?, pool.acquire().await?);
    other.tick(&mut cn, true).await?;
    let first = Lease::with_token("p", "1-1", true, t);
    first.tick(&mut cp, true).await?;
    other.tick(&mut cn, true).await?;
    let first_seen = Instant::now();
    sleep(Duration::from_millis(1500)).await;
    // The preferred host restarts: a new process, the same host name.
    let second = Lease::with_token("p", "2-2", true, t);
    second.tick(&mut cp, true).await?;
    other.tick(&mut cn, true).await?;
    let restart_seen = Instant::now();
    // Past return_after since the first start, well within it since the restart.
    tokio::time::sleep_until((first_seen + t.return_after + Duration::from_millis(300)).into())
        .await;
    second.tick(&mut cp, true).await?;
    other.tick(&mut cn, true).await?;
    assert!(
        !other.yielding(),
        "the count must start again at the restart"
    );
    tokio::time::sleep_until((restart_seen + t.return_after).into()).await;
    second.tick(&mut cp, true).await?;
    other.tick(&mut cn, true).await?;
    assert!(other.yielding());
    Ok(())
}

/// A call to record, from a file with no tag.
fn sample() -> reader::FileCall {
    reader::read_bytes(
        "20310115_120000_x.mp3",
        &common::frames(28),
        Utc::now(),
        chrono_tz::UTC,
    )
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_frozen_holder_cannot_record_once_its_lease_is_gone(pool: PgPool) -> sqlx::Result<()> {
    let t = quick();
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    let seen = Seen {
        rel_path: "20310115_120000_x.mp3".to_string(),
        size: 1,
        file_mtime: Utc::now(),
    };
    store::upsert_seen(&pool, source, &[seen]).await?;
    let row: i64 = sqlx::query_scalar("SELECT id FROM ingest_files")
        .fetch_one(&pool)
        .await?;
    let call = sample();
    store::ensure_partition(&pool, call.started_at).await?;

    let frozen = host("a", false);
    let (mut ca, mut cp) = (pool.acquire().await?, pool.acquire().await?);
    frozen.tick(&mut ca, true).await?;
    assert!(frozen.held());
    // Frozen: no renewal. Its own deadline passes before the database's expiry.
    sleep(t.ttl - t.margin + Duration::from_millis(100)).await;
    assert!(!frozen.held());
    sleep(t.margin + Duration::from_millis(300)).await;
    let preferred = host("p", true);
    preferred.tick(&mut cp, true).await?;
    assert!(preferred.held());

    // It resumes between its check and its insert: the transaction stops it.
    let new = NewCall {
        call: &call,
        source_id: source,
        location_id: location,
        rel_path: "2031-01-15/Unknown/20310115_120000_x.mp3",
    };
    let err = store::record(&mut ca, frozen.holder(), row, &new)
        .await
        .unwrap_err();
    assert!(matches!(err, RecordError::LeaseLost), "{err}");
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&pool)
        .await?;
    assert_eq!(calls, 0);
    Ok(())
}

// A holder frozen inside the transaction that records a call holds the lease
// row FOR SHARE. The server ends that transaction after the idle limit, and
// the takeover goes ahead.
#[sqlx::test(migrations = "./migrations")]
async fn a_holder_frozen_in_its_transaction_blocks_a_takeover_for_seconds_only(
    pool: PgPool,
) -> sqlx::Result<()> {
    let frozen = host("a", false);
    let (mut ca, mut cp) = (pool.acquire().await?, pool.acquire().await?);
    frozen.tick(&mut ca, true).await?;
    let mut tx = ca.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL idle_in_transaction_session_timeout = '{}'",
        store::FENCE_IDLE_TIMEOUT
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query("SELECT 1 FROM leases WHERE name = 'processing' AND holder = $1 FOR SHARE")
        .bind(frozen.holder())
        .execute(&mut *tx)
        .await?;
    // Frozen here, with the transaction open.
    sleep(quick().ttl + Duration::from_millis(100)).await;
    let preferred = host("p", true);
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(10), preferred.tick(&mut cp, true))
        .await
        .expect("the takeover waited for the frozen holder for good")?;
    assert!(started.elapsed() < Duration::from_secs(6));
    assert_eq!(holder(&pool).await, "p 1-1");
    // The round waited longer than these short timings allow, so its own
    // deadline has passed; the next round renews the lease.
    preferred.tick(&mut cp, true).await?;
    assert!(preferred.held());
    drop(tx);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unhealthy_holder_gives_up_the_lease_and_its_heartbeat(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (sick, other) = (host("a", true), host("b", false));
    let (mut ca, mut cb) = (pool.acquire().await?, pool.acquire().await?);
    sick.tick(&mut ca, true).await?;
    other.tick(&mut cb, true).await?;
    sick.tick(&mut ca, false).await?;
    assert!(!sick.held());
    let status = lease::status(&pool).await?;
    assert!(!status.valid());
    let hosts: Vec<&str> = status.heartbeats.iter().map(|h| h.host.as_str()).collect();
    assert_eq!(hosts, ["b"], "the unhealthy host's heartbeat must go");
    other.tick(&mut cb, true).await?;
    assert!(other.held());
    // While it stays unhealthy it takes nothing.
    sick.tick(&mut ca, false).await?;
    assert!(!sick.held());
    Ok(())
}

#[test]
fn the_status_says_who_holds_the_lease_and_which_hosts_run() {
    let beat = |host: &str, age: f64, expires_in: f64| Heartbeat {
        host: host.to_string(),
        age,
        expires_in,
    };
    let status = Status {
        holder: Some("a 1-1".to_string()),
        expires_in: 12.34,
        heartbeats: vec![beat("a", 1.2, 13.8), beat("b", 20.0, -5.0)],
    };
    assert!(status.valid());
    assert_eq!(
        status.lines(),
        [
            "holder: a 1-1 (expires in 12.3 s)",
            "heartbeat a: renewed 1.2 s ago (running)",
            "heartbeat b: renewed 20.0 s ago (stopped)",
        ]
    );
    let expired = Status {
        expires_in: -3.21,
        heartbeats: Vec::new(),
        ..status
    };
    assert!(!expired.valid());
    assert_eq!(
        expired.lines(),
        ["holder: none (the lease of a 1-1 expired 3.2 s ago)"]
    );
    let released = Status {
        holder: Some(String::new()),
        expires_in: -86400.0,
        heartbeats: Vec::new(),
    };
    assert_eq!(released.lines(), ["holder: none (released)"]);
}
