mod common;

use std::fs;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::{DateTime, Utc};
use common::{Answer, FakeAsr, HeldFence};
use scannerpulse::asr::Client;
use scannerpulse::queue::Written;
use scannerpulse::settings::{Server, Transcription};
use scannerpulse::transcriber::{Outcome, Transcriber};
use sqlx::PgPool;
use tempfile::TempDir;

const HOLDER: &str = "host-a 100-1900000000";

fn utc(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn settings(url: &str, weight: u32) -> Transcription {
    Transcription {
        servers: vec![Server {
            url: url.to_string(),
            weight,
        }],
        model: "model-x".to_string(),
        language: None,
        min_duration_ms: 1000,
        request_timeout: Duration::from_secs(5),
        prompt: None,
    }
}

/// An archive with `n` queued calls whose files exist, a minute apart, and the
/// lease held; the transcriber against `url` with `weight` places.
async fn site(
    pool: &PgPool,
    n: usize,
    url: &str,
    weight: u32,
) -> (
    TempDir,
    Arc<HeldFence>,
    Transcriber,
    Vec<(i64, DateTime<Utc>)>,
) {
    let root = tempfile::tempdir().unwrap();
    let places = common::seed_places(pool, root.path(), root.path()).await;
    common::hold_lease(pool, HOLDER).await;
    let mut calls = Vec::new();
    for i in 0..n {
        let rel = format!("2031-01-15/127-Alpha Fire/call-{i}.mp3");
        fs::create_dir_all(root.path().join("2031-01-15/127-Alpha Fire")).unwrap();
        fs::write(root.path().join(&rel), format!("audio of call {i}")).unwrap();
        let at = utc(&format!("2031-01-15T18:0{i}:00Z"));
        let (id, _) = common::insert_call(pool, places, &rel, at, 2000, true).await;
        sqlx::query(
            "INSERT INTO transcription_queue (call_id, started_at, queued_at, next_try_at, state) \
             VALUES ($1, $2, now(), now(), 'queued')",
        )
        .bind(id)
        .bind(at)
        .execute(pool)
        .await
        .unwrap();
        calls.push((id, at));
    }
    let fence = HeldFence::new(HOLDER);
    let mut transcriber = Transcriber::new(pool.clone(), fence.clone(), Client::new(None));
    transcriber.set(Some(settings(url, weight)));
    (root, fence, transcriber, calls)
}

async fn one(t: &mut Transcriber) -> Outcome {
    let done = tokio::time::timeout(Duration::from_secs(10), t.next_done())
        .await
        .expect("a request did not end within 10 s")
        .expect("no request in flight");
    t.settle(done).await.unwrap()
}

async fn queued(pool: &PgPool) -> Vec<(i64, i16, String)> {
    sqlx::query_as("SELECT call_id, attempts, state FROM transcription_queue ORDER BY call_id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_queued_call_is_sent_and_its_text_written(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Text("Engine 5 responding.")]);
    let (_root, _fence, mut t, calls) = site(&pool, 1, &server.url, 1).await;
    assert_eq!(t.fill().await?, 1);
    assert_eq!(
        one(&mut t).await,
        Outcome::Written(calls[0].0, Written::Text)
    );
    let text: String = sqlx::query_scalar("SELECT text FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(text, "Engine 5 responding.");
    assert!(queued(&pool).await.is_empty());
    assert!(server.requests()[0].contains("audio of call 0"));
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_server_takes_as_many_calls_as_its_weight_newest_first(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Text("Copy.")]);
    let (_root, _fence, mut t, _calls) = site(&pool, 3, &server.url, 2).await;
    assert_eq!(t.fill().await?, 2);
    assert_eq!(t.busy(), 2);
    assert_eq!(t.fill().await?, 0, "no free place");
    one(&mut t).await;
    one(&mut t).await;
    let sent = server.requests();
    assert!(
        sent.iter().any(|r| r.contains("call-2.mp3"))
            && sent.iter().any(|r| r.contains("call-1.mp3"))
    );
    assert_eq!(t.fill().await?, 1);
    one(&mut t).await;
    assert!(queued(&pool).await.is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_failing_server_cools_down_and_the_call_stays_queued(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Status(503, "busy")]);
    let (_root, _fence, mut t, calls) = site(&pool, 1, &server.url, 1).await;
    t.fill().await?;
    match one(&mut t).await {
        Outcome::ServerDown { call, .. } => assert_eq!(call, calls[0].0),
        other => panic!("{other:?}"),
    }
    assert_eq!(queued(&pool).await, [(calls[0].0, 0, "queued".to_string())]);
    assert_eq!(t.fill().await?, 0, "the server cools down");
    let states = t.take_server_change().unwrap();
    assert_eq!(states[0].failed_rounds, 1);
    assert!(states[0].cooling_for.is_some());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_refused_call_counts_against_the_call_only(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Status(400, "unreadable audio")]);
    let (_root, _fence, mut t, calls) = site(&pool, 1, &server.url, 1).await;
    t.fill().await?;
    match one(&mut t).await {
        Outcome::Retry { call, error } => {
            assert_eq!(call, calls[0].0);
            assert!(error.contains("unreadable audio"), "{error}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(queued(&pool).await, [(calls[0].0, 1, "queued".to_string())]);
    assert!(t.server_states()[0].cooling_for.is_none());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn a_missing_file_counts_against_the_call(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Text("never sent")]);
    let (root, _fence, mut t, calls) = site(&pool, 1, &server.url, 1).await;
    fs::remove_file(root.path().join("2031-01-15/127-Alpha Fire/call-0.mp3")).unwrap();
    t.fill().await?;
    match one(&mut t).await {
        Outcome::Retry { error, .. } => assert!(error.contains("could not be read"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(queued(&pool).await, [(calls[0].0, 1, "queued".to_string())]);
    assert!(server.requests().is_empty());
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn after_a_lost_lease_nothing_is_sent_or_written(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Text("Engine 5")]);
    let (_root, fence, mut t, calls) = site(&pool, 2, &server.url, 1).await;
    t.fill().await?;
    fence.held.store(false, Ordering::SeqCst);
    assert_eq!(one(&mut t).await, Outcome::LeaseLost(calls[1].0));
    assert_eq!(t.fill().await?, 0);
    let texts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await?;
    assert_eq!(texts, 0);
    assert_eq!(queued(&pool).await.len(), 2);
    Ok(())
}

#[sqlx::test(migrations = "./migrations")]
async fn while_transcription_is_off_nothing_is_sent(pool: PgPool) -> sqlx::Result<()> {
    let server = FakeAsr::start(vec![Answer::Text("Engine 5")]);
    let (_root, _fence, mut t, _calls) = site(&pool, 1, &server.url, 1).await;
    t.set(None);
    assert_eq!(t.fill().await?, 0);
    assert!(server.requests().is_empty());
    Ok(())
}
