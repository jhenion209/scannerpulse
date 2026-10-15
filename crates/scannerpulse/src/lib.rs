//! ScannerPulse records, archives and plays back trunked radio calls.

pub mod accounts;
pub mod accuracy;
pub mod archive;
pub mod asr;
pub mod backfill;
pub mod evaluation;
pub mod favorites;
pub mod filer;
pub mod health;
pub mod ingest;
pub mod jobs;
pub mod lease;
pub mod listing;
pub mod localtime;
pub mod log;
pub mod mp3;
pub mod queue;
pub mod reader;
pub mod search;
pub mod servers;
pub mod sessions;
pub mod settings;
pub mod smartsort;
pub mod store;
pub mod transcriber;
pub mod transcripts;
pub mod watcher;
pub mod web;
pub mod wer;
pub mod xlsx;

/// The schema, applied by `scannerpulse db-update`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
