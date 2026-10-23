use chrono::{Duration, Utc};
use scannerpulse::audit;
use sqlx::PgPool;

#[sqlx::test(migrations = "./migrations")]
async fn the_audit_log_is_never_changed_or_deleted(pool: PgPool) {
    audit::keep(
        &pool,
        "ann",
        Some("198.51.100.7"),
        "set web.system_order from nothing to [\"B\"]",
    )
    .await
    .unwrap();
    for statement in [
        "UPDATE audit_log SET what = 'nothing happened'",
        "DELETE FROM audit_log",
        "TRUNCATE audit_log",
    ] {
        let error = sqlx::query(statement).execute(&pool).await.unwrap_err();
        assert!(
            error.to_string().contains("never changed or deleted"),
            "{statement}: {error}"
        );
    }
    let kept = audit::list(&pool, 0, None, None, 10).await.unwrap();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].what, "set web.system_order from nothing to [\"B\"]");
}

#[sqlx::test(migrations = "./migrations")]
async fn entries_are_listed_oldest_first_by_who_since_when_and_after_which(pool: PgPool) {
    for (actor, what) in [
        ("ann", "signed in with a password"),
        (
            "root on sp-a",
            "ran `scannerpulse settings set time_zone '\"UTC\"'`",
        ),
        ("annabel", "signed in with a password"),
        ("ann", "signed out"),
    ] {
        audit::keep(&pool, actor, None, what).await.unwrap();
    }
    let whats = |entries: Vec<audit::Entry>| -> Vec<String> {
        entries.into_iter().map(|e| e.what).collect()
    };
    let all = audit::list(&pool, 0, None, None, 10).await.unwrap();
    assert_eq!(all.len(), 4);
    assert!(all.windows(2).all(|w| w[0].id < w[1].id));
    assert_eq!(
        whats(audit::list(&pool, 0, None, Some("ann"), 10).await.unwrap()),
        ["signed in with a password", "signed out"]
    );
    // An account on any machine is named by the account.
    let root = audit::list(&pool, 0, None, Some("root"), 10).await.unwrap();
    assert_eq!(root.len(), 1);
    assert_eq!(root[0].actor, "root on sp-a");
    assert_eq!(
        whats(audit::list(&pool, all[1].id, None, None, 10).await.unwrap()).len(),
        2
    );
    let later = Utc::now() + Duration::minutes(1);
    assert!(
        audit::list(&pool, 0, Some(later), None, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(audit::list(&pool, 0, None, None, 2).await.unwrap().len(), 2);
}
