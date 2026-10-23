mod common;

use scannerpulse::clocks::{self, Checked};
use sqlx::PgPool;

fn checked(host: &str, role: &str) -> Checked {
    Checked {
        host: host.into(),
        role: role.into(),
        ntp: Some(true),
        synchronized: Some(true),
        offset_ms: Some(40),
        problem: None,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_machine_whose_clock_is_off_is_named_with_its_offset(pool: PgPool) {
    clocks::record(&pool, &checked("web-a", "scannerpulse"))
        .await
        .unwrap();
    for row in [
        Checked {
            ntp: Some(false),
            synchronized: Some(false),
            ..checked("db1", "database")
        },
        // A container's clock is its host's: no time service of its own,
        // and synchronized.
        Checked {
            ntp: Some(false),
            ..checked("ct1", "scannerpulse")
        },
        Checked {
            synchronized: Some(false),
            ..checked("db2", "database")
        },
        Checked {
            offset_ms: Some(-2_400),
            ..checked("web-b", "scannerpulse")
        },
        Checked {
            ntp: None,
            synchronized: None,
            offset_ms: None,
            problem: Some("did not answer within 20 s".into()),
            ..checked("referee", "referee")
        },
    ] {
        clocks::record(&pool, &row).await.unwrap();
    }
    // A machine checked again keeps one row.
    clocks::record(&pool, &checked("web-a", "scannerpulse"))
        .await
        .unwrap();
    let warnings: Vec<String> = clocks::machine_warnings(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|w| format!("{}: {}", w.subject, w.text))
        .collect();
    assert_eq!(
        warnings,
        [
            "db1: its time service is off",
            "db2: its clock is not synchronized by NTP",
            "referee: its clock could not be checked: did not answer within 20 s",
            "web-b: its clock is 2.4 s behind the database's",
        ]
    );
    // A row not checked again for half an hour says so.
    sqlx::query(
        "UPDATE clock_checks SET checked_at = now() - interval '45 minutes' WHERE host = 'web-a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let stale: Vec<String> = clocks::machine_warnings(&pool)
        .await
        .unwrap()
        .into_iter()
        .filter(|w| w.subject == "web-a")
        .map(|w| w.text)
        .collect();
    assert_eq!(stale, ["its clock was last checked 45 minutes ago"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_scannerpulse_machine_checks_its_own_clock_against_the_database(pool: PgPool) {
    let own = clocks::own(&pool, "web-a").await.unwrap();
    assert_eq!(
        (own.host.as_str(), own.role.as_str()),
        ("web-a", "scannerpulse")
    );
    // The test machine and the database share a time service: well within a
    // second, the round trip and the clocks' resolution included.
    let offset = own.offset_ms.unwrap();
    assert!(offset.abs() < 1_000, "{offset}");
    assert_eq!(own.problem, None);
}
