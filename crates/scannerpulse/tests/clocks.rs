mod common;

use chrono::{DateTime, Duration, Utc};
use scannerpulse::clocks::{self, Checked};
use scannerpulse::store;
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

/// A recorder named `name`, and the archive's location the first time.
async fn recorder(pool: &PgPool, name: &str) -> i16 {
    sqlx::query(
        "INSERT INTO locations (name, root, mode, is_primary) \
         VALUES ('primary', '/srv/archive', 'managed', true) ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path) VALUES ($1, 'sdrtrunk', '/srv/' || $1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A call of `source` its file's name starts at `named`, `length_ms` long,
/// first seen at `seen`, filed.
async fn filed(
    pool: &PgPool,
    source: i16,
    named: DateTime<Utc>,
    length_ms: i32,
    seen: DateTime<Utc>,
) -> (i64, DateTime<Utc>) {
    store::ensure_partition(pool, named).await.unwrap();
    let path = format!("{source}/{}.mp3", named.timestamp_millis());
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO calls (started_at, duration_ms, source_id, location_id, rel_path) \
         VALUES ($1, $2, $3, (SELECT id FROM locations LIMIT 1), $4) RETURNING id",
    )
    .bind(named)
    .bind(length_ms)
    .bind(source)
    .bind(&path)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, call_id, \
                                   call_started_at, named_at) \
         VALUES ($1, $2, $3, $3, 'done', $4, $5, $5)",
    )
    .bind(source)
    .bind(&path)
    .bind(seen)
    .bind(id)
    .bind(named)
    .execute(pool)
    .await
    .unwrap();
    (id, named)
}

/// `source`'s copy of `call`, its name starting at `named`, deleted as a
/// duplicate.
async fn copied(pool: &PgPool, source: i16, call: (i64, DateTime<Utc>), named: DateTime<Utc>) {
    sqlx::query(
        "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state, call_id, \
                                   call_started_at, named_at) \
         VALUES ($1, $2, now(), now(), 'duplicate', $3, $4, $5)",
    )
    .bind(source)
    .bind(format!("copy-{}.mp3", call.0))
    .bind(call.0)
    .bind(call.1)
    .bind(named)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE calls SET copies_from = coalesce(copies_from, '{}') || $3::smallint \
         WHERE id = $1 AND started_at = $2",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(source)
    .execute(pool)
    .await
    .unwrap();
}

async fn recorder_warnings(pool: &PgPool) -> Vec<String> {
    clocks::recorder_warnings(&mut pool.acquire().await.unwrap())
        .await
        .unwrap()
        .into_iter()
        .map(|w| format!("{}: {}", w.subject, w.text))
        .collect()
}

fn ago(seconds: i64) -> DateTime<Utc> {
    // Names give whole seconds.
    let at = Utc::now() - Duration::seconds(seconds);
    DateTime::from_timestamp(at.timestamp(), 0).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_whose_files_are_seen_before_their_names_say_is_ahead(pool: PgPool) {
    let (north, south, few) = (
        recorder(&pool, "north").await,
        recorder(&pool, "south").await,
        recorder(&pool, "few").await,
    );
    let old = recorder(&pool, "older-archive").await;
    sqlx::query("INSERT INTO imports (source_id) VALUES ($1)")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap();
    // A file is seen once it has been still for 2 s, so once its call has
    // ended: a 2 s call named 4 s before it is seen is on time.
    for minute in 1..=6 {
        let seen = ago(minute * 60);
        // North's names run 3 s ahead.
        filed(&pool, north, seen - Duration::seconds(1), 2_000, seen).await;
        // South's are seen a second late.
        filed(&pool, south, seen - Duration::seconds(5), 2_000, seen).await;
        // An import's names are old, whatever its offset.
        filed(&pool, old, seen + Duration::seconds(30), 2_000, seen).await;
    }
    // Too few files to tell.
    for minute in 1..=4 {
        let seen = ago(minute * 60);
        filed(&pool, few, seen - Duration::seconds(1), 2_000, seen).await;
    }
    // A backlog brought in hours late is no clock's doing: south's last two
    // hours' files, seen now.
    for minute in 1..=20 {
        let named = ago(7200 + minute * 60);
        filed(&pool, south, named, 2_000, ago(30)).await;
    }
    assert_eq!(
        recorder_warnings(&pool).await,
        [
            "north: its clock is 3.0 s ahead of the database's: its files are seen before their calls end"
        ]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorder_whose_files_are_seen_long_after_their_calls_end_is_named(pool: PgPool) {
    let (north, asked) = (
        recorder(&pool, "north").await,
        recorder(&pool, "asked").await,
    );
    // A recorder taken in when asked brings its files in late: it has no
    // such measure.
    sqlx::query("UPDATE sources SET intake = 'asked' WHERE id = $1")
        .bind(asked)
        .execute(&pool)
        .await
        .unwrap();
    for minute in 2..=7 {
        let seen = ago(minute * 60);
        filed(&pool, north, seen - Duration::seconds(97), 5_000, seen).await;
        filed(&pool, asked, seen - Duration::seconds(97), 5_000, seen).await;
    }
    assert_eq!(
        recorder_warnings(&pool).await,
        [
            "north: its files are seen 90.0 s after their calls end: its clock may be behind, or its files slow to arrive"
        ]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn two_recorders_whose_copies_differ_by_seconds_are_named_once(pool: PgPool) {
    let (north, south, east) = (
        recorder(&pool, "north").await,
        recorder(&pool, "south").await,
        recorder(&pool, "east").await,
    );
    // North's clock runs 2 s behind, which the files' delay does not show
    // (well under a minute); south's names run 2 s ahead of north's: three
    // of north's calls south had copies of, three of south's north had
    // copies of.
    for minute in 1..=6 {
        let seen = ago(minute * 60);
        let start = seen - Duration::seconds(7);
        if minute <= 3 {
            let call = filed(&pool, north, start, 3_000, seen).await;
            copied(&pool, south, call, start + Duration::seconds(2)).await;
        } else {
            let call = filed(&pool, south, start + Duration::seconds(2), 3_000, seen).await;
            copied(&pool, north, call, start).await;
        }
    }
    // East's run 2 s ahead too, but three copies are too few to tell.
    for minute in 1..=3 {
        let seen = ago(minute * 60 + 30);
        let call = filed(&pool, north, seen - Duration::seconds(5), 3_000, seen).await;
        copied(&pool, east, call, seen - Duration::seconds(3)).await;
    }
    assert_eq!(
        recorder_warnings(&pool).await,
        ["south: its clock is 2.0 s ahead of north's (6 copies): one call can be kept as two"]
    );
    // The Recorders area shows it on south's row.
    let listed = scannerpulse::recorders::list(&mut pool.acquire().await.unwrap(), ago(3600))
        .await
        .unwrap();
    let clocks: Vec<(&str, Option<&str>)> = listed
        .iter()
        .map(|r| (r.name.as_str(), r.clock.as_deref()))
        .collect();
    assert_eq!(
        clocks,
        [
            ("north", None),
            (
                "south",
                Some("its clock is 2.0 s ahead of north's (6 copies): one call can be kept as two")
            ),
            ("east", None)
        ]
    );
}
