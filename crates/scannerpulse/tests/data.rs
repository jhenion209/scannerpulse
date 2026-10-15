mod common;

use chrono::{TimeZone, Utc};
use common::{admin, app, send, sign_in};
use scannerpulse::talkgroups::{self, Renamed};
use serde_json::json;
use sqlx::PgPool;

const PASSWORD: &str = "correct horse battery";

async fn names(pool: &PgPool, id: i32) -> (String, Option<String>, bool) {
    sqlx::query_as("SELECT name, files_name, files_correct FROM talkgroups WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_rename_marks_a_talkgroup_with_calls_as_not_corrected(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    let eight = common::talkgroup(&pool, "SYSA", 8).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();

    // No calls: nothing in the archive carries the old name.
    assert_eq!(
        talkgroups::rename(&mut conn, eight, "Eight").await.unwrap(),
        Renamed::Done {
            files_correct: true
        }
    );
    assert_eq!(
        names(&pool, eight).await,
        ("Eight".into(), Some("Group 8".into()), true)
    );

    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Alpha / Fire")
            .await
            .unwrap(),
        Renamed::Done {
            files_correct: false
        }
    );
    // A second rename keeps the name the files carry.
    talkgroups::rename(&mut conn, seven, "Alpha Fire")
        .await
        .unwrap();
    assert_eq!(
        names(&pool, seven).await,
        ("Alpha Fire".into(), Some("Group 7".into()), false)
    );
    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Alpha Fire")
            .await
            .unwrap(),
        Renamed::Same
    );
    assert_eq!(
        talkgroups::rename(&mut conn, seven, "Eight").await.unwrap(),
        Renamed::Taken(eight, "SYSA/8".into())
    );
    assert_eq!(
        talkgroups::rename(&mut conn, 999_999, "X").await.unwrap(),
        Renamed::NoSuch
    );
    // Another system may have the name.
    let other = common::talkgroup(&pool, "SYSB", 7).await;
    assert!(matches!(
        talkgroups::rename(&mut conn, other, "Eight").await.unwrap(),
        Renamed::Done { .. }
    ));

    let list = talkgroups::uncorrected(&pool).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!((list[0].id, list[0].calls), (seven, 1));
    assert_eq!(list[0].files_name.as_deref(), Some("Group 7"));

    // The files of Eight carry the name it had when it got a call.
    common::call(
        &pool,
        eight,
        Utc.with_ymd_and_hms(2031, 3, 2, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    talkgroups::rename(&mut conn, eight, "Eight 2")
        .await
        .unwrap();
    assert_eq!(
        names(&pool, eight).await,
        ("Eight 2".into(), Some("Eight".into()), false)
    );
}

#[test]
fn a_name_that_cannot_round_trip_is_refused() {
    assert!(talkgroups::name_problem("Alpha / Fire").is_none());
    for bad in [
        "",
        "  ",
        " Alpha",
        "Alpha ",
        "Al\"pha",
        "Al\npha",
        &"x".repeat(201),
    ] {
        assert!(talkgroups::name_problem(bad).is_some(), "{bad:?}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn the_recorders_new_name_marks_the_files_too(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let mut conn = pool.acquire().await.unwrap();
    let system: i32 = sqlx::query_scalar("SELECT system_id FROM talkgroups WHERE id = $1")
        .bind(seven)
        .fetch_one(&pool)
        .await
        .unwrap();
    let seen = scannerpulse::reader::Talkgroup {
        number: 7,
        name: "Group Seven".into(),
    };
    scannerpulse::store::talkgroup_id(&mut conn, system, &seen)
        .await
        .unwrap();
    assert_eq!(
        names(&pool, seven).await,
        ("Group Seven".into(), Some("Group 7".into()), false)
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn an_admin_renames_a_talkgroup(pool: PgPool) {
    let seven = common::talkgroup(&pool, "SYSA", 7).await;
    common::talkgroup(&pool, "SYSA", 8).await;
    common::call(
        &pool,
        seven,
        Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap(),
        None,
    )
    .await;
    let app = app(&pool);
    let uri = format!("/api/admin/talkgroups/{seven}/rename");
    assert_eq!(
        send(&app, "POST", &uri, &[], Some(json!({"name": "A"})))
            .await
            .status,
        401
    );
    admin(&pool, "ann", PASSWORD).await;
    let (cookie, csrf) = sign_in(&app, "ann", PASSWORD).await;
    let change = [("cookie", cookie.as_str()), ("x-csrf-token", csrf.as_str())];
    let done = send(&app, "POST", &uri, &change, Some(json!({"name": "Alpha"}))).await;
    assert_eq!(done.status, 200);
    assert_eq!(done.body["files_correct"], false);
    let taken = send(
        &app,
        "POST",
        &uri,
        &change,
        Some(json!({"name": "Group 8"})),
    )
    .await;
    assert_eq!(taken.status, 409);
    assert!(taken.body["detail"].as_str().unwrap().contains("SYSA/8"));
    let bad = send(&app, "POST", &uri, &change, Some(json!({"name": "A\"B"}))).await;
    assert_eq!(bad.status, 400);
    let missing = send(
        &app,
        "POST",
        "/api/admin/talkgroups/999999/rename",
        &change,
        Some(json!({"name": "X"})),
    )
    .await;
    assert_eq!(missing.status, 404);
    let list = send(
        &app,
        "GET",
        "/api/admin/data/uncorrected",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body[0]["name"], "Alpha");
    assert_eq!(list.body[0]["files_name"], "Group 7");
}
