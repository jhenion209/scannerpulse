use scannerpulse::jobs::{self, JobError, Kind, Stopped};
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
