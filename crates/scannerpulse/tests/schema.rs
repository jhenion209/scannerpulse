use sqlx::{PgPool, Row};

/// One system, talkgroup, location and source; returns (talkgroup, location, source).
async fn seed(pool: &PgPool) -> sqlx::Result<(i32, i16, i16)> {
    let system: i16 = sqlx::query_scalar("INSERT INTO systems (name) VALUES ('SYSA') RETURNING id")
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
    // Deleting the call removes its transcript.
    sqlx::query("DELETE FROM calls WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await?;
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(left, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn same_path_twice_in_one_location_is_refused(pool: PgPool) -> sqlx::Result<()> {
    let (talkgroup, location, source) = seed(&pool).await?;
    sqlx::query("SELECT ensure_month_partitions('2031-01-01')")
        .execute(&pool)
        .await?;
    for attempt in 0..2 {
        let result = sqlx::query(INSERT_CALL)
            .bind("2031-01-10 12:00:00+00")
            .bind(talkgroup)
            .bind(source)
            .bind(location)
            .bind("2031-01-10/same.mp3")
            .fetch_one(&pool)
            .await;
        assert_eq!(result.is_ok(), attempt == 0);
    }
    Ok(())
}
