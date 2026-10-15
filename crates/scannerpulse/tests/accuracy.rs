mod common;

use std::time::Duration;

use chrono::{TimeZone, Utc};

use common::{Answer, FakeAsr, labeled_set};
use scannerpulse::accuracy::{self, Candidate, Run, STOP_AFTER_FAULTS};
use scannerpulse::asr::Client;
use sqlx::PgPool;

fn candidate(url: &str) -> Candidate<'_> {
    Candidate {
        server: url,
        model: "model-y",
        language: None,
        prompt: Some("Engine, Medic"),
        timeout: Duration::from_secs(10),
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn production_is_scored_from_the_newest_label_and_the_first_output(pool: PgPool) {
    let (_root, ids) = labeled_set(&pool, &[("engine one", "engine one"), ("", "thank you")]).await;
    // A later save of the first call: its text wins, the first output stays.
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, machine_text, source) \
         SELECT id, started_at, 'engine one respond', 'engine one', 'user_edit' \
         FROM calls WHERE id = $1",
    )
    .bind(ids[0])
    .execute(&pool)
    .await
    .unwrap();
    // A call of the set with no label is not scored.
    let labeled = accuracy::labeled(&pool, "s1").await.unwrap();
    assert_eq!(labeled.len(), 2);
    assert_eq!(labeled[0].said, "engine one respond");
    let score = accuracy::production(&labeled);
    assert_eq!((score.errors, score.words, score.calls), (3, 3, 2));
    assert_eq!((score.silent, score.phantom), (1, 1));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_candidate_is_scored_on_the_same_calls_and_nothing_is_saved(pool: PgPool) {
    let (_root, _) = labeled_set(
        &pool,
        &[
            ("engine one respond", "engine 1"),
            ("medic two", "medic two"),
        ],
    )
    .await;
    let asr = FakeAsr::start(vec![Answer::Text("Engine one, respond.")]);
    let labeled = accuracy::labeled(&pool, "s1").await.unwrap();
    let mut seen = Vec::new();
    let tried = accuracy::try_candidate(&Client::new(None), &labeled, &candidate(&asr.url), |n| {
        seen.push(n)
    })
    .await;
    assert_eq!(seen, vec![1, 2]);
    assert_eq!(tried.stopped, None);
    // The candidate gets the first call right and the second wholly wrong.
    assert_eq!((tried.candidate.errors, tried.candidate.words), (3, 5));
    assert_eq!((tried.production.errors, tried.production.words), (2, 5));
    assert!(tried.seconds_per_call().is_some());
    let sent = asr.requests();
    assert!(sent[0].contains("model-y") && sent[0].contains("Engine, Medic"));
    let transcripts: i64 = sqlx::query_scalar("SELECT count(*) FROM transcripts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(transcripts, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_call_without_its_file_is_skipped_and_the_rest_are_scored(pool: PgPool) {
    let (_root, ids) = labeled_set(&pool, &[("no file", "x"), ("medic two", "medic two")]).await;
    let asr = FakeAsr::start(vec![Answer::Text("medic two")]);
    let labeled = accuracy::labeled(&pool, "s1").await.unwrap();
    let tried =
        accuracy::try_candidate(&Client::new(None), &labeled, &candidate(&asr.url), |_| {}).await;
    assert_eq!(tried.skipped.len(), 1);
    assert_eq!(tried.skipped[0].0, ids[0]);
    assert!(
        tried.skipped[0].1.contains("call-0.mp3"),
        "{:?}",
        tried.skipped
    );
    assert_eq!((tried.candidate.calls, tried.candidate.errors), (1, 0));
    assert_eq!(tried.production.calls, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_server_that_keeps_failing_stops_the_run(pool: PgPool) {
    let labels: Vec<(&str, &str)> = (0..5).map(|_| ("engine one", "engine one")).collect();
    let (_root, _) = labeled_set(&pool, &labels).await;
    let asr = FakeAsr::start(vec![Answer::Status(500, "{}")]);
    let labeled = accuracy::labeled(&pool, "s1").await.unwrap();
    let tried =
        accuracy::try_candidate(&Client::new(None), &labeled, &candidate(&asr.url), |_| {}).await;
    assert!(tried.stopped.is_some());
    assert_eq!(tried.skipped.len(), STOP_AFTER_FAULTS);
    assert_eq!(tried.candidate.calls, 0);
    assert_eq!(tried.seconds_per_call(), None);
    assert_eq!(asr.requests().len(), STOP_AFTER_FAULTS);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_server_that_refuses_every_call_stops_the_run(pool: PgPool) {
    let labels: Vec<(&str, &str)> = (0..5).map(|_| ("engine one", "engine one")).collect();
    let (_root, _) = labeled_set(&pool, &labels).await;
    let asr = FakeAsr::start(vec![Answer::Status(422, "{}")]);
    let labeled = accuracy::labeled(&pool, "s1").await.unwrap();
    let tried =
        accuracy::try_candidate(&Client::new(None), &labeled, &candidate(&asr.url), |_| {}).await;
    assert!(tried.stopped.is_some());
    assert_eq!(asr.requests().len(), STOP_AFTER_FAULTS);
}

#[sqlx::test(migrations = "./migrations")]
async fn runs_are_kept_for_the_set(pool: PgPool) {
    let run = Run {
        run_at: Utc.with_ymd_and_hms(2031, 3, 2, 0, 0, 0).unwrap(),
        server: "http://asr-1:8000/v1/audio/transcriptions".into(),
        model: "model-y".into(),
        prompt: None,
        calls: 40,
        wer: 0.125,
        seconds_per_call: 1.5,
    };
    accuracy::save_run(&pool, "s1", &run).await.unwrap();
    assert_eq!(accuracy::runs(&pool, "s1").await.unwrap(), vec![run]);
    assert!(accuracy::runs(&pool, "s2").await.unwrap().is_empty());
}
