//! The password file setup writes for a database the person runs is read
//! back by the services (sqlx) as it was typed.

use std::str::FromStr;

use scannerpulse::setup::steps::Remote;

#[test]
fn the_services_read_back_the_password_setup_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pgpass");
    // SAFETY: the only test in this binary, so nothing reads the
    // environment while it is set.
    unsafe { std::env::set_var("PGPASSFILE", &path) };
    for remote in [
        Remote {
            host: "db.example.org".into(),
            port: 5433,
            name: "calls".into(),
            role: "sp".into(),
            password: "se:cret".into(),
        },
        Remote {
            host: "2001:db8::40".into(),
            port: 5432,
            name: "radio calls".into(),
            role: "sp@site".into(),
            password: "a\\b:c d".into(),
        },
    ] {
        std::fs::write(&path, remote.pgpass_line()).unwrap();
        // Mode 600, as setup writes it: on Linux the reader ignores a file
        // others can read.
        std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .unwrap();
        let options = sqlx::postgres::PgConnectOptions::from_str(&remote.url()).unwrap();
        let shown = format!("{options:?}");
        let expected = format!("password: {:?}", Some(&remote.password));
        assert!(shown.contains(&expected), "{}{shown}", remote.pgpass_line());
    }
}
