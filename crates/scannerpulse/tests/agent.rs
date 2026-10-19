//! The root helper's requests: one at a time, taken by a helper that is up,
//! with what it says kept line by line, and closed if it stops partway.

use scannerpulse::agent::{self, Kind, Refused};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn a_request_waits_for_a_helper_and_one_at_a_time(pool: PgPool) {
    // No helper has said it is up.
    assert!(matches!(
        agent::ask(&pool, Kind::Update, "ann").await,
        Err(Refused::NoHelper)
    ));
    agent::beat(&pool, "radio-a").await.unwrap();
    let id = agent::ask(&pool, Kind::Update, "ann").await.unwrap();
    // Another, of either kind, waits for this one to finish.
    assert!(matches!(
        agent::ask(&pool, Kind::Switchover, "bob").await,
        Err(Refused::Busy)
    ));
    assert_eq!(
        agent::claim(&pool, "radio-a").await.unwrap(),
        Some((id, Kind::Update))
    );
    assert_eq!(agent::claim(&pool, "radio-a").await.unwrap(), None);
    agent::say(&pool, id, "Updating db1 (0.8.0 to 0.9.0)...")
        .await
        .unwrap();
    agent::finish(&pool, id, true, "ScannerPulse 0.9.0 runs on every machine.")
        .await
        .unwrap();
    assert!(!agent::open(&pool).await.unwrap());
    assert_eq!(agent::last(&pool, Kind::Switchover).await.unwrap(), None);
    let last = agent::last(&pool, Kind::Update).await.unwrap().unwrap();
    assert_eq!(last.id, id);
    assert_eq!(last.kind, "update");
    assert_eq!(last.requested_by, "ann");
    assert_eq!(last.host.as_deref(), Some("radio-a"));
    assert_eq!(last.ok, Some(true));
    assert!(last.started_at.is_some() && last.finished_at.is_some());
    assert_eq!(
        last.said,
        "Updating db1 (0.8.0 to 0.9.0)...\nScannerPulse 0.9.0 runs on every machine.\n"
    );
    // Finished: the next may be asked, and each kind keeps its own newest.
    let next = agent::ask(&pool, Kind::Switchover, "bob").await.unwrap();
    assert!(next > id);
    assert!(agent::open(&pool).await.unwrap());
    assert_eq!(
        agent::last(&pool, Kind::Update).await.unwrap().unwrap().id,
        id
    );
    assert_eq!(
        agent::last(&pool, Kind::Switchover)
            .await
            .unwrap()
            .unwrap()
            .id,
        next
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_helper_not_heard_from_lately_takes_nothing(pool: PgPool) {
    agent::beat(&pool, "radio-a").await.unwrap();
    sqlx::query("UPDATE agents SET seen_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        agent::ask(&pool, Kind::Update, "ann").await,
        Err(Refused::NoHelper)
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_request_left_running_is_closed_when_its_helper_starts_again(pool: PgPool) {
    agent::beat(&pool, "radio-a").await.unwrap();
    let id = agent::ask(&pool, Kind::Switchover, "ann").await.unwrap();
    agent::claim(&pool, "radio-a").await.unwrap();
    agent::say(&pool, id, "Stopping db1...").await.unwrap();
    // Another machine's helper leaves it alone.
    assert_eq!(agent::abandoned(&pool, "radio-b").await.unwrap(), 0);
    assert_eq!(agent::abandoned(&pool, "radio-a").await.unwrap(), 1);
    let last = agent::last(&pool, Kind::Switchover).await.unwrap().unwrap();
    assert_eq!(last.ok, Some(false));
    assert!(last.said.starts_with("Stopping db1...\n"), "{}", last.said);
    assert!(
        last.said.contains("stopped before this finished"),
        "{}",
        last.said
    );
    assert!(last.said.contains("radio-a"), "{}", last.said);
    // The next request is open to an admin again.
    assert!(agent::ask(&pool, Kind::Update, "ann").await.is_ok());
}

#[sqlx::test(migrations = "./migrations")]
async fn the_daily_check_is_on_unless_set_off_and_keeps_the_newest_release(pool: PgPool) {
    assert!(agent::checks(&pool).await.unwrap());
    sqlx::query("INSERT INTO settings (key, value) VALUES ('updates.check', 'false')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(!agent::checks(&pool).await.unwrap());
    for version in ["0.9.0", "0.9.1"] {
        let release = scannerpulse::update::Release {
            tag: format!("v{version}"),
            version: version.into(),
            notes: format!("What {version} changed."),
        };
        agent::remember(&pool, &release).await.unwrap();
    }
    let kept: Vec<(String, String)> = sqlx::query_as("SELECT version, notes FROM newest_release")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(kept, vec![("0.9.1".into(), "What 0.9.1 changed.".into())]);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_open_request_is_busy_even_while_its_helper_is_quiet(pool: PgPool) {
    agent::beat(&pool, "radio-a").await.unwrap();
    agent::ask(&pool, Kind::Update, "ann").await.unwrap();
    sqlx::query("UPDATE agents SET seen_at = now() - interval '1 minute'")
        .execute(&pool)
        .await
        .unwrap();
    // The admin is told it is busy, not to install the helper.
    assert!(matches!(
        agent::ask(&pool, Kind::Switchover, "bob").await,
        Err(Refused::Busy)
    ));
}
