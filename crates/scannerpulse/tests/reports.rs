mod common;

use chrono::{Duration, TimeZone, Utc};
use common::{admin, app, send, sign_in};
use scannerpulse::reports::{self, SLOTS};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn the_traffic_report_averages_each_five_minutes_by_kind_of_day(pool: PgPool) {
    let talkgroup = common::talkgroup(&pool, "SYSA", 7).await;
    // "Today" is Monday 2031-03-10; the report covers the 120 days before it.
    let now = Utc.with_ymd_and_hms(2031, 3, 10, 0, 30, 0).unwrap();
    let wednesday = Utc.with_ymd_and_hms(2031, 3, 5, 12, 0, 0).unwrap();
    for n in 0..3 {
        common::call(
            &pool,
            talkgroup,
            wednesday + Duration::seconds(n * 50),
            None,
        )
        .await;
    }
    common::call(
        &pool,
        talkgroup,
        Utc.with_ymd_and_hms(2031, 3, 8, 3, 2, 0).unwrap(),
        None,
    )
    .await;
    // Today, and 121 days ago: outside the report.
    common::call(&pool, talkgroup, now, None).await;
    common::call(&pool, talkgroup, now - Duration::days(121), None).await;
    let mut conn = pool.acquire().await.unwrap();
    let report = reports::traffic(&mut conn, chrono_tz::UTC, now)
        .await
        .unwrap();
    assert_eq!(report.calls, 4);
    assert_eq!(report.weekdays + report.weekend_days, 120);
    assert_eq!((report.weekday.len(), report.weekend.len()), (SLOTS, SLOTS));
    let per_weekday = 3.0 / report.weekdays as f64;
    assert!(
        (report.weekday[144] - per_weekday).abs() < 1e-9,
        "12:00 on weekdays"
    );
    assert!(
        (report.weekend[36] - 1.0 / report.weekend_days as f64).abs() < 1e-9,
        "03:00 on weekends"
    );
    assert_eq!(report.weekday.iter().filter(|v| **v > 0.0).count(), 1);
    assert_eq!(report.quietest_weekday.len(), 3);
    for window in &report.quietest_weekday {
        assert_eq!(window.calls_per_day, 0.0);
        assert!(
            !(window.start.as_str() <= "12:00" && window.end.as_str() > "12:00"),
            "{window:?}"
        );
    }
    assert_eq!(report.time_zone, "UTC");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_reports_tab_is_for_admins_and_is_kept_a_while(pool: PgPool) {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('time_zone', '\"UTC\"')")
        .execute(&pool)
        .await
        .unwrap();
    let talkgroup = common::talkgroup(&pool, "SYSA", 7).await;
    common::call(&pool, talkgroup, Utc::now() - Duration::days(2), None).await;
    let app = app(&pool);
    assert_eq!(
        send(&app, "GET", "/api/admin/reports/traffic", &[], None)
            .await
            .status,
        401
    );
    admin(&pool, "ann", "correct horse battery").await;
    let (cookie, _) = sign_in(&app, "ann", "correct horse battery").await;
    let first = send(
        &app,
        "GET",
        "/api/admin/reports/traffic",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body["calls"], 1);
    common::call(&pool, talkgroup, Utc::now() - Duration::days(3), None).await;
    let again = send(
        &app,
        "GET",
        "/api/admin/reports/traffic",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(again.body["calls"], 1, "the report is kept for ten minutes");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_traffic_report_places_calls_on_the_local_day(pool: PgPool) {
    let talkgroup = common::talkgroup(&pool, "SYSA", 7).await;
    let now = Utc.with_ymd_and_hms(2031, 3, 10, 12, 0, 0).unwrap();
    // 18:00 UTC on a Wednesday is 12:00 in Chicago; 04:00 UTC on a Sunday
    // is 22:00 on the Saturday before.
    common::call(
        &pool,
        talkgroup,
        Utc.with_ymd_and_hms(2031, 3, 5, 18, 0, 0).unwrap(),
        None,
    )
    .await;
    common::call(
        &pool,
        talkgroup,
        Utc.with_ymd_and_hms(2031, 3, 9, 4, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    let report = reports::traffic(&mut conn, chrono_tz::America::Chicago, now)
        .await
        .unwrap();
    assert!(report.weekday[144] > 0.0, "12:00 on a weekday");
    assert!(report.weekend[264] > 0.0, "22:00 on a weekend day");
    assert_eq!(report.calls, 2);
}
