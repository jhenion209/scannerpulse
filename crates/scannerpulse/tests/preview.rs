mod common;

use std::fs;
use std::path::Path;

use id3::TagLike;
use scannerpulse::preview::{self, Verdict};
use scannerpulse::recorders::Refused;
use sqlx::PgPool;

const ZONE: chrono_tz::Tz = chrono_tz::America::Chicago;

/// A file of `system`, talkgroup `number` named `name`, from radio 4021
/// "Medic 7", at 12:00:`second` on 2031-01-15 (invented values).
fn file(folder: &Path, second: u32, system: &str, number: i64, name: &str) {
    let mut tag = id3::Tag::new();
    tag.set_title(format!("{number}\"{name}\""));
    tag.set_artist("4021-Medic 7");
    tag.set_text("TIT1", system);
    tag.add_frame(id3::frame::Comment {
        lang: "eng".to_string(),
        description: String::new(),
        text: format!("System:{system};Site:North;"),
    });
    fs::write(
        folder.join(format!(
            "20310115_1200{second:02}{system}_North__TO_{number}_FROM_4021.mp3"
        )),
        common::tagged(&tag, 25),
    )
    .unwrap();
}

/// How many rows each table that filing writes holds.
async fn written(pool: &PgPool) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "systems",
        "talkgroups",
        "sites",
        "calls",
        "ingest_files",
        "name_proposals",
    ] {
        counts.push(
            sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    counts
}

#[sqlx::test(migrations = "./migrations")]
async fn a_preview_says_what_filing_would_do_and_changes_nothing(pool: PgPool) {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    common::talkgroup(&pool, "SYSA", 7).await;
    let eight = common::talkgroup(&pool, "SYSA", 8).await;
    let nine = common::talkgroup(&pool, "SYSA", 9).await;
    sqlx::query("UPDATE talkgroups SET name = 'Unidentified 8' WHERE id = $1")
        .bind(eight)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO name_proposals (talkgroup_id, name, dismissed) VALUES ($1, 'Niner', true)",
    )
    .bind(nine)
    .execute(&pool)
    .await
    .unwrap();
    let folder = source.path();
    file(folder, 0, "SYSA", 7, "Group 7");
    file(folder, 1, "SYSA", 7, "Group Seven");
    file(folder, 2, "SYSA", 8, "Eight");
    file(folder, 3, "SYSA", 9, "Niner");
    file(folder, 4, "SYSA", 10, "Ten");
    file(folder, 5, "SYSB", 215, "Bravo EMS");
    fs::write(folder.join("20310115_120006.mp3"), b"not audio").unwrap();
    let before = written(&pool).await;

    let seen = preview::preview(&pool, "live", ZONE).await.unwrap();

    assert_eq!(seen.files, 7);
    let verdicts: Vec<&Verdict> = seen.sample.iter().map(|file| &file.verdict).collect();
    assert_eq!(
        verdicts,
        [
            &Verdict::Filed,
            &Verdict::Proposal {
                current: "Group 7".into()
            },
            &Verdict::Named {
                current: "Unidentified 8".into()
            },
            &Verdict::Filed,
            &Verdict::NewTalkgroup,
            &Verdict::Staged,
            &Verdict::Rejected {
                reason: "the file holds no MPEG audio frame".into()
            },
        ]
    );
    let counts = &seen.counts;
    assert_eq!(
        (
            counts.filed,
            counts.proposal,
            counts.named,
            counts.new_talkgroup,
            counts.staged,
            counts.rejected,
            counts.unreadable
        ),
        (2, 1, 1, 1, 1, 1, 0)
    );
    let first = &seen.sample[0];
    assert_eq!(
        first.rel_path,
        "20310115_120000SYSA_North__TO_7_FROM_4021.mp3"
    );
    assert_eq!(
        first.started_at.unwrap().to_rfc3339(),
        "2031-01-15T18:00:00+00:00"
    );
    assert_eq!(
        (
            first.system.as_deref(),
            first.site.as_deref(),
            first.talkgroup,
            first.talkgroup_name.as_deref(),
            first.radio_id,
            first.radio_alias.as_deref()
        ),
        (
            Some("SYSA"),
            Some("North"),
            Some(7),
            Some("Group 7"),
            Some(4021),
            Some("Medic 7")
        )
    );
    assert!(first.duration_ms.unwrap() > 0);
    assert_eq!(written(&pool).await, before, "the preview writes nothing");
    assert_eq!(
        fs::read_dir(folder).unwrap().count(),
        7,
        "and deletes nothing"
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn a_large_folders_sample_is_spread_over_it(pool: PgPool) {
    let (root, source) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    common::seed_places(&pool, root.path(), source.path()).await;
    let total = preview::SAMPLE_FILES * 2 + 1;
    for minute in 0..total {
        let folder = source.path().join(format!("{:02}", minute / 60));
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join(format!(
                "20310115_{:02}{:02}00SYSA__TO_7_FROM_4021.mp3",
                minute / 60,
                minute % 60
            )),
            common::frames(2),
        )
        .unwrap();
    }
    let seen = preview::preview(&pool, "live", ZONE).await.unwrap();
    assert_eq!(seen.files, total);
    assert_eq!(seen.sample.len(), preview::SAMPLE_FILES);
    let paths: Vec<&str> = seen
        .sample
        .iter()
        .map(|file| file.rel_path.as_str())
        .collect();
    assert_eq!(paths[0], "00/20310115_000000SYSA__TO_7_FROM_4021.mp3");
    assert!(
        paths
            .last()
            .unwrap()
            .starts_with(&format!("{:02}/", (total - 1) / 60)),
        "the last part of the folder is sampled too: {paths:?}"
    );
    let mut sorted = paths.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, paths, "in order, each file once");
}

#[sqlx::test(migrations = "./migrations")]
async fn only_a_recorder_is_previewed(pool: PgPool) {
    assert!(matches!(
        preview::preview(&pool, "nowhere", ZONE).await,
        Err(Refused::NoSuch(_))
    ));
}
