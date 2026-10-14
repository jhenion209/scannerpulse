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
async fn a_filed_file_still_in_the_source_waits_again(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    store::mark_done(
        &pool,
        row_id(&pool, "a.mp3").await,
        (1, utc("2031-01-15T18:00:00Z")),
    )
    .await?;
    assert_eq!(row(&pool, "a.mp3").await.0, "done");
    store::upsert_seen(&pool, source, &[seen("a.mp3", 5)]).await?;
    assert_eq!(row(&pool, "a.mp3").await.0, "waiting");
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
fn a_placeholder_never_replaces_a_real_name() {
    assert_eq!(store::renamed("Alpha", "Alpha"), None);
    assert_eq!(
        store::renamed("Alpha", "Alpha Fire"),
        Some("Alpha Fire".to_string())
    );
    assert_eq!(store::renamed("Alpha", "Unidentified 127"), None);
    assert_eq!(
        store::renamed("Unidentified 127", "Alpha"),
        Some("Alpha".to_string())
    );
    assert_eq!(
        store::renamed("Unidentified 127", "Unidentified 127 B"),
        Some("Unidentified 127 B".to_string())
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_renamed_talkgroup_remembers_the_name_its_files_carry(pool: PgPool) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    let system = store::system_id(&mut conn, "SYSA").await?;
    let tg = |name: &str| Talkgroup {
        number: 127,
        name: name.to_string(),
    };
    let names = async |conn: &mut sqlx::PgConnection| -> (String, Option<String>, bool) {
        sqlx::query_as("SELECT name, files_name, transcribe FROM talkgroups WHERE number = 127")
            .fetch_one(conn)
            .await
            .unwrap()
    };
    let id = store::talkgroup_id(&mut conn, system, &tg("Alpha")).await?;
    assert_eq!(names(&mut conn).await, ("Alpha".to_string(), None, false));
    assert_eq!(
        store::talkgroup_id(&mut conn, system, &tg("Alpha Fire")).await?,
        id
    );
    assert_eq!(
        names(&mut conn).await,
        ("Alpha Fire".to_string(), Some("Alpha".to_string()), false)
    );
    // The folders keep the first old name until the rename job runs.
    store::talkgroup_id(&mut conn, system, &tg("Alpha Fire 2")).await?;
    assert_eq!(
        names(&mut conn).await,
        ("Alpha Fire 2".to_string(), Some("Alpha".to_string()), false)
    );
    store::talkgroup_id(&mut conn, system, &tg("Unidentified 127")).await?;
    assert_eq!(names(&mut conn).await.0, "Alpha Fire 2");
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
async fn only_old_rows_of_filed_files_are_pruned(pool: PgPool) -> sqlx::Result<()> {
    let (_, source) = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    store::upsert_seen(
        &pool,
        source,
        &[seen("old.mp3", 5), seen("new.mp3", 5), seen("wait.mp3", 5)],
    )
    .await?;
    for name in ["old.mp3", "new.mp3"] {
        store::mark_done(
            &pool,
            row_id(&pool, name).await,
            (1, utc("2031-01-15T18:00:00Z")),
        )
        .await?;
    }
    sqlx::query("UPDATE ingest_files SET seen_at = now() - interval '8 days' WHERE rel_path IN ('old.mp3', 'wait.mp3')")
        .execute(&pool)
        .await?;
    assert_eq!(store::prune_done(&pool, 7).await?, 1);
    let left: Vec<String> =
        sqlx::query_scalar("SELECT rel_path FROM ingest_files ORDER BY rel_path")
            .fetch_all(&pool)
            .await?;
    assert_eq!(left, ["new.mp3", "wait.mp3"]);
    Ok(())
}
