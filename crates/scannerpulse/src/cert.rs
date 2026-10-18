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

/// What a certificate says of itself: the names and addresses it covers,
/// and when it starts and ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub names: Vec<String>,
    pub not_before: chrono::DateTime<chrono::Utc>,
    pub not_after: chrono::DateTime<chrono::Utc>,
    /// It signed itself, as the certificate setup makes does.
    pub self_signed: bool,
}

/// What the certificate in `cert` (PEM, the site's own first) says of
/// itself.
pub fn read(cert: &Path) -> Result<Found, String> {
    use rustls::pki_types::pem::PemObject;
    let first = rustls::pki_types::CertificateDer::pem_file_iter(cert)
        .map_err(|error| format!("{}: not a PEM certificate ({error})", cert.display()))?
        .next()
        .ok_or_else(|| format!("{} holds no certificate", cert.display()))?
        .map_err(|error| format!("{}: not a PEM certificate ({error})", cert.display()))?;
    describe(&first).map_err(|why| format!("{}: {why}", cert.display()))
}

/// What the certificate `key` serves says of itself.
pub fn found(key: &rustls::sign::CertifiedKey) -> Result<Found, String> {
    let first = key.cert.first().ok_or("no certificate")?;
    describe(first)
}

fn describe(der: &[u8]) -> Result<Found, String> {
    let (_, parsed) = x509_parser::parse_x509_certificate(der)
        .map_err(|_| "its certificate cannot be read".to_string())?;
    let time = |t: x509_parser::time::ASN1Time| {
        chrono::DateTime::from_timestamp(t.timestamp(), 0).unwrap_or_default()
    };
    let not_after = time(parsed.validity().not_after);
    let mut names = Vec::new();
    if let Ok(Some(alt)) = parsed.subject_alternative_name() {
        for name in &alt.value.general_names {
            match name {
                x509_parser::extensions::GeneralName::DNSName(dns) => names.push(dns.to_string()),
                x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                    let ip = match bytes.len() {
                        4 => <[u8; 4]>::try_from(*bytes).map(std::net::IpAddr::from).ok(),
                        16 => <[u8; 16]>::try_from(*bytes)
                            .map(std::net::IpAddr::from)
                            .ok(),
                        _ => None,
                    };
                    names.extend(ip.map(|ip| ip.to_string()));
                }
                _ => {}
            }
        }
    }
    if names.is_empty() {
        names.extend(
            parsed
                .subject()
                .iter_common_name()
                .filter_map(|cn| cn.as_str().ok().map(String::from)),
        );
    }
    Ok(Found {
        names,
        not_before: time(parsed.validity().not_before),
        not_after,
        self_signed: parsed.issuer() == parsed.subject(),
    })
}

/// Checks a certificate an administrator gives (`cert`, PEM: the site's own
/// certificate first, then any chain) and its private key (`key`, PEM): both
/// read, the key the certificate's, and the certificate within its dates.
pub fn check(cert: &Path, key: &Path) -> Result<Found, String> {
    let certified = certified(cert, key)?;
    let found = self::found(&certified).map_err(|why| format!("{}: {why}", cert.display()))?;
    let now = chrono::Utc::now();
    if now < found.not_before {
        return Err(format!(
            "{} is good only from {}",
            cert.display(),
            found.not_before
        ));
    }
    if now > found.not_after {
        return Err(format!("{} ended on {}", cert.display(), found.not_after));
    }
    Ok(found)
}

/// The certificate chain in `cert` (PEM: the site's own first) and its
/// private key in `key` (PEM), ready to serve: both read, and the key the
/// certificate's.
pub fn certified(cert: &Path, key: &Path) -> Result<rustls::sign::CertifiedKey, String> {
    let read =
        |path: &Path| std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()));
    certified_from(
        &read(cert)?,
        &read(key)?,
        &cert.display().to_string(),
        &key.display().to_string(),
    )
}

/// The same from PEM text in memory; `cert_name` and `key_name` name them
/// in an error.
pub fn certified_from(
    cert: &[u8],
    key: &[u8],
    cert_name: &str,
    key_name: &str,
) -> Result<rustls::sign::CertifiedKey, String> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert)
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{cert_name}: not a PEM certificate ({error})"))?;
    if certs.is_empty() {
        return Err(format!("{cert_name} holds no certificate"));
    }
    let private = PrivateKeyDer::from_pem_slice(key)
        .map_err(|error| format!("{key_name}: not a PEM private key ({error})"))?;
    let signing = rustls::crypto::ring::sign::any_supported_type(&private)
        .map_err(|_| format!("{key_name}: a kind of key this program cannot use"))?;
    let certified = rustls::sign::CertifiedKey::new(certs, signing);
    certified
        .keys_match()
        .map_err(|_| format!("{key_name} is not the key of {cert_name}"))?;
    Ok(certified)
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
