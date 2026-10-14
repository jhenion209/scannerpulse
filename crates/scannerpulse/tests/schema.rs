use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

/// One system, talkgroup, location and source; returns (talkgroup, location, source).
async fn seed(pool: &PgPool) -> sqlx::Result<(i32, i16, i16)> {
    let system: i32 = sqlx::query_scalar("INSERT INTO systems (name) VALUES ('SYSA') RETURNING id")
        .fetch_one(pool)
        .await?;
    let talkgroup: i32 = sqlx::query_scalar(
        "INSERT INTO talkgroups (system_id, number, name) VALUES ($1, 12, 'Alpha PD') RETURNING id",
    )
    .bind(system)
    .fetch_one(pool)
    .await?;
    let location: i16 = sqlx::query_scalar(
        "INSERT INTO locations (name, root, mode, is_primary) \
         VALUES ('primary', '/arch', 'managed', true) RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    let source: i16 = sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path) VALUES ('live', 'sdrtrunk', '/in') RETURNING id",
    )
    .fetch_one(pool)
    .await?;
    Ok((talkgroup, location, source))
}

const INSERT_CALL: &str = "INSERT INTO calls (started_at, talkgroup_id, source_id, location_id, rel_path) \
     VALUES ($1::timestamptz, $2, $3, $4, $5) RETURNING id, tableoid::regclass::text AS part";

/// Inserts one call; the row holds its `id` and the name of its partition, `part`.
async fn add_call(
    executor: impl sqlx::PgExecutor<'_>,
    (talkgroup, location, source): (i32, i16, i16),
    started_at: &str,
    rel_path: &str,
) -> sqlx::Result<PgRow> {
    sqlx::query(INSERT_CALL)
        .bind(started_at)
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind(rel_path)
        .fetch_one(executor)
        .await
}

#[sqlx::test(migrations = "./migrations")]
async fn migrations_apply_twice_without_change(pool: PgPool) -> sqlx::Result<()> {
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await?;
    assert_eq!(before, after);
    assert!(before >= 1);
    let tables: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE tablename = 'calls'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(tables, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn second_primary_location_is_refused(pool: PgPool) -> sqlx::Result<()> {
    seed(&pool).await?;
    let err = sqlx::query(
        "INSERT INTO locations (name, root, mode, is_primary) VALUES ('other', '/b', 'managed', true)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(err.to_string().contains("locations_one_primary"), "{err}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn readonly_location_cannot_be_primary(pool: PgPool) -> sqlx::Result<()> {
    let err = sqlx::query(
        "INSERT INTO locations (name, root, mode, is_primary) VALUES ('ro', '/c', 'readonly', true)",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("locations_primary_is_managed"),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn talkgroup_number_is_unique_within_a_system(pool: PgPool) -> sqlx::Result<()> {
    seed(&pool).await?;
    let err = sqlx::query(
        "INSERT INTO talkgroups (system_id, number, name) SELECT id, 12, 'Again' FROM systems",
    )
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("talkgroups_system_id_number_key"),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn call_lands_in_its_month_and_fails_without_a_partition(pool: PgPool) -> sqlx::Result<()> {
    let (talkgroup, location, source) = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-15')")
        .execute(&pool)
        .await?;
    // Calling it again must change nothing and must not fail.
    sqlx::query("SELECT ensure_month_partitions('2031-01-01')")
        .execute(&pool)
        .await?;

    let row = sqlx::query(INSERT_CALL)
        .bind("2031-01-31 23:59:59+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-01-31/a.mp3")
        .fetch_one(&pool)
        .await?;
    assert_eq!(row.get::<String, _>("part"), "calls_y2031m01");

    let err = sqlx::query(INSERT_CALL)
        .bind("2031-02-01 00:00:00+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-02-01/b.mp3")
        .fetch_one(&pool)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no partition of relation \"calls\""),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn partition_bounds_are_utc_whatever_the_session_zone(pool: PgPool) -> sqlx::Result<()> {
    let (talkgroup, location, source) = seed(&pool).await?;
    let mut conn = pool.acquire().await?;
    sqlx::query("SET TIME ZONE 'America/Chicago'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SELECT ensure_month_partitions('2031-03-01')")
        .execute(&mut *conn)
        .await?;
    // The first instant of March in UTC is still February in Chicago.
    let row = sqlx::query(INSERT_CALL)
        .bind("2031-03-01 00:00:00+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-03-01/c.mp3")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(row.get::<String, _>("part"), "calls_y2031m03");
    // The last instant of March in UTC.
    let row = sqlx::query(INSERT_CALL)
        .bind("2031-03-31 23:59:59.999+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-03-31/d.mp3")
        .fetch_one(&mut *conn)
        .await?;
    assert_eq!(row.get::<String, _>("part"), "calls_y2031m03");
    // The upper bound is exclusive: the first instant of April is not in March.
    let err = sqlx::query(INSERT_CALL)
        .bind("2031-04-01 00:00:00+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-04-01/e.mp3")
        .fetch_one(&mut *conn)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no partition of relation \"calls\""),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn partition_bounds_are_utc_whatever_the_session_date_style(
    pool: PgPool,
) -> sqlx::Result<()> {
    let ids = seed(&pool).await?;
    let mut conn = pool.acquire().await?;
    // The function sets its own time zone and date style. Without that, it would write
    // the bounds in this session's style, with a zone abbreviation, and PostgreSQL
    // reads IST as Israel Standard Time, not as India's.
    sqlx::query("SET TIME ZONE 'Asia/Kolkata'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SET datestyle = 'SQL, DMY'")
        .execute(&mut *conn)
        .await?;
    sqlx::query("SELECT ensure_month_partitions('2031-07-10')")
        .execute(&mut *conn)
        .await?;
    // The first and the last instant of July in UTC are in July's partition ...
    for (started_at, rel_path) in [
        ("2031-07-01 00:00:00+00", "2031-07-01/f.mp3"),
        ("2031-07-31 23:59:59.999+00", "2031-07-31/g.mp3"),
    ] {
        let row = add_call(&mut *conn, ids, started_at, rel_path).await?;
        assert_eq!(
            row.get::<String, _>("part"),
            "calls_y2031m07",
            "{started_at}"
        );
    }
    // ... and the first instant of August is not.
    let err = add_call(
        &mut *conn,
        ids,
        "2031-08-01 00:00:00+00",
        "2031-08-01/h.mp3",
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("no partition of relation \"calls\""),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn transcript_search_splits_on_every_non_alphanumeric_run(pool: PgPool) -> sqlx::Result<()> {
    let (talkgroup, location, source) = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-01')")
        .execute(&pool)
        .await?;
    let call = sqlx::query(INSERT_CALL)
        .bind("2031-01-10 12:00:00+00")
        .bind(talkgroup)
        .bind(source)
        .bind(location)
        .bind("2031-01-10/e.mp3")
        .fetch_one(&pool)
        .await?;
    let id: i64 = call.get("id");
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin) \
         VALUES ($1, '2031-01-10 12:00:00+00', 'unit-12 en.route to 4th/main', 'machine')",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    for word in ["unit", "12", "en", "route", "4th", "main"] {
        let hit: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM transcripts WHERE search @@ to_tsquery('simple', $1)",
        )
        .bind(word)
        .fetch_one(&pool)
        .await?;
        assert_eq!(hit, 1, "word {word}");
    }
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn one_file_is_one_call_within_a_month(pool: PgPool) -> sqlx::Result<()> {
    let ids = seed(&pool).await?;
    for month in ["2031-01-01", "2031-02-01"] {
        sqlx::query("SELECT ensure_month_partitions($1::date)")
            .bind(month)
            .execute(&pool)
            .await?;
    }
    add_call(&pool, ids, "2031-01-10 12:00:00+00", "2031-01-10/same.mp3").await?;

    // The same file at another time of the same month is the same call ...
    let err = add_call(&pool, ids, "2031-01-20 08:30:00+00", "2031-01-10/same.mp3")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("location_id_rel_path_key"),
        "{err}"
    );
    // ... and so is the same file at the same time.
    let err = add_call(&pool, ids, "2031-01-10 12:00:00+00", "2031-01-10/same.mp3")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("location_id_rel_path_key"),
        "{err}"
    );

    // The rule is per month: each partition carries its own unique constraint, so
    // the same file in the next month is accepted.
    let row = add_call(&pool, ids, "2031-02-10 12:00:00+00", "2031-01-10/same.mp3").await?;
    assert_eq!(row.get::<String, _>("part"), "calls_y2031m02");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_supplied_call_id_is_refused(pool: PgPool) -> sqlx::Result<()> {
    let (_, location, source) = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-01')")
        .execute(&pool)
        .await?;
    let err = sqlx::query(
        "INSERT INTO calls (id, started_at, source_id, location_id, rel_path) \
         VALUES (42, '2031-01-10 12:00:00+00', $1, $2, '2031-01-10/f.mp3')",
    )
    .bind(source)
    .bind(location)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot insert a non-DEFAULT value into column \"id\""),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn deleting_a_call_removes_its_transcript_and_its_label(pool: PgPool) -> sqlx::Result<()> {
    let ids = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-01')")
        .execute(&pool)
        .await?;
    let id: i64 = add_call(&pool, ids, "2031-01-10 12:00:00+00", "2031-01-10/g.mp3")
        .await?
        .get("id");
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin) \
         VALUES ($1, '2031-01-10 12:00:00+00', 'ten four', 'machine')",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, source) \
         VALUES ($1, '2031-01-10 12:00:00+00', 'Ten-four.', 'person')",
    )
    .bind(id)
    .execute(&pool)
    .await?;

    sqlx::query("DELETE FROM calls WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;

    // Both rows belong to the call and go with it.
    let transcripts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(transcripts, 0);
    let labels: i64 = sqlx::query_scalar("SELECT count(*) FROM transcript_labels")
        .fetch_one(&pool)
        .await?;
    assert_eq!(labels, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn two_sessions_creating_the_same_month_both_succeed(pool: PgPool) -> sqlx::Result<()> {
    // Session A creates the month and keeps its transaction open.
    let mut a = pool.begin().await?;
    sqlx::query("SELECT ensure_month_partitions('2031-05-10')")
        .execute(&mut *a)
        .await?;
    // Session B asks for the same month meanwhile. It must wait for A, not fail.
    let b = tokio::spawn({
        let pool = pool.clone();
        async move {
            sqlx::query("SELECT ensure_month_partitions('2031-05-10')")
                .execute(&pool)
                .await
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    a.commit().await?;
    b.await.expect("session B panicked")?;

    let relations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relname = 'calls_y2031m05'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(relations, 1);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_waiting_session_that_created_another_month_also_succeeds(
    pool: PgPool,
) -> sqlx::Result<()> {
    // Session B has already run the creating path once on its connection, so it
    // has nothing new to read from the catalog when it wakes up.
    let mut conn_b = pool.acquire().await?;
    sqlx::query("SELECT ensure_month_partitions('2031-06-10')")
        .execute(&mut *conn_b)
        .await?;
    // Session A creates another month and keeps its transaction open.
    let mut a = pool.begin().await?;
    sqlx::query("SELECT ensure_month_partitions('2031-05-10')")
        .execute(&mut *a)
        .await?;
    // Session B asks for A's month on that same connection. It must wait for A,
    // then see that the month exists, not fail.
    let b = tokio::spawn(async move {
        sqlx::query("SELECT ensure_month_partitions('2031-05-10')")
            .execute(&mut *conn_b)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    a.commit().await?;
    b.await.expect("session B panicked")?;

    let relations: i64 =
        sqlx::query_scalar("SELECT count(*) FROM pg_class WHERE relname = 'calls_y2031m05'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(relations, 1);
    Ok(())
}

// A search lists its newest matches first. With the listing index the database reads
// the newest month first and stops when the page is full; without it, it collects
// every match of every month and sorts them.
#[sqlx::test(migrations = "./migrations")]
async fn newest_transcripts_are_read_through_an_index(pool: PgPool) -> sqlx::Result<()> {
    for day in ["2031-01-15", "2031-02-15"] {
        sqlx::query("SELECT ensure_month_partitions($1::date)")
            .bind(day)
            .execute(&pool)
            .await?;
    }

    // Months created after the migration get the index too. This counts tables, not
    // indexes, so 3 means the parent and both months each have it.
    let tables: i64 = sqlx::query_scalar(
        "SELECT count(DISTINCT tablename) FROM pg_indexes \
         WHERE schemaname = current_schema() \
           AND tablename IN ('transcripts', 'transcripts_y2031m01', 'transcripts_y2031m02') \
           AND indexdef LIKE '%(started_at DESC, call_id DESC)'",
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        tables, 3,
        "the listing index must exist on transcripts and on each of its two month partitions"
    );

    // The index must give the order, so that no sort is needed. SET LOCAL reaches the
    // EXPLAIN only on the same connection and inside the same transaction.
    let mut tx = pool.begin().await?;
    for setting in [
        "SET LOCAL enable_seqscan = off",
        "SET LOCAL enable_bitmapscan = off",
        "SET LOCAL enable_sort = off",
    ] {
        sqlx::query(setting).execute(&mut *tx).await?;
    }
    // EXPLAIN answers with the type json: read it as text, without sqlx's type check.
    let plan: String = sqlx::query(
        "EXPLAIN (FORMAT JSON) \
         SELECT call_id, started_at FROM transcripts \
         WHERE search @@ to_tsquery('simple', 'unit') \
         ORDER BY started_at DESC, call_id DESC LIMIT 100",
    )
    .fetch_one(&mut *tx)
    .await?
    .try_get_unchecked(0)?;
    tx.rollback().await?;
    assert!(
        !plan.contains("\"Node Type\": \"Sort\""),
        "the newest-first query needs a sort: the listing index is missing \
         or does not give the order\n{plan}"
    );
    assert!(
        !plan.contains("\"Node Type\": \"Incremental Sort\""),
        "the newest-first query needs an incremental sort: the listing index is missing \
         or gives only part of the order\n{plan}"
    );
    // The two checks above would also pass if a later PostgreSQL spelled its plan
    // differently, so the plan must also show the index being read.
    assert!(
        plan.contains("\"Node Type\": \"Index Scan\""),
        "the newest-first query does not read through an index: the plan's format changed, \
         or the listing index is not used\n{plan}"
    );
    Ok(())
}

// Migration 0003: ingest.

const INSERT_INGEST_FILE: &str = "INSERT INTO ingest_files (source_id, rel_path, first_seen, seen_at, state) \
     VALUES ($1, 'a.mp3', now(), now(), $2)";

#[sqlx::test(migrations = "./migrations")]
async fn a_file_has_one_ingest_row_per_source(pool: PgPool) -> sqlx::Result<()> {
    let (_, _, source) = seed(&pool).await?;
    sqlx::query(INSERT_INGEST_FILE)
        .bind(source)
        .bind("waiting")
        .execute(&pool)
        .await?;
    let err = sqlx::query(INSERT_INGEST_FILE)
        .bind(source)
        .bind("waiting")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("ingest_files_source_id_rel_path_key"),
        "{err}"
    );
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_ingest_row_is_waiting_done_or_rejected(pool: PgPool) -> sqlx::Result<()> {
    let (_, _, source) = seed(&pool).await?;
    let err = sqlx::query(INSERT_INGEST_FILE)
        .bind(source)
        .bind("filed")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ingest_files_state"), "{err}");
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_queued_call_leaves_the_queue_with_its_call(pool: PgPool) -> sqlx::Result<()> {
    let ids = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-15')")
        .execute(&pool)
        .await?;
    let call = add_call(&pool, ids, "2031-01-15 12:00:00+00", "2031-01-15/a.mp3").await?;
    let id: i64 = call.get("id");
    sqlx::query(
        "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
         VALUES ($1, '2031-01-15 12:00:00+00', now(), now(), 'queued')",
    )
    .bind(id)
    .execute(&pool)
    .await?;
    sqlx::query("DELETE FROM calls WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM transcription_queue")
        .fetch_one(&pool)
        .await?;
    assert_eq!(left, 0, "the queue row must go when its call is deleted");
    Ok(())
}
