mod common;

use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use scannerpulse::backfill::{self, Ask};
use sqlx::PgPool;

const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn ask(from: &str, to: &str) -> Ask {
    Ask {
        system: "SYSA".to_string(),
        number: 127,
        from: from.parse::<NaiveDate>().unwrap(),
        to: to.parse::<NaiveDate>().unwrap(),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_local_days_are_counted_both_included_and_queued_in_batches(
    pool: PgPool,
) -> sqlx::Result<()> {
    let places = common::seed_places(&pool, Path::new("/arch"), Path::new("/in")).await;
    // Chicago is six hours behind UTC in January.
    for (at, duration) in [
        ("2031-01-15T05:59:00Z", 2000), // the 14th, local
        ("2031-01-15T06:00:00Z", 2000), // the 15th
        ("2031-01-15T18:00:00Z", 2000),
        ("2031-01-16T05:59:00Z", 2000), // still the 15th
        ("2031-01-16T06:00:00Z", 2000), // the 16th
        ("2031-01-15T19:00:00Z", 900),  // too short
    ] {
        common::insert_call(
            &pool,
            places,
            &format!("{at}.mp3"),
            utc(at),
            duration,
            false,
        )
        .await;
    }
    let mut conn = pool.acquire().await?;
    let plan = backfill::plan(&mut conn, &ask("2031-01-15", "2031-01-15"), ZONE, 1000)
        .await
        .unwrap();
    assert_eq!(plan.count, 3);
    assert!(!plan.talkgroup_on);
    let mut seen = Vec::new();
    let queued = backfill::run(&mut conn, &plan.past, 2, |n| seen.push(n)).await?;
    assert_eq!((queued, seen), (3, vec![2, 3]));
    let again = backfill::plan(&mut conn, &ask("2031-01-15", "2031-01-15"), ZONE, 1000)
        .await
        .unwrap();
    assert_eq!(again.count, 0);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn an_unknown_talkgroup_and_days_out_of_order_are_refused(pool: PgPool) -> sqlx::Result<()> {
    let mut conn = pool.acquire().await?;
    let unknown = backfill::plan(&mut conn, &ask("2031-01-15", "2031-01-15"), ZONE, 1000).await;
    assert!(unknown.unwrap_err().contains("no talkgroup SYSA/127"));
    let backwards = backfill::plan(&mut conn, &ask("2031-01-16", "2031-01-15"), ZONE, 1000).await;
    assert!(backwards.unwrap_err().contains("before"));
    Ok(())
}
