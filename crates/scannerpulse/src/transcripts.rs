//! A person's transcript. Saving one keeps a label: the person's text beside
//! the machine's first output, which is what the accuracy reports compare.

use chrono::{DateTime, Utc};
use sqlx::Connection;
use sqlx::postgres::PgConnection;

/// Saves `text` as a person's transcript of `call`, in one transaction:
///
/// - a row in `transcript_labels` with the text, the machine's first output
///   (from the call's oldest label, or else its machine transcript), source
///   `user_edit` and the `labeler` (`admin`, or `None` for a visitor);
/// - the transcript, now of origin `human`, which the machine never replaces;
/// - the call's transcript state (`no_speech` for an empty text);
/// - the call out of the transcription queue;
/// - a notice on `scannerpulse_transcripts`.
///
/// False when there is no such call.
pub async fn correct(
    conn: &mut PgConnection,
    call: (i64, DateTime<Utc>),
    text: &str,
    labeler: Option<&str>,
) -> sqlx::Result<bool> {
    let mut tx = conn.begin().await?;
    // Two people saving at once are taken one after the other.
    let found: Option<i64> =
        sqlx::query_scalar("SELECT id FROM calls WHERE id = $1 AND started_at = $2 FOR UPDATE")
            .bind(call.0)
            .bind(call.1)
            .fetch_optional(&mut *tx)
            .await?;
    if found.is_none() {
        return Ok(false);
    }
    let oldest: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT machine_text, machine_model FROM transcript_labels \
         WHERE call_id = $1 AND started_at = $2 ORDER BY id LIMIT 1",
    )
    .bind(call.0)
    .bind(call.1)
    .fetch_optional(&mut *tx)
    .await?;
    let (machine_text, machine_model) = match oldest {
        Some(first) => first,
        None => {
            let machine: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT text, model FROM transcripts \
                 WHERE call_id = $1 AND started_at = $2 AND origin = 'machine'",
            )
            .bind(call.0)
            .bind(call.1)
            .fetch_optional(&mut *tx)
            .await?;
            match machine {
                Some((text, model)) => (Some(text), model),
                None => (None, None),
            }
        }
    };
    let text = text.trim();
    sqlx::query(
        "INSERT INTO transcript_labels \
         (call_id, started_at, text, machine_text, machine_model, source, labeler) \
         VALUES ($1, $2, $3, $4, $5, 'user_edit', $6)",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(text)
    .bind(&machine_text)
    .bind(&machine_model)
    .bind(labeler)
    .execute(&mut *tx)
    .await?;
    // The row keeps the model that wrote the first text; `origin` says a
    // person changed it.
    sqlx::query(
        "INSERT INTO transcripts (call_id, started_at, text, origin, model, updated_at) \
         VALUES ($1, $2, $3, 'human', NULL, now()) \
         ON CONFLICT (call_id, started_at) DO UPDATE \
         SET text = EXCLUDED.text, origin = 'human', updated_at = now()",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(text)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE calls SET transcript_state = CASE WHEN $3 = '' THEN 'no_speech' ELSE 'text' END \
         WHERE id = $1 AND started_at = $2",
    )
    .bind(call.0)
    .bind(call.1)
    .bind(text)
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM transcription_queue WHERE call_id = $1 AND started_at = $2")
        .bind(call.0)
        .bind(call.1)
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT pg_notify('scannerpulse_transcripts', $1)")
        .bind(call.0.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}
