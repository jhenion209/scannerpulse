use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use scannerpulse::health::{Health, HealthTimings, read_dir_probe};
use tokio::time::sleep;

/// The spec's timings, shortened.
fn quick() -> HealthTimings {
    HealthTimings {
        every: Duration::from_millis(50),
        timeout: Duration::from_millis(200),
        fail_after: Duration::from_millis(300),
    }
}

#[tokio::test]
async fn paths_that_answer_are_healthy() {
    let (archive, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut health = Health::new(archive.path(), &[source.path()], quick(), read_dir_probe);
    health.check().await;
    assert!(health.healthy());
}

#[tokio::test]
async fn a_failing_archive_makes_the_host_unhealthy_after_the_limit() {
    let (parent, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let archive = parent.path().join("archive");
    let mut health = Health::new(&archive, &[source.path()], quick(), read_dir_probe);
    health.check().await;
    assert!(health.healthy(), "not before the limit");
    sleep(quick().fail_after).await;
    health.check().await;
    assert!(!health.healthy());
    // It answers again: healthy at once.
    std::fs::create_dir(&archive).unwrap();
    health.check().await;
    assert!(health.healthy());
}

#[tokio::test]
async fn one_failing_source_is_not_enough_but_all_of_them_are() {
    let archive = tempfile::tempdir().unwrap();
    let good = tempfile::tempdir().unwrap();
    let (a, b) = (archive.path().join("a"), archive.path().join("b"));
    let mut one = Health::new(archive.path(), &[good.path(), &a], quick(), read_dir_probe);
    let mut all = Health::new(archive.path(), &[&a, &b], quick(), read_dir_probe);
    one.check().await;
    all.check().await;
    sleep(quick().fail_after).await;
    one.check().await;
    all.check().await;
    assert!(one.healthy());
    assert!(!all.healthy());
}

static PROBES: AtomicUsize = AtomicUsize::new(0);

/// A hung mount: it answers only after a second.
fn hung(_: &Path) -> io::Result<()> {
    PROBES.fetch_add(1, Ordering::SeqCst);
    std::thread::sleep(Duration::from_secs(1));
    Ok(())
}

#[tokio::test]
async fn a_hung_path_fails_without_blocking_and_gets_no_second_probe() {
    let archive = tempfile::tempdir().unwrap();
    let mut health = Health::new(archive.path(), &[], quick(), hung);
    let started = Instant::now();
    health.check().await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "the check must not wait for the hung path"
    );
    health.check().await;
    assert_eq!(PROBES.load(Ordering::SeqCst), 1);
    sleep(quick().fail_after).await;
    health.check().await;
    assert!(!health.healthy());
}
