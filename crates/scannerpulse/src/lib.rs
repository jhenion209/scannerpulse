//! ScannerPulse records, archives and plays back trunked radio calls.

pub mod archive;
pub mod filer;
pub mod localtime;
pub mod log;
pub mod mp3;
pub mod reader;
pub mod settings;
pub mod store;

/// The schema, applied by `scannerpulse db-update`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
