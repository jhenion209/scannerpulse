//! ScannerPulse records, archives and plays back trunked radio calls.

pub mod mp3;
pub mod settings;

/// The schema, applied by `scannerpulse db-update`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
