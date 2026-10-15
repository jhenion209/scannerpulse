mod common;

use std::time::Duration;

use axum::http::StatusCode;
use chrono::{DateTime, TimeZone, Utc};
use common::{call, get, talkgroup};
use scannerpulse::listing::{self, Filter};
use serde_json::Value;
use sqlx::PgPool;

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2031, 3, 1, 12, minute, 0).unwrap()
}

fn ids(answer: &Value) -> Vec<i64> {
    answer["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn pages_follow_each_other_without_a_gap_or_a_repeat(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let mut made = Vec::new();
    for minute in 0..25 {
        made.push(call(&pool, tg, at(minute), None).await);
    }
    // Two calls at the same instant: the id orders them.
    made.push(call(&pool, tg, at(24), None).await);
    let newest_first: Vec<i64> = {
        let mut v = made.clone();
        v.sort_by_key(|id| std::cmp::Reverse(*id));
        v
    };
    let mut seen = Vec::new();
    let mut uri = "/api/calls?size=10".to_string();
    loop {
        let (status, answer) = get(&pool, &uri).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        seen.extend(ids(&answer));
        match answer["next"].as_str() {
            Some(next) => uri = format!("/api/calls?size=10&count=false&after={next}"),
            None => break,
        }
    }
    assert_eq!(seen, newest_first);
    let mut oldest = Vec::new();
    let mut uri = "/api/calls?size=10&order=oldest".to_string();
    loop {
        let (status, answer) = get(&pool, &uri).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
        oldest.extend(ids(&answer));
        match answer["next"].as_str() {
            Some(next) => uri = format!("/api/calls?size=10&order=oldest&after={next}"),
            None => break,
        }
    }
    let oldest_first: Vec<i64> = newest_first.iter().rev().copied().collect();
    assert_eq!(oldest, oldest_first);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_filters_narrow_the_list(pool: PgPool) {
    let a1 = talkgroup(&pool, "SYSA", 1).await;
    let a2 = talkgroup(&pool, "SYSA", 2).await;
    let b1 = talkgroup(&pool, "SYSB", 1).await;
    let x = call(&pool, a1, at(1), None).await;
    let y = call(&pool, a2, at(2), None).await;
    let z = call(&pool, b1, at(3), None).await;
    let (_, by_talkgroups) = get(&pool, &format!("/api/calls?talkgroups={a1},{b1}")).await;
    assert_eq!(ids(&by_talkgroups), vec![z, x]);
    let system: i32 = sqlx::query_scalar("SELECT id FROM systems WHERE name = 'SYSA'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (_, by_system) = get(&pool, &format!("/api/calls?system={system}")).await;
    assert_eq!(ids(&by_system), vec![y, x]);
    let (_, by_time) = get(
        &pool,
        "/api/calls?from=2031-03-01T12:02:00Z&until=2031-03-01T12:03:00Z",
    )
    .await;
    assert_eq!(ids(&by_time), vec![y], "from included, until excluded");
    assert_eq!(by_time["total"], 1);
    assert_eq!(by_time["total_capped"], false);
}

#[sqlx::test(migrations = "./migrations")]
async fn every_form_of_the_search_box_runs(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let fire = call(&pool, tg, at(1), Some("Engine 5, structure fire")).await;
    let smoke = call(&pool, tg, at(2), Some("smoke showing, 10-4")).await;
    let both = call(&pool, tg, at(3), Some("fire and smoke at O'Brien's")).await;
    call(&pool, tg, at(4), None).await;
    let cases: [(&str, Vec<i64>); 9] = [
        ("engine 5", vec![fire]),
        ("\"structure fire\"", vec![fire]),
        ("\"fire structure\"", vec![]),
        ("fire OR smoke", vec![both, smoke, fire]),
        ("fire -smoke", vec![fire]),
        ("fire NOT smoke", vec![fire]),
        ("struct*", vec![fire]),
        ("10-4", vec![smoke]),
        ("o'brien*", vec![both]),
    ];
    for (q, want) in cases {
        let uri = format!("/api/calls?q={}", urlencode(q));
        let (status, answer) = get(&pool, &uri).await;
        assert_eq!(status, StatusCode::OK, "{q}: {answer}");
        assert_eq!(ids(&answer), want, "{q}");
    }
    // Text with nothing to search for lists every call.
    let (_, all) = get(&pool, "/api/calls?q=%21%21").await;
    assert_eq!(ids(&all).len(), 4);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_search_in_one_talkgroup_pages_through_its_matches_only(pool: PgPool) {
    let a = talkgroup(&pool, "SYSA", 1).await;
    let b = talkgroup(&pool, "SYSA", 2).await;
    let mut want = Vec::new();
    for minute in 0..12 {
        want.push(call(&pool, a, at(minute), Some("engine responding")).await);
        call(&pool, b, at(minute), Some("engine on scene")).await;
        call(&pool, a, at(minute), Some("ladder only")).await;
    }
    // Two matches at the same instant: the id orders them.
    want.push(call(&pool, a, at(11), Some("engine two")).await);
    want.sort_by_key(|id| std::cmp::Reverse(*id));
    for (order, want) in [
        ("newest", want.clone()),
        ("oldest", want.iter().rev().copied().collect()),
    ] {
        let base = format!("/api/calls?size=10&q=engine&talkgroups={a}&order={order}");
        let (status, first) = get(&pool, &base).await;
        assert_eq!(status, StatusCode::OK, "{first}");
        assert_eq!(first["total"], 13, "{order}");
        let next = first["next"].as_str().expect("a second page");
        let (_, second) = get(&pool, &format!("{base}&after={next}")).await;
        assert!(second["next"].is_null(), "{order}");
        let seen: Vec<i64> = ids(&first).into_iter().chain(ids(&second)).collect();
        assert_eq!(seen, want, "{order}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_search_in_one_system_finds_its_matches_only(pool: PgPool) {
    let a = talkgroup(&pool, "SYSA", 1).await;
    let b = talkgroup(&pool, "SYSB", 1).await;
    let first = call(&pool, a, at(1), Some("engine responding")).await;
    call(&pool, b, at(2), Some("engine on scene")).await;
    call(&pool, a, at(3), Some("ladder only")).await;
    let second = call(&pool, a, at(4), Some("engine two")).await;
    call(&pool, b, at(5), Some("engine clear")).await;
    let system: i32 = sqlx::query_scalar("SELECT id FROM systems WHERE name = 'SYSA'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let (status, answer) = get(&pool, &format!("/api/calls?system={system}&q=engine")).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(ids(&answer), vec![second, first]);
    assert_eq!(answer["total"], 2);
}

fn urlencode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'*' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_count_stops_at_its_cap_and_says_so(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    for minute in 0..5 {
        call(&pool, tg, at(minute), None).await;
    }
    let mut conn = pool.acquire().await.unwrap();
    let limit = Duration::from_secs(30);
    let everything = Filter::default();
    assert_eq!(
        listing::count(&mut conn, &everything, Some(3), limit)
            .await
            .unwrap(),
        (3, true)
    );
    assert_eq!(
        listing::count(&mut conn, &everything, Some(5), limit)
            .await
            .unwrap(),
        (5, false)
    );
    assert_eq!(
        listing::count(&mut conn, &everything, None, limit)
            .await
            .unwrap(),
        (5, false)
    );
    let (_, exact) = get(&pool, "/api/calls?exact=true").await;
    assert_eq!(
        (exact["total"].as_i64(), exact["total_capped"].as_bool()),
        (Some(5), Some(false))
    );
    let (_, none) = get(&pool, "/api/calls?count=false").await;
    assert!(none["total"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn smart_sort_reorders_the_page_only(pool: PgPool) {
    let a = talkgroup(&pool, "SYSA", 1).await;
    let b = talkgroup(&pool, "SYSA", 2).await;
    let c = talkgroup(&pool, "SYSA", 3).await;
    let first = call(&pool, a, at(0), None).await;
    let other = call(&pool, b, at(0) + chrono::Duration::seconds(3), None).await;
    let reply = call(&pool, a, at(0) + chrono::Duration::seconds(6), None).await;
    let newest = call(&pool, c, at(0) + chrono::Duration::seconds(9), None).await;
    let (_, plain) = get(&pool, "/api/calls").await;
    assert_eq!(ids(&plain), vec![newest, reply, other, first]);
    let (_, smart) = get(&pool, "/api/calls?smart=true").await;
    assert_eq!(ids(&smart), vec![newest, other, reply, first]);
    assert_eq!(smart["total"], plain["total"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_bad_parameter_is_refused_with_its_reason(pool: PgPool) {
    for uri in [
        "/api/calls?size=7",
        "/api/calls?order=sideways",
        "/api/calls?after=garbage",
        "/api/calls?talkgroups=1,x",
        "/api/calls?from=yesterday",
        "/api/calls?size=abc",
        "/api/calls?smart=1",
        "/api/calls?system=x",
    ] {
        let (status, answer) = get(&pool, uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        assert_eq!(answer["error"], "bad request", "{uri}");
        assert!(!answer["detail"].as_str().unwrap().is_empty(), "{uri}");
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_search_of_one_endless_word_runs(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let hit = call(&pool, tg, at(1), Some("a a a")).await;
    let uri = format!("/api/calls?q={}", "a-".repeat(30_000));
    let (status, answer) = get(&pool, &uri).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(ids(&answer), vec![hit]);
}

#[sqlx::test(migrations = "./migrations")]
async fn every_read_of_the_list_stops_at_the_time_limit(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    call(&pool, tg, at(0), None).await;
    // A lock makes the reads wait; their limit must end the wait.
    let mut locker = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE calls IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *locker)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let limit = Duration::from_millis(300);
    let everything = Filter::default();
    let code = |error: sqlx::Error| {
        error
            .as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.to_string())
    };
    let page = tokio::time::timeout(
        Duration::from_secs(10),
        listing::page(&mut conn, &everything, false, None, 10, limit),
    )
    .await
    .expect("the page stops at its limit");
    assert_eq!(code(page.unwrap_err()).as_deref(), Some("57014"));
    let count = tokio::time::timeout(
        Duration::from_secs(10),
        listing::count(&mut conn, &everything, None, limit),
    )
    .await
    .expect("the count stops at its limit");
    assert_eq!(code(count.unwrap_err()).as_deref(), Some("57014"));
}
