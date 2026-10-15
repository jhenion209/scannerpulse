mod common;

use chrono::{Duration, TimeZone, Utc};
use scannerpulse::jobs::{self, JobError, Kind, Stopped};
use scannerpulse::queue::Past;
use serde_json::json;
use sqlx::PgPool;

async fn state(pool: &PgPool, id: i64) -> (String, bool, String) {
    sqlx::query_as("SELECT state, stop_requested, message FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_new_job_waits_and_the_newest_are_listed_first(pool: PgPool) {
    let first = jobs::create(&pool, Kind::Queue, &json!({}), Some(10), "ann")
        .await
        .unwrap();
    let second = jobs::create(&pool, Kind::AsrRun, &json!({"set": "s1"}), None, "bob")
        .await
        .unwrap();
    let listed = jobs::list(&pool, 10).await.unwrap();
    assert_eq!(
        listed.iter().map(|j| j.id).collect::<Vec<_>>(),
        [second, first]
    );
    let job = jobs::get(&pool, first).await.unwrap().unwrap();
    assert_eq!(
        (job.kind.as_str(), job.state.as_str(), job.total, job.done),
        ("queue", "queued", Some(10), 0)
    );
    assert_eq!(job.started_by, "ann");
    assert_eq!(job.work, json!({}));
    assert!(job.host.is_none() && job.updated_at.is_none());
    assert_eq!(Kind::parse("asr_run"), Some(Kind::AsrRun));
    assert_eq!(Kind::parse("purge"), None);
}

#[sqlx::test(migrations = "./migrations")]
async fn stopping_a_waiting_job_ends_it_and_a_running_one_after_its_step(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    let waiting = jobs::create(&pool, Kind::Queue, &json!({}), None, "ann")
        .await
        .unwrap();
    assert_eq!(
        jobs::stop(&mut conn, waiting, "bob").await.unwrap(),
        Stopped::Now
    );
    let (st, asked, message) = state(&pool, waiting).await;
    assert_eq!((st.as_str(), asked), ("stopped", true));
    assert!(message.contains("bob"), "{message}");
    let running = jobs::create(&pool, Kind::Queue, &json!({}), None, "ann")
        .await
        .unwrap();
    sqlx::query("UPDATE jobs SET state = 'running' WHERE id = $1")
        .bind(running)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        jobs::stop(&mut conn, running, "bob").await.unwrap(),
        Stopped::AfterStep
    );
    assert_eq!(state(&pool, running).await.0, "running");
    assert!(state(&pool, running).await.1);
    assert!(matches!(
        jobs::stop(&mut conn, waiting, "bob").await,
        Err(JobError::Ended(_))
    ));
    assert!(matches!(
        jobs::stop(&mut conn, 999_999, "bob").await,
        Err(JobError::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stop_that_meets_a_step_in_progress_is_told_to_ask_again(pool: PgPool) {
    let id = jobs::create(&pool, Kind::Queue, &json!({}), None, "ann")
        .await
        .unwrap();
    let mut step = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM jobs WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_one(&mut *step)
        .await
        .unwrap();
    let mut conn = pool.acquire().await.unwrap();
    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        jobs::stop(&mut conn, id, "bob"),
    )
    .await
    .expect("stop must not wait without a limit");
    assert!(matches!(asked, Err(JobError::Busy(busy)) if busy == id));
    step.rollback().await.unwrap();
    assert_eq!(
        jobs::stop(&mut conn, id, "bob").await.unwrap(),
        Stopped::Now
    );
}

/// Five untried calls of a talkgroup (transcription on or off), a minute apart.
async fn calls(pool: &PgPool, number: i64, on: bool) -> i32 {
    let talkgroup = common::talkgroup(pool, "SYSA", number).await;
    sqlx::query("UPDATE talkgroups SET transcribe = $2 WHERE id = $1")
        .bind(talkgroup)
        .bind(on)
        .execute(pool)
        .await
        .unwrap();
    let at = Utc.with_ymd_and_hms(2031, 1, 15, 12, 0, 0).unwrap();
    for n in 0..5 {
        common::call(pool, talkgroup, at + Duration::minutes(n), None).await;
    }
    talkgroup
}

async fn queued(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM transcription_queue")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn context(batch: i64) -> jobs::Context {
    jobs::Context {
        batch,
        ..jobs::Context::new(None)
    }
}

fn past(talkgroup: Option<i32>) -> Past {
    Past {
        talkgroup_id: talkgroup,
        from: None,
        until: None,
        min_ms: 1000,
    }
}

#[sqlx::test(migrations = "./migrations")]
async fn a_queue_job_queues_in_steps_until_it_is_done(pool: PgPool) {
    let talkgroup = calls(&pool, 7, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let (id, count) = jobs::start_queue(&mut conn, &past(Some(talkgroup)), "ann")
        .await
        .unwrap();
    assert_eq!(count, 5);
    let mut states = Vec::new();
    while let Some((stepped, state)) = jobs::step(&pool, "host-a", &context(2)).await.unwrap() {
        assert_eq!(stepped, id);
        states.push(state);
    }
    assert_eq!(states, ["running", "running", "running", "done"]);
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!((job.done, job.total), (5, Some(5)));
    assert_eq!(job.host.as_deref(), Some("host-a"));
    assert!(job.started_at.is_some() && job.finished_at.is_some());
    assert_eq!(job.message, "queued 5 calls");
    assert_eq!(queued(&pool).await, 5);
}

#[sqlx::test(migrations = "./migrations")]
async fn the_backlog_takes_only_the_talkgroups_that_are_on(pool: PgPool) {
    calls(&pool, 7, true).await;
    calls(&pool, 8, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let (_, count) = jobs::start_queue(&mut conn, &past(None), "ann")
        .await
        .unwrap();
    assert_eq!(count, 5);
    while jobs::step(&pool, "host-a", &context(1000))
        .await
        .unwrap()
        .is_some()
    {}
    assert_eq!(queued(&pool).await, 5);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stop_takes_effect_after_the_current_step(pool: PgPool) {
    let talkgroup = calls(&pool, 7, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let (id, _) = jobs::start_queue(&mut conn, &past(Some(talkgroup)), "ann")
        .await
        .unwrap();
    jobs::step(&pool, "host-a", &context(2)).await.unwrap();
    assert_eq!(
        jobs::stop(&mut conn, id, "bob").await.unwrap(),
        Stopped::AfterStep
    );
    assert_eq!(
        jobs::step(&pool, "host-a", &context(2)).await.unwrap(),
        Some((id, "stopped".into()))
    );
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(job.done, 2);
    assert!(job.message.contains("2 of 5"), "{}", job.message);
    assert_eq!(queued(&pool).await, 2);
    assert_eq!(
        jobs::step(&pool, "host-a", &context(2)).await.unwrap(),
        None
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_another_host_is_stepping_is_left_alone(pool: PgPool) {
    use sqlx::Connection;
    let talkgroup = calls(&pool, 7, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let (id, _) = jobs::start_queue(&mut conn, &past(Some(talkgroup)), "ann")
        .await
        .unwrap();
    // Another host holds the job's row, as it does during its step.
    let mut other = pool.acquire().await.unwrap();
    let mut tx = other.begin().await.unwrap();
    sqlx::query("SELECT id FROM jobs WHERE id = $1 FOR UPDATE")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
    // Nor does this host start the next job instead: one at a time.
    jobs::start_queue(&mut conn, &past(Some(talkgroup)), "ann")
        .await
        .unwrap();
    assert_eq!(
        jobs::step(&pool, "host-b", &context(2)).await.unwrap(),
        None
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        jobs::step(&pool, "host-b", &context(2)).await.unwrap(),
        Some((id, "running".into()))
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_resumes_on_another_host_where_it_was(pool: PgPool) {
    let talkgroup = calls(&pool, 7, false).await;
    let mut conn = pool.acquire().await.unwrap();
    let (id, _) = jobs::start_queue(&mut conn, &past(Some(talkgroup)), "ann")
        .await
        .unwrap();
    jobs::step(&pool, "host-a", &context(2)).await.unwrap();
    // host-a stops holding the lease; host-b takes over.
    while jobs::step(&pool, "host-b", &context(2))
        .await
        .unwrap()
        .is_some()
    {}
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        (job.state.as_str(), job.done, job.host.as_deref()),
        ("done", 5, Some("host-b"))
    );
    assert_eq!(queued(&pool).await, 5);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_of_an_unknown_kind_fails(pool: PgPool) {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO jobs (kind, started_by) VALUES ('purge', 'ann') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        jobs::step(&pool, "host-a", &context(2)).await.unwrap(),
        Some((id, "failed".into()))
    );
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert!(job.message.contains("purge"), "{}", job.message);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_queue_job_for_no_such_talkgroup_is_refused(pool: PgPool) {
    let mut conn = pool.acquire().await.unwrap();
    assert!(matches!(
        jobs::start_queue(&mut conn, &past(Some(424_242)), "ann").await,
        Err(JobError::Refused(_))
    ));
}

// A candidate's run as a job.

async fn improvement_on(pool: &PgPool) {
    sqlx::query("INSERT INTO settings (key, value) VALUES ('improvement_module.enabled', 'true')")
        .execute(pool)
        .await
        .unwrap();
}

async fn runs_kept(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM asr_runs")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn a_candidate_run_steps_one_call_at_a_time_and_keeps_its_score(pool: PgPool) {
    improvement_on(&pool).await;
    let (_root, _) = common::labeled_set(
        &pool,
        &[
            ("engine one", "engine one"),
            ("medic two", "medic"),
            ("", ""),
        ],
    )
    .await;
    let asr = common::FakeAsr::start(vec![
        common::Answer::Text("engine one"),
        common::Answer::Text("medic two"),
        common::Answer::Text(""),
    ]);
    let mut conn = pool.acquire().await.unwrap();
    let (id, total) = jobs::start_asr_run(&mut conn, "s1", &asr.url, "model-y", None, "ann")
        .await
        .unwrap();
    assert_eq!(total, 3);
    let mut states = Vec::new();
    while let Some((_, state)) = jobs::step(&pool, "host-a", &context(2)).await.unwrap() {
        states.push(state);
    }
    assert_eq!(states, ["running", "running", "running", "done"]);
    assert_eq!(asr.requests().len(), 3, "one request a step");
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert!(job.message.starts_with("kept as run"), "{}", job.message);
    let (wer, calls): (f32, i32) = sqlx::query_as("SELECT wer, calls FROM asr_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((wer, calls), (0.0, 3));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_candidate_run_is_refused_when_it_cannot_run(pool: PgPool) {
    let (_root, _) = common::labeled_set(&pool, &[("engine one", "engine one")]).await;
    let mut conn = pool.acquire().await.unwrap();
    let refused = |r: Result<(i64, i64), JobError>| matches!(r, Err(JobError::Refused(_)));
    let url = "http://127.0.0.1:9/v1/audio/transcriptions";
    assert!(
        refused(jobs::start_asr_run(&mut conn, "s1", url, "m", None, "ann").await),
        "module off"
    );
    improvement_on(&pool).await;
    assert!(refused(
        jobs::start_asr_run(&mut conn, "s1", "http://k@x/", "m", None, "ann").await
    ));
    assert!(refused(
        jobs::start_asr_run(&mut conn, "s1", url, " ", None, "ann").await
    ));
    assert!(refused(
        jobs::start_asr_run(&mut conn, "nothing", url, "m", None, "ann").await
    ));
    assert!(
        jobs::start_asr_run(&mut conn, "s1", url, "m", None, "ann")
            .await
            .is_ok()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_server_that_keeps_failing_fails_the_run_and_nothing_is_kept(pool: PgPool) {
    improvement_on(&pool).await;
    let (_root, _) =
        common::labeled_set(&pool, &[("a", "a"), ("b", "b"), ("c", "c"), ("d", "d")]).await;
    let asr = common::FakeAsr::start(vec![common::Answer::Status(503, "busy")]);
    let mut conn = pool.acquire().await.unwrap();
    let (id, _) = jobs::start_asr_run(&mut conn, "s1", &asr.url, "m", None, "ann")
        .await
        .unwrap();
    while jobs::step(&pool, "host-a", &context(2))
        .await
        .unwrap()
        .is_some()
    {}
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!((job.state.as_str(), job.done), ("failed", 2));
    assert!(job.message.contains("nothing was kept"), "{}", job.message);
    assert_eq!(runs_kept(&pool).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_stopped_run_keeps_nothing(pool: PgPool) {
    improvement_on(&pool).await;
    let (_root, _) = common::labeled_set(&pool, &[("a", "a"), ("b", "b")]).await;
    let asr = common::FakeAsr::start(vec![common::Answer::Text("a")]);
    let mut conn = pool.acquire().await.unwrap();
    let (id, _) = jobs::start_asr_run(&mut conn, "s1", &asr.url, "m", None, "ann")
        .await
        .unwrap();
    jobs::step(&pool, "host-a", &context(2)).await.unwrap();
    jobs::stop(&mut conn, id, "ann").await.unwrap();
    assert_eq!(
        jobs::step(&pool, "host-a", &context(2)).await.unwrap(),
        Some((id, "stopped".into()))
    );
    assert_eq!(runs_kept(&pool).await, 0);
}

#[sqlx::test(migrations = "./migrations")]
async fn a_candidate_run_keeps_the_calls_it_started_with(pool: PgPool) {
    improvement_on(&pool).await;
    let (_root, ids) = common::labeled_set(
        &pool,
        &[
            ("engine one", "engine one"),
            ("medic two", "medic"),
            ("a", "a"),
            ("b", "b"),
        ],
    )
    .await;
    // The last call has no label yet, so the run does not start with it.
    sqlx::query("DELETE FROM transcript_labels WHERE call_id = $1")
        .bind(ids[3])
        .execute(&pool)
        .await
        .unwrap();
    let asr = common::FakeAsr::start(vec![
        common::Answer::Text("engine one"),
        common::Answer::Text("medic two"),
    ]);
    let mut conn = pool.acquire().await.unwrap();
    let (id, total) = jobs::start_asr_run(&mut conn, "s1", &asr.url, "m", None, "ann")
        .await
        .unwrap();
    assert_eq!(total, 3);
    jobs::step(&pool, "host-a", &context(2)).await.unwrap();
    // While the run goes on: take away the label of a call it has not reached,
    // and label the call that was not in it.
    sqlx::query("DELETE FROM transcript_labels WHERE call_id = $1")
        .bind(ids[2])
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO transcript_labels (call_id, started_at, text, machine_text, source) \
         SELECT call_id, started_at, 'late', 'late', 'user_edit' FROM eval_calls WHERE call_id = $1",
    )
    .bind(ids[3])
    .execute(&pool)
    .await
    .unwrap();
    while jobs::step(&pool, "host-a", &context(2))
        .await
        .unwrap()
        .is_some()
    {}
    let job = jobs::get(&pool, id).await.unwrap().unwrap();
    assert_eq!((job.state.as_str(), job.total), ("done", Some(3)));
    assert_eq!(asr.requests().len(), 2, "the removed call is not sent");
    let (wer, calls): (f32, i32) = sqlx::query_as("SELECT wer, calls FROM asr_runs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((wer, calls), (0.0, 2));
}
