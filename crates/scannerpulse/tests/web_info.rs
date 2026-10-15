mod common;

use axum::http::StatusCode;
use chrono::{TimeZone, Utc};
use common::{call, get, talkgroup};
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn a_call_shows_its_details_and_whether_a_person_labeled_it(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 7).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    let id = call(&pool, tg, at, Some("engine five")).await;
    let (status, detail) = get(&pool, &format!("/api/calls/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["talkgroup"], 7);
    assert_eq!(detail["talkgroup_name"], "Group 7");
    assert_eq!(detail["system"], "SYSA");
    assert_eq!(detail["transcript"], "engine five");
    assert_eq!(detail["origin"], "machine");
    assert_eq!(detail["labeled"], false);
    assert!(detail["file"].as_str().unwrap().ends_with(".mp3"));
    assert!(!detail["file"].as_str().unwrap().contains('/'));
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, source) VALUES ($1, $2, 'x', 'user_edit')",
    )
    .bind(id)
    .bind(at)
    .execute(&pool)
    .await
    .unwrap();
    let (_, again) = get(&pool, &format!("/api/calls/{id}")).await;
    assert_eq!(again["labeled"], true);
    assert_eq!(
        get(&pool, "/api/calls/999999").await.0,
        StatusCode::NOT_FOUND
    );
    let (status, answer) = get(&pool, "/api/calls/abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(answer["error"], "bad request");
}

#[sqlx::test(migrations = "./migrations")]
async fn the_filters_group_talkgroups_by_system_in_the_sites_order(pool: PgPool) {
    talkgroup(&pool, "ALPHA", 1).await;
    talkgroup(&pool, "BRAVO", 2).await;
    talkgroup(&pool, "BRAVO", 1).await;
    sqlx::query("INSERT INTO settings (key, value) VALUES ('web.system_order', '[\"BRAVO\"]')")
        .execute(&pool)
        .await
        .unwrap();
    let (status, filters) = get(&pool, "/api/filters").await;
    assert_eq!(status, StatusCode::OK);
    let groups = filters["talkgroup_groups"].as_array().unwrap();
    assert_eq!(groups[0]["system"], "BRAVO");
    assert_eq!(groups[1]["system"], "ALPHA");
    let numbers: Vec<i64> = groups[0]["talkgroups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["number"].as_i64().unwrap())
        .collect();
    assert_eq!(numbers, vec![1, 2]);
    assert_eq!(filters["systems"].as_array().unwrap().len(), 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_favorites_list_their_talkgroups(pool: PgPool) {
    let a = talkgroup(&pool, "SYSA", 1).await;
    let b = talkgroup(&pool, "SYSA", 2).await;
    sqlx::query("INSERT INTO favorites (id, name) VALUES (1, 'Dispatch'), (2, 'Empty')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO favorite_talkgroups VALUES (1, $1), (1, $2)")
        .bind(b)
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();
    let (_, favorites) = get(&pool, "/api/favorites").await;
    assert_eq!(favorites[0]["name"], "Dispatch");
    assert_eq!(favorites[0]["talkgroups"], serde_json::json!([a, b]));
    assert_eq!(favorites[1]["talkgroups"], serde_json::json!([]));
}

#[sqlx::test(migrations = "./migrations")]
async fn the_status_tells_the_banner_what_is_on(pool: PgPool) {
    let (_, off) = get(&pool, "/api/status").await;
    assert_eq!(
        off,
        serde_json::json!({"maintenance": false, "transcription": false, "queue": 0})
    );
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('maintenance.enabled', 'true'), \
         ('transcription.enabled', 'true')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (_, on) = get(&pool, "/api/status").await;
    assert_eq!(
        (on["maintenance"].as_bool(), on["transcription"].as_bool()),
        (Some(true), Some(true))
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn the_labelers_list_is_shown_only_when_it_is_public(pool: PgPool) {
    let tg = talkgroup(&pool, "SYSA", 1).await;
    let at = Utc.with_ymd_and_hms(2031, 3, 1, 12, 0, 0).unwrap();
    let id = call(&pool, tg, at, Some("x")).await;
    sqlx::query("INSERT INTO eval_calls (set_name, call_id, started_at, drawn_at) VALUES ('s1', $1, $2, now())")
        .bind(id)
        .bind(at)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        get(&pool, "/api/labels/todo?set=s1").await.0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("INSERT INTO settings (key, value) VALUES ('labeler_list.public', 'true')")
        .execute(&pool)
        .await
        .unwrap();
    let (status, todo) = get(&pool, "/api/labels/todo?set=s1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(todo["size"], 1);
    assert_eq!(todo["calls"][0]["id"], id);
    assert_eq!(
        get(&pool, "/api/labels/todo?set=nope").await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_bad_setting_is_an_internal_fault_that_names_nothing(pool: PgPool) {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', '\"yes\"')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let (status, answer) = get(&pool, "/api/labels/todo?set=s1").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        answer,
        serde_json::json!({"error": "internal", "detail": "something went wrong on the server"})
    );
}
