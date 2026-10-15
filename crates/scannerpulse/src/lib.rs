//! ScannerPulse records, archives and plays back trunked radio calls.

pub mod archive;
pub mod asr;
pub mod backfill;
pub mod filer;
pub mod health;
pub mod ingest;
pub mod lease;
pub mod localtime;
pub mod log;
pub mod mp3;
pub mod queue;
pub mod reader;
pub mod servers;
pub mod settings;
pub mod store;
pub mod transcriber;
pub mod transcripts;
pub mod watcher;
pub mod wer;

/// The schema, applied by `scannerpulse db-update`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
