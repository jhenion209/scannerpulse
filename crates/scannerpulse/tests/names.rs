mod common;

use scannerpulse::names;
use scannerpulse::reader::Talkgroup;
use scannerpulse::recorders::Refused;
use scannerpulse::store;
use sqlx::PgPool;

/// SYSA/7 "Group 7", with a recorder's proposal `name` from `sources`.
async fn proposed(pool: &PgPool, name: &str, sources: &[i16]) -> i32 {
    let seven = common::talkgroup(pool, "SYSA", 7).await;
    send(pool, name, sources).await;
    seven
}

/// SYSA/7 named `name` in a file of each of `sources`.
async fn send(pool: &PgPool, name: &str, sources: &[i16]) {
    let mut conn = pool.acquire().await.unwrap();
    let system = store::system_id(&mut conn, "SYSA").await.unwrap();
    for source in sources {
        let seen = Talkgroup {
            number: 7,
            name: name.into(),
        };
        store::talkgroup_id(&mut conn, system, &seen, *source)
            .await
            .unwrap();
    }
}

async fn recorder(pool: &PgPool, name: &str) -> i16 {
    sqlx::query_scalar(
        "INSERT INTO sources (name, kind, path) VALUES ($1, 'sdrtrunk', '/srv/in/' || $1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn proposals_are_listed_with_their_recorders_and_counts(pool: PgPool) {
    let (live, east) = (recorder(&pool, "live").await, recorder(&pool, "east").await);
    let seven = proposed(&pool, "Group Seven", &[east, live, east]).await;
    let mut conn = pool.acquire().await.unwrap();
    let listed = names::proposals(&mut conn).await.unwrap();
    assert_eq!(listed.len(), 1);
    let p = &listed[0];
    assert_eq!(
        (
            p.talkgroup_id,
            p.system.as_str(),
            p.number,
            p.current.as_str(),
            p.proposed.as_str(),
            p.calls
        ),
        (seven, "SYSA", 7, "Group 7", "Group Seven", 3)
    );
    assert_eq!(p.recorders, ["live", "east"]);
}

#[sqlx::test(migrations = "./migrations")]
async fn an_approved_name_renames_and_the_old_one_is_not_proposed_back(pool: PgPool) {
    let seven = proposed(&pool, "Group Seven", &[1]).await;
    let mut conn = pool.acquire().await.unwrap();
    names::approve(&mut conn, seven, "Group Seven")
        .await
        .unwrap();
    let name: String = sqlx::query_scalar("SELECT name FROM talkgroups WHERE id = $1")
        .bind(seven)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Group Seven");
    assert!(names::proposals(&mut conn).await.unwrap().is_empty());
    // A recorder not yet updated still sends the old name: dismissed already.
    send(&pool, "Group 7", &[2]).await;
    assert!(names::proposals(&mut conn).await.unwrap().is_empty());
    assert!(matches!(
        names::approve(&mut conn, seven, "Group Seven").await,
        Err(Refused::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_dismissed_spelling_is_not_listed_again(pool: PgPool) {
    let seven = proposed(&pool, "Grp 7", &[1]).await;
    let mut conn = pool.acquire().await.unwrap();
    names::dismiss(&mut conn, seven, "Grp 7").await.unwrap();
    send(&pool, "Grp 7", &[1]).await;
    assert!(names::proposals(&mut conn).await.unwrap().is_empty());
    assert!(matches!(
        names::dismiss(&mut conn, seven, "Grp 7").await,
        Err(Refused::NoSuch(_))
    ));
}

#[sqlx::test(migrations = "./migrations")]
async fn a_proposal_whose_name_another_talkgroup_has_is_refused(pool: PgPool) {
    common::talkgroup(&pool, "SYSA", 8).await;
    let seven = proposed(&pool, "Group 8", &[1]).await;
    let mut conn = pool.acquire().await.unwrap();
    match names::approve(&mut conn, seven, "Group 8").await {
        Err(Refused::Refused(why)) => assert!(why.contains("merge into it"), "{why}"),
        other => panic!("expected a refusal: {other:?}"),
    }
}
