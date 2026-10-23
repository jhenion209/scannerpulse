mod common;

use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use scannerpulse::localtime::TimeHow;
use scannerpulse::reader::{Described, FileCall, Talkgroup};
use scannerpulse::store::{self, NewCall, RecordError, Seen};
use sqlx::PgPool;
use sqlx::postgres::PgListener;

const HOLDER: &str = "host-a 100-1900000000";

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn seen(rel_path: &str, size: i64) -> Seen {
    Seen {
        rel_path: rel_path.to_string(),
        size,
        file_mtime: utc("2031-01-15T18:00:05Z"),
    }
}

fn file_call(described: Described) -> FileCall {
    FileCall {
        local_start: utc("2031-01-15T12:00:00Z").naive_utc(),
        started_at: utc("2031-01-15T18:00:00Z"),
        time_how: TimeHow::Plain,
        duration_ms: 1008,
        file_mtime: utc("2031-01-15T18:00:05Z"),
        described,
    }
}

fn alpha() -> Described {
    Described {
        system: Some("SYSA".to_string()),
        site: Some("North".to_string()),
        talkgroup: Some(Talkgroup {
            number: 127,
            name: "Alpha Fire".to_string(),
        }),
        title: None,
        radio_id: Some(711351),
        radio_alias: Some("Engine 5".to_string()),
    }
}

/// state, reason, first_seen, seen_at of the row of `rel_path`.
async fn row(
    pool: &PgPool,
    rel_path: &str,
) -> (String, Option<String>, DateTime<Utc>, DateTime<Utc>) {
    sqlx::query_as(
        "SELECT state, reason, first_seen, seen_at FROM ingest_files WHERE rel_path = $1",
    )
    .bind(rel_path)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn row_id(pool: &PgPool, rel_path: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM ingest_files WHERE rel_path = $1")
        .bind(rel_path)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn the_sources_and_the_primary_location_are_read(pool: PgPool) -> sqlx::Result<()> {
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    sqlx::query(
        "INSERT INTO sources (name, kind, path, enabled) VALUES ('off', 'sdrtrunk', '/off', false)",
    )
    .execute(&pool)
    .await?;
    let primary = store::primary_location(&pool).await?.unwrap();
    assert_eq!(
        (primary.id, primary.root.as_path()),
        (location, Path::new("/arch"))
    );
    let sources = store::enabled_sources(&pool).await?;
    assert_eq!(sources.len(), 1);
    assert_eq!(
        (sources[0].id, sources[0].path.as_path()),
        (source, Path::new("/in"))
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_file_waits_and_a_known_one_is_refreshed(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let (state, _, first, seen_at) = row(&pool, "a.mp3").await;
    assert_eq!(state, "waiting");
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let (state, _, first_again, seen_again) = row(&pool, "a.mp3").await;
    assert_eq!(state, "waiting");
    assert_eq!(first_again, first);
    assert!(seen_again > seen_at);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rejected_file_waits_again_only_when_it_changes(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    store::reject(&pool, row_id(&pool, "a.mp3").await, "no frame").await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let (state, reason, ..) = row(&pool, "a.mp3").await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("rejected", Some("no frame"))
    );
    store::upsert_seen(&pool, source, &[seen("a.mp3", 6)]).await?;
    let (state, reason, ..) = row(&pool, "a.mp3").await;
    assert_eq!((state.as_str(), reason), ("waiting", None));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_file_rejected_as_missing_waits_again_when_it_comes_back(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    store::reject(&pool, row_id(&pool, "a.mp3").await, "missing").await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let (state, reason, ..) = row(&pool, "a.mp3").await;
    assert_eq!((state.as_str(), reason), ("waiting", None));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_filed_file_still_in_the_source_waits_again(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    store::mark_done(
        &pool,
        row_id(&pool, "a.mp3").await,
        (1, utc("2031-01-15T18:00:00Z")),
        None,
    )
    .await?;
    assert_eq!(row(&pool, "a.mp3").await.0, "done");
    let call = || async {
        sqlx::query_scalar::<_, Option<i64>>("SELECT call_id FROM ingest_files")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    assert_eq!(row(&pool, "a.mp3").await.0, "waiting");
    assert_eq!(
        call().await,
        Some(1),
        "unchanged, it is still the call's file"
    );
    // Another file under the name is another call.
    store::upsert_seen(&pool, source, &[seen("a.mp3", 6)]).await?;
    assert_eq!(call().await, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_copy_deleted_as_a_duplicate_stays_so_until_it_changes(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    sqlx::query(
        "UPDATE ingest_files SET state = 'duplicate', reason = 'a copy', call_id = 1, \
         call_started_at = '2031-01-15T18:00:00Z'",
    )
    .execute(&pool)
    .await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let (state, reason, ..) = row(&pool, "a.mp3").await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("duplicate", Some("a copy"))
    );
    store::upsert_seen(&pool, source, &[seen("a.mp3", 6)]).await?;
    let (state, reason, ..) = row(&pool, "a.mp3").await;
    assert_eq!((state.as_str(), reason), ("waiting", None));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn waiting_files_come_oldest_first_without_the_skipped_ones(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("b.mp3", 5)]).await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let names =
        |rows: Vec<store::Waiting>| rows.into_iter().map(|r| r.rel_path).collect::<Vec<_>>();
    assert_eq!(
        names(store::waiting(&pool, 10, &[]).await?),
        ["b.mp3", "a.mp3"]
    );
    assert_eq!(names(store::waiting(&pool, 1, &[]).await?), ["b.mp3"]);
    let b = row_id(&pool, "b.mp3").await;
    assert_eq!(names(store::waiting(&pool, 10, &[b]).await?), ["a.mp3"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_lookup_uses_no_identity_value_when_the_row_exists(pool: PgPool) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    let first = store::system_id(&mut conn, "SYSA").await?;
    assert_eq!(store::system_id(&mut conn, "SYSA").await?, first);
    assert_eq!(store::system_id(&mut conn, "SYSB").await?, first + 1);
    let site = store::site_id(&mut conn, first, "North").await?;
    assert_eq!(store::site_id(&mut conn, first, "North").await?, site);
    Ok(())
}

#[test]
fn a_different_name_is_a_proposal_and_a_placeholder_is_no_name() {
    use store::NameSeen::{Propose, Rename, Same};
    assert_eq!(store::name_seen("Alpha", "Alpha"), Same);
    assert_eq!(store::name_seen("Alpha", "Alpha Fire"), Propose);
    assert_eq!(store::name_seen("Alpha", "Unidentified 127"), Same);
    assert_eq!(store::name_seen("Unidentified 127", "Alpha"), Rename);
    assert_eq!(
        store::name_seen("Unidentified 127", "Unidentified 127 B"),
        Same
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorders_different_name_is_proposed_and_renames_nothing(
    pool: PgPool,
) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    let system = store::system_id(&mut conn, "SYSA").await?;
    let tg = |name: &str| Talkgroup {
        number: 127,
        name: name.to_string(),
    };
    let names = async |conn: &mut sqlx::PgConnection| -> (String, Option<String>, bool) {
        sqlx::query_as("SELECT name, files_name, files_correct FROM talkgroups WHERE number = 127")
            .fetch_one(conn)
            .await
            .unwrap()
    };
    let id = store::talkgroup_id(&mut conn, system, &tg("Alpha"), 1).await?;
    assert_eq!(names(&mut conn).await, ("Alpha".to_string(), None, true));
    assert_eq!(
        store::talkgroup_id(&mut conn, system, &tg("Alpha Fire"), 1).await?,
        id
    );
    store::talkgroup_id(&mut conn, system, &tg("Alpha Fire"), 2).await?;
    store::talkgroup_id(&mut conn, system, &tg("Alpha Fire"), 2).await?;
    // The talkgroup keeps its name; the file is filed under the one it
    // carries, so its files wait for the correction.
    assert_eq!(
        names(&mut conn).await,
        ("Alpha".to_string(), Some("Alpha Fire".to_string()), false)
    );
    let proposal: (String, Vec<i16>, i32, bool) = sqlx::query_as(
        "SELECT name, source_ids, calls, dismissed FROM name_proposals WHERE talkgroup_id = $1",
    )
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    assert_eq!(proposal, ("Alpha Fire".to_string(), vec![1, 2], 3, false));
    store::talkgroup_id(&mut conn, system, &tg("Unidentified 127"), 1).await?;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM name_proposals")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(count, 1, "a placeholder is never proposed");
    // A placeholder gives way to a real name at once.
    let other = store::talkgroup_id(
        &mut conn,
        system,
        &Talkgroup {
            number: 128,
            name: "Unidentified 128".into(),
        },
        1,
    )
    .await?;
    store::talkgroup_id(
        &mut conn,
        system,
        &Talkgroup {
            number: 128,
            name: "Bravo".into(),
        },
        1,
    )
    .await?;
    let renamed: String = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(other)
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(renamed, "Bravo");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_system_is_known_by_its_name_or_a_mapped_spelling_and_the_first_is_taken_at_once(
    pool: PgPool,
) -> sqlx::Result<()> {
    assert!(
        store::system_known(&pool, "SYSA").await?,
        "no system yet: the first is taken"
    );
    let mut conn = pool.acquire().await?;
    let sysa = store::system_id(&mut conn, "SYSA").await?;
    assert!(store::system_known(&pool, "SYSA").await?);
    assert!(!store::system_known(&pool, "SYS-A").await?);
    sqlx::query("INSERT INTO system_spellings (name, system_id) VALUES ('SYS-A', $1)")
        .bind(sysa)
        .execute(&pool)
        .await?;
    assert!(store::system_known(&pool, "SYS-A").await?);
    assert_eq!(
        store::resolve_system(&mut conn, "SYS-A").await?,
        (sysa, true)
    );
    assert_eq!(
        store::resolve_system(&mut conn, "SYSA").await?,
        (sysa, false)
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_of_a_mapped_spelling_files_under_its_system_and_waits_for_the_correction(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    common::hold_lease(&pool, HOLDER).await;
    let mut conn = pool.acquire().await?;
    let sysa = store::system_id(&mut conn, "SYSA").await?;
    sqlx::query("INSERT INTO system_spellings (name, system_id) VALUES ('SYS-A', $1)")
        .bind(sysa)
        .execute(&pool)
        .await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let mut described = alpha();
    described.system = Some("SYS-A".to_string());
    let call = file_call(described);
    store::ensure_partition(&pool, call.started_at).await?;
    let new = NewCall {
        call: &call,
        source_id: source,
        location_id: location,
        rel_path: "2031-01-15/7-Alpha/a.mp3",
        transcribe_from_ms: None,
    };
    let (id, started_at) = store::record(&mut conn, HOLDER, row_id(&pool, "a.mp3").await, &new)
        .await
        .unwrap();
    let (system, files_correct): (i32, bool) = sqlx::query_as(
        "SELECT t.system_id, t.files_correct FROM calls c JOIN talkgroups t ON t.id = c.talkgroup_id          WHERE c.id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert_eq!((system, files_correct), (sysa, false));
    let mapped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mapped_calls WHERE call_id = $1 AND started_at = $2",
    )
    .bind(id)
    .bind(started_at)
    .fetch_one(&pool)
    .await?;
    assert_eq!(mapped, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn recording_a_call_marks_its_file_done_and_announces_it(pool: PgPool) -> sqlx::Result<()> {
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    common::hold_lease(&pool, HOLDER).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let file = row_id(&pool, "a.mp3").await;
    let call = file_call(alpha());
    store::ensure_partition(&pool, call.started_at).await?;
    let mut listener = PgListener::connect_with(&pool).await?;
    listener.listen("scannerpulse_calls").await?;

    let mut conn = pool.acquire().await?;
    let new = NewCall {
        call: &call,
        source_id: source,
        location_id: location,
        rel_path: "2031-01-15/127-Alpha Fire/a.mp3",
        transcribe_from_ms: None,
    };
    let (id, started_at) = store::record(&mut conn, HOLDER, file, &new).await.unwrap();
    assert_eq!(started_at, call.started_at);

    let (tg_name, site, radio_id, alias, title, duration, rel_path, mtime): (
        String,
        String,
        i64,
        String,
        Option<String>,
        i32,
        String,
        DateTime<Utc>,
    ) = sqlx::query_as(
        "SELECT tg.name, si.name, c.radio_id, c.radio_alias, c.title, c.duration_ms, c.rel_path, c.file_mtime \
         FROM calls c JOIN talkgroups tg ON tg.id = c.talkgroup_id JOIN sites si ON si.id = c.site_id \
         WHERE c.id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        (
            tg_name.as_str(),
            site.as_str(),
            radio_id,
            alias.as_str(),
            title,
            duration
        ),
        ("Alpha Fire", "North", 711351, "Engine 5", None, 1008)
    );
    assert_eq!(rel_path, "2031-01-15/127-Alpha Fire/a.mp3");
    assert_eq!(mtime, call.file_mtime);

    let (state, call_id): (String, Option<i64>) =
        sqlx::query_as("SELECT state, call_id FROM ingest_files WHERE id = $1")
            .bind(file)
            .fetch_one(&pool)
            .await?;
    assert_eq!((state.as_str(), call_id), ("done", Some(id)));
    assert_eq!(
        store::call_at(&pool, location, &rel_path, call.started_at).await?,
        Some((id, call.started_at))
    );

    let notice = tokio::time::timeout(Duration::from_secs(5), listener.recv())
        .await
        .expect("no notice within 5 s")?;
    assert_eq!(notice.payload(), id.to_string());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn without_the_lease_nothing_is_recorded(pool: PgPool) -> sqlx::Result<()> {
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    let file = row_id(&pool, "a.mp3").await;
    let call = file_call(alpha());
    store::ensure_partition(&pool, call.started_at).await?;
    let new = NewCall {
        call: &call,
        source_id: source,
        location_id: location,
        rel_path: "2031-01-15/127-Alpha Fire/a.mp3",
        transcribe_from_ms: None,
    };
    let mut conn = pool.acquire().await?;
    // No lease row; another holder; an expired lease.
    for setup in [
        None,
        Some("UPDATE leases SET holder = 'host-b 7-7'"),
        Some("UPDATE leases SET holder = $1, expires_at = now() - interval '1 second'"),
    ] {
        if let Some(sql) = setup {
            common::hold_lease(&pool, HOLDER).await;
            sqlx::query(sql).bind(HOLDER).execute(&pool).await?;
        }
        let err = store::record(&mut conn, HOLDER, file, &new)
            .await
            .unwrap_err();
        assert!(matches!(err, RecordError::LeaseLost), "{err}");
    }
    let calls: i64 = sqlx::query_scalar("SELECT count(*) FROM calls")
        .fetch_one(&pool)
        .await?;
    let systems: i64 = sqlx::query_scalar("SELECT count(*) FROM systems")
        .fetch_one(&pool)
        .await?;
    assert_eq!((calls, systems), (0, 0));
    assert_eq!(row(&pool, "a.mp3").await.0, "waiting");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn waiting_and_rejected_files_are_counted(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(
        &pool,
        source,
        &[seen("a.mp3", 5), seen("b.mp3", 5), seen("c.mp3", 5)],
    )
    .await?;
    store::reject(&pool, row_id(&pool, "a.mp3").await, "no frame").await?;
    store::mark_done(
        &pool,
        row_id(&pool, "b.mp3").await,
        (1, utc("2031-01-15T18:00:00Z")),
        None,
    )
    .await?;
    assert_eq!(store::counts(&pool).await?, (1, 1));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn only_old_rows_of_filed_files_are_pruned(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(
        &pool,
        source,
        &[
            seen("old.mp3", 5),
            seen("new.mp3", 5),
            seen("wait.mp3", 5),
            seen("copy.mp3", 5),
        ],
    )
    .await?;
    for name in ["old.mp3", "new.mp3"] {
        store::mark_done(
            &pool,
            row_id(&pool, name).await,
            (1, utc("2031-01-15T18:00:00Z")),
            None,
        )
        .await?;
    }
    sqlx::query("UPDATE ingest_files SET state = 'duplicate' WHERE rel_path = 'copy.mp3'")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE ingest_files SET seen_at = now() - interval '8 days' WHERE rel_path IN ('old.mp3', 'wait.mp3', 'copy.mp3')")
        .execute(&pool)
        .await?;
    assert_eq!(store::prune_done(&pool, 7).await?, 2);
    let left: Vec<String> =
        sqlx::query_scalar("SELECT rel_path FROM ingest_files ORDER BY rel_path")
            .fetch_all(&pool)
            .await?;
    assert_eq!(left, ["new.mp3", "wait.mp3"]);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_recorded_call_is_queued_when_long_enough_and_its_talkgroup_is_on(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (location, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    common::hold_lease(&pool, HOLDER).await;
    // SYSA/127 with transcription on, as the admin page would leave it.
    let system: i32 = sqlx::query_scalar("INSERT INTO systems (name) VALUES ('SYSA') RETURNING id")
        .fetch_one(&pool)
        .await?;
    sqlx::query("INSERT INTO talkgroups (system_id, number, name, transcribe) VALUES ($1, 127, 'Alpha Fire', true)")
        .bind(system)
        .execute(&pool)
        .await?;
    let call = file_call(alpha()); // 1008 ms long
    store::ensure_partition(&pool, call.started_at).await?;
    let record = async |n: usize, from: Option<i32>| -> bool {
        let rel = format!("2031-01-15/127-Alpha Fire/{n}.mp3");
        store::upsert_seen(&pool, source, &[seen(&rel, 5)])
            .await
            .unwrap();
        let file = row_id(&pool, &rel).await;
        let new = NewCall {
            call: &call,
            source_id: source,
            location_id: location,
            rel_path: &rel,
            transcribe_from_ms: from,
        };
        let mut conn = pool.acquire().await.unwrap();
        let (id, _) = store::record(&mut conn, HOLDER, file, &new).await.unwrap();
        let queued: Option<String> =
            sqlx::query_scalar("SELECT state FROM transcription_queue WHERE call_id = $1")
                .bind(id)
                .fetch_optional(&pool)
                .await
                .unwrap();
        queued.is_some()
    };
    assert!(record(0, Some(1000)).await, "long enough");
    assert!(record(1, Some(1008)).await, "exactly the minimum");
    assert!(!record(2, Some(1009)).await, "too short");
    assert!(!record(3, None).await, "transcription off");
    sqlx::query("UPDATE talkgroups SET transcribe = false")
        .execute(&pool)
        .await?;
    assert!(
        !record(4, Some(1000)).await,
        "the talkgroup's transcription off"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn waiting_files_carry_their_recorders_folder_and_skip_removed_recorders(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    let other: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path) VALUES ('east', 'sdrtrunk', '/east') RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    store::upsert_seen(&pool, other, &[seen("b.mp3", 5)]).await?;
    let rows = |rows: Vec<store::Waiting>| {
        rows.into_iter()
            .map(|r| (r.source_path, r.rel_path))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(store::waiting(&pool, 10, &[]).await?),
        [
            ("/in".to_string(), "a.mp3".to_string()),
            ("/east".to_string(), "b.mp3".to_string())
        ]
    );
    sqlx::query("UPDATE sources SET removed_at = now(), enabled = false WHERE id = $1")
        .bind(other)
        .execute(&pool)
        .await?;
    assert_eq!(
        rows(store::waiting(&pool, 10, &[]).await?),
        [("/in".to_string(), "a.mp3".to_string())]
    );
    // A host that has not heard of the removal records nothing more.
    store::upsert_seen(&pool, other, &[seen("c.mp3", 5)]).await?;
    let c: i64 = sqlx::query_scalar("SELECT count(*) FROM ingest_files WHERE rel_path = 'c.mp3'")
        .fetch_one(&pool)
        .await?;
    assert_eq!(c, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn the_watcher_lists_watched_and_asked_recorders_and_notes_what_it_brought_in(
    pool: PgPool,
) -> sqlx::Result<()> {
    let (_, live) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    let asked_at = utc("2031-01-15T18:00:00Z");
    let ids: Vec<i16> = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path, enabled, intake, bring_in_at, removed_at) VALUES \
         ('paused', 'sdrtrunk', '/p', false, 'watched', NULL, NULL), \
         ('asked', 'sdrtrunk', '/a', false, 'asked', $1, NULL), \
         ('gone', 'sdrtrunk', '/g', false, 'asked', NULL, now()) RETURNING id",
    )
    .bind(asked_at)
    .fetch_all(&pool)
    .await?;
    let asked = ids[1];
    let listed = store::listed_recorders(&pool).await?;
    let seen: Vec<(i16, bool, Option<DateTime<Utc>>)> = listed
        .iter()
        .map(|l| (l.source.id, l.asked, l.bring_in))
        .collect();
    assert_eq!(seen, [(live, false, None), (asked, true, Some(asked_at))]);
    // A request newer than the one listed stays to be listed.
    store::brought_in(&pool, asked, utc("2031-01-15T17:00:00Z")).await?;
    assert_eq!(
        store::listed_recorders(&pool).await?[1].bring_in,
        Some(asked_at)
    );
    store::brought_in(&pool, asked, asked_at).await?;
    assert_eq!(store::listed_recorders(&pool).await?[1].bring_in, None);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn each_host_writes_what_it_found_in_each_folder(pool: PgPool) -> sqlx::Result<()> {
    let (_, live) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    let gone: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path, enabled, removed_at) \
         VALUES ('gone', 'sdrtrunk', '/g', false, now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await?;
    store::folder_health(&pool, "host-a", &[(live, Ok(3)), (gone, Ok(1))]).await?;
    store::folder_health(
        &pool,
        "host-b",
        &[(live, Err("cannot be listed: gone".into()))],
    )
    .await?;
    store::folder_health(&pool, "host-a", &[(live, Ok(4))]).await?;
    type Row = (String, i16, bool, Option<String>, Option<i32>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT host, source_id, ok, problem, files FROM source_health ORDER BY host",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        rows,
        [
            ("host-a".to_string(), live, true, None, Some(4)),
            (
                "host-b".to_string(),
                live,
                false,
                Some("cannot be listed: gone".to_string()),
                None
            ),
        ],
        "a removed recorder gets no row"
    );
    Ok(())
}
