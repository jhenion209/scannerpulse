use scannerpulse::jobs::{self, JobError, Kind};
use scannerpulse::steps::Step;
use serde_json::json;
use sqlx::PgPool;

async fn create(pool: &PgPool, kind: Kind, by: &str) -> Result<i64, JobError> {
    let mut conn = pool.acquire().await.unwrap();
    jobs::create(&mut *conn, kind, &json!({}), None, by).await
}

async fn set_state(pool: &PgPool, id: i64, state: &str) {
    sqlx::query("UPDATE jobs SET state = $2 WHERE id = $1")
        .bind(id)
        .bind(state)
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_job_that_changes_archive_files_is_refused_while_a_step_is_carried_out(pool: PgPool) {
    let options = pool.connect_options();
    let step = Step::begin(&options, "the archive's switch", "ann")
        .await
        .unwrap();
    for kind in [
        Kind::Move,
        Kind::PurgeBefore,
        Kind::Import,
        Kind::Consolidate,
        Kind::Correct,
        Kind::Merge,
        Kind::Purge,
    ] {
        match create(&pool, kind, "cy").await {
            Err(JobError::Excluded(why)) => {
                assert!(
                    why.starts_with("the archive's switch (started by ann on "),
                    "{why}"
                );
                assert!(
                    why.ends_with("is being carried out: start this once it has ended"),
                    "{why}"
                );
            }
            other => panic!("{kind:?}: {other:?}"),
        }
    }
    // Jobs that change no archive file go on.
    create(&pool, Kind::Queue, "cy").await.unwrap();
    create(&pool, Kind::ImportReview, "cy").await.unwrap();
    step.end().await;
    create(&pool, Kind::Move, "cy").await.unwrap();
}

#[sqlx::test(migrations = "./migrations")]
async fn a_step_is_refused_while_a_job_that_changes_archive_files_waits_or_runs(pool: PgPool) {
    let options = pool.connect_options();
    let id = create(&pool, Kind::Move, "cy").await.unwrap();
    let waiting = Step::begin(&options, "the archive's switch", "ann")
        .await
        .err();
    assert_eq!(
        waiting.as_deref(),
        Some(&*format!(
            "job {id} (move), started by cy, is waiting: wait for it to end, or stop it, then try again"
        ))
    );
    set_state(&pool, id, "running").await;
    let running = Step::begin(&options, "the database's move", "ann")
        .await
        .err();
    assert!(running.unwrap().contains("is running"));
    set_state(&pool, id, "done").await;
    Step::begin(&options, "the database's move", "ann")
        .await
        .unwrap()
        .end()
        .await;
    // A queued job that changes no archive file does not hold a step back.
    create(&pool, Kind::Queue, "cy").await.unwrap();
    let step = Step::begin(&options, "the database's move", "ann")
        .await
        .unwrap();
    // Nor does one step hold back nothing but another.
    let other = Step::begin(&options, "the second machine's addition", "bob")
        .await
        .err();
    assert!(
        other
            .unwrap()
            .starts_with("the database's move (started by ann on "),
        "the second step names the first"
    );
    step.end().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_step_that_died_frees_the_jobs_and_its_row_is_forgotten(pool: PgPool) {
    let options = pool.connect_options();
    let step = Step::begin(&options, "the archive's switch", "ann")
        .await
        .unwrap();
    drop(step);
    let mut created = create(&pool, Kind::Move, "cy").await;
    for _ in 0..50 {
        if created.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        created = create(&pool, Kind::Move, "cy").await;
    }
    let id = created.expect("the server frees a dead session's lock");
    set_state(&pool, id, "done").await;
    let step = Step::begin(&options, "the database's move", "bob")
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM conversion_steps")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "the dead step's row is gone");
    step.end().await;
}

#[sqlx::test(migrations = "./migrations")]
async fn a_step_and_a_job_started_at_once_are_never_both_let_through(pool: PgPool) {
    let options = pool.connect_options();
    for _ in 0..10 {
        let (a, b) = (pool.clone(), options.clone());
        let job = tokio::spawn(async move { create(&a, Kind::Move, "cy").await });
        let step =
            tokio::spawn(async move { Step::begin(&b, "the archive's switch", "ann").await });
        let (job, step) = (job.await.unwrap(), step.await.unwrap());
        assert!(
            !(job.is_ok() && step.is_ok()),
            "both a move and a step were let through"
        );
        assert!(
            job.is_ok() || step.is_ok(),
            "{job:?} {:?}",
            step.as_ref().err()
        );
        if let Ok(step) = step {
            step.end().await;
        }
        sqlx::query("UPDATE jobs SET state = 'done' WHERE state = 'queued'")
            .execute(&pool)
            .await
            .unwrap();
    }
}
