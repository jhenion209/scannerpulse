//! ScannerPulse records, archives and plays back trunked radio calls.

/// The schema, applied by `scannerpulse db-update`.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
