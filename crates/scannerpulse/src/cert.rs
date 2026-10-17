//! A certificate this machine makes for itself: what `web` serves until the
//! administrator installs another. No authority signed it, so a browser warns
//! once; the connection is encrypted all the same.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::Datelike;
use rcgen::{CertificateParams, DnType, KeyPair};

/// How long a made certificate is good for, in years.
const YEARS: i32 = 5;

/// Writes a certificate for `names` (host names or addresses) to `cert`
/// (readable by all) and its private key to `key` (the owner's alone).
pub fn make(names: &[String], cert: &Path, key: &Path) -> Result<(), String> {
    let Some(first) = names.first() else {
        return Err("a certificate needs at least one name or address".into());
    };
    // An address becomes an address entry, anything else a host name.
    let mut params = CertificateParams::new(names.to_vec())
        .map_err(|_| format!("{names:?} holds something that is not a host name or an address"))?;
    params
        .distinguished_name
        .push(DnType::CommonName, first.as_str());
    let today = chrono::Utc::now().date_naive();
    params.not_before = rcgen::date_time_ymd(today.year(), today.month() as u8, today.day() as u8);
    params.not_after = rcgen::date_time_ymd(
        today.year() + YEARS,
        today.month() as u8,
        today.day().min(28) as u8,
    );
    let pair = KeyPair::generate().map_err(|error| format!("cannot make a key: {error}"))?;
    let made = params
        .self_signed(&pair)
        .map_err(|error| format!("cannot make the certificate: {error}"))?;
    write(cert, made.pem().as_bytes(), 0o644)?;
    write(key, pair.serialize_pem().as_bytes(), 0o600)
}

/// Writes `bytes` to `path`, created with `mode` (an existing file keeps its
/// owner and gets the mode).
fn write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .and_then(|()| file.write_all(bytes))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))
}
