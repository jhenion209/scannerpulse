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
    pool: &PgPool,
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
        .fetch_one(pool)
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
