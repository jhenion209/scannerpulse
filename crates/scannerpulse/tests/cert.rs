//! An installed certificate is checked before setup or `tls install` takes it.

use scannerpulse::cert;

fn made(
    dir: &std::path::Path,
    stem: &str,
    names: &[&str],
) -> (std::path::PathBuf, std::path::PathBuf) {
    let (c, k) = (
        dir.join(format!("{stem}.pem")),
        dir.join(format!("{stem}.key")),
    );
    let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    cert::make(&names, &c, &k).unwrap();
    (c, k)
}

#[test]
fn a_good_certificate_gives_its_names_and_its_end() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = made(dir.path(), "site", &["radio-a.example.org", "192.0.2.10"]);
    let found = cert::check(&c, &k).unwrap();
    assert_eq!(found.names, ["radio-a.example.org", "192.0.2.10"]);
    let days = (found.not_after - chrono::Utc::now()).num_days();
    assert!((1820..=1828).contains(&days), "five years: {days} days");
}

#[test]
fn another_certificates_key_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (c, _) = made(dir.path(), "site", &["radio-a.example.org"]);
    let (_, other) = made(dir.path(), "other", &["radio-b.example.org"]);
    let error = cert::check(&c, &other).unwrap_err();
    assert!(error.contains("is not the key of"), "{error}");
}

#[test]
fn an_ended_certificate_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut params = rcgen::CertificateParams::new(vec!["old.example.org".to_string()]).unwrap();
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2021, 1, 1);
    let pair = rcgen::KeyPair::generate().unwrap();
    let made = params.self_signed(&pair).unwrap();
    let (c, k) = (dir.path().join("old.pem"), dir.path().join("old.key"));
    std::fs::write(&c, made.pem()).unwrap();
    std::fs::write(&k, pair.serialize_pem()).unwrap();
    let error = cert::check(&c, &k).unwrap_err();
    assert!(error.contains("ended on 2021-01-01"), "{error}");
}

#[test]
fn something_that_is_no_certificate_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_, k) = made(dir.path(), "site", &["radio-a.example.org"]);
    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "not a certificate\n").unwrap();
    let error = cert::check(&text, &k).unwrap_err();
    assert!(error.contains("holds no certificate"), "{error}");
}

#[test]
fn a_failed_key_write_leaves_no_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = (
        dir.path().join("site.pem"),
        dir.path().join("no-such-folder/site.key"),
    );
    let names = vec!["radio-a.example.org".to_string()];
    assert!(cert::make(&names, &c, &k).is_err());
    assert!(
        !c.exists(),
        "a certificate without its key would be served with the old one"
    );
}

#[test]
fn a_certificate_needs_a_name_it_can_hold() {
    let dir = tempfile::tempdir().unwrap();
    let (c, k) = (dir.path().join("site.pem"), dir.path().join("site.key"));
    let error = cert::make(&[], &c, &k).unwrap_err();
    assert!(error.contains("at least one name"), "{error}");
    let error = cert::make(&["radio a".to_string()], &c, &k).unwrap_err();
    assert!(error.contains("not a host name or an address"), "{error}");
    assert!(!c.exists() && !k.exists());
}
