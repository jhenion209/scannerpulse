//! The machine's settings file: what this host's processes read from the
//! environment (the database's address, the host's name, the listen address,
//! the certificate, the secrets), written by `setup` and read by systemd as
//! the services' `EnvironmentFile`. The binary reads it too when an
//! administrator runs a command by hand; a variable already set wins.

use std::path::Path;

/// Where setup writes the file; `SCANNERPULSE_SETTINGS_FILE` names another.
pub const DEFAULT_PATH: &str = "/etc/scannerpulse/scannerpulse.env";

/// The file's `NAME=value` lines, in order. Blank lines and `#` comments are
/// skipped; a value may be wrapped in double quotes, inside which `\"` and
/// `\\` stand for `"` and `\`, as systemd reads them.
pub fn parse(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: not NAME=value", n + 1))?;
        let name = name.trim();
        if !good_name(name) {
            return Err(format!("line {}: {name:?} is not a variable's name", n + 1));
        }
        let value = unquote(value.trim()).map_err(|e| format!("line {}: {e}", n + 1))?;
        // The environment cannot hold a NUL (setting it would panic).
        if value.contains('\0') {
            return Err(format!("line {}: {name}'s value holds a NUL byte", n + 1));
        }
        pairs.push((name.to_string(), value));
    }
    Ok(pairs)
}

/// Capital letters, digits and `_`, not starting with a digit.
fn good_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_')
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// A value, unquoted. Inside quotes only `\"` and `\\` (what `render` writes) are taken; a raw quote or another
/// escape is refused rather than read some other way than systemd reads it.
fn unquote(value: &str) -> Result<String, String> {
    let Some(inner) = value.strip_prefix('"') else {
        return Ok(value.to_string());
    };
    let inner = inner
        .strip_suffix('"')
        .ok_or("a quoted value has no closing quote")?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(escaped @ ('"' | '\\')) => out.push(escaped),
                Some(other) => return Err(format!("a quoted value holds the escape \\{other}")),
                None => return Err("a quoted value ends with a backslash".into()),
            }
        } else if c == '"' {
            return Err("a quoted value holds a quote that is not escaped".into());
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// The file's text for `pairs`, every value quoted, after a heading that says
/// what the file is. A value holding a line break or a NUL is refused.
pub fn render(pairs: &[(&str, &str)]) -> Result<String, String> {
    let mut text = String::from(
        "# ScannerPulse: this machine's settings, written by `scannerpulse setup`.\n\
         # Read by the services (systemd EnvironmentFile) and by the command line.\n",
    );
    for (name, value) in pairs {
        if !good_name(name) {
            return Err(format!("{name:?} is not a variable's name"));
        }
        if value.contains(['\n', '\r']) {
            return Err(format!("{name}'s value holds a line break"));
        }
        if value.contains('\0') {
            return Err(format!("{name}'s value holds a NUL byte"));
        }
        let quoted = value.replace('\\', "\\\\").replace('"', "\\\"");
        text.push_str(&format!("{name}=\"{quoted}\"\n"));
    }
    Ok(text)
}

/// The file's `text` with `name` set to `value`, or removed when `value` is
/// `None`; the other lines keep their order.
pub fn changed(text: &str, name: &str, value: Option<&str>) -> Result<String, String> {
    let mut pairs = parse(text)?;
    match (pairs.iter().position(|(n, _)| n == name), value) {
        (Some(at), Some(value)) => pairs[at].1 = value.to_string(),
        (Some(at), None) => {
            pairs.remove(at);
        }
        (None, Some(value)) => pairs.push((name.to_string(), value.to_string())),
        (None, None) => {}
    }
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    render(&pairs)
}

/// The file's pairs whose names `is_set` does not already claim: the ones a
/// process should take. A missing file gives none.
pub fn to_apply(
    path: &Path,
    is_set: impl Fn(&str) -> bool,
) -> Result<Vec<(String, String)>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        // Read, but not text: no question of who may read it.
        Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
            return Err(format!("{} is not text (UTF-8): {error}", path.display()));
        }
        Err(error) => {
            return Err(format!(
                "cannot read {}: {error} (run as root or as the scannerpulse account)",
                path.display()
            ));
        }
    };
    let pairs = parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(pairs
        .into_iter()
        .filter(|(name, _)| !is_set(name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::{changed, parse, render, to_apply};

    #[test]
    fn a_file_that_is_not_text_gets_no_root_hint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scannerpulse.env");
        std::fs::write(&path, b"A=\xff\xfe\n").unwrap();
        let error = to_apply(&path, |_| false).unwrap_err();
        assert!(!error.contains("run as root"), "{error}");
        assert!(error.contains("not text"), "{error}");
    }

    #[test]
    fn render_refuses_a_name_parse_would_refuse() {
        let error = render(&[("bad name", "x")]).unwrap_err();
        assert!(error.contains("not a variable's name"), "{error}");
        assert!(render(&[("SCANNERPULSE_HOST", "x")]).is_ok());
    }

    #[test]
    fn a_quoted_value_holds_only_what_render_writes() {
        assert!(
            parse("A=\"x\"y\"\n").unwrap_err().contains("quote"),
            "a raw quote inside"
        );
        assert!(
            parse("A=\"x\\ny\"\n").unwrap_err().contains("escape"),
            "an escape render never writes"
        );
        assert_eq!(
            parse("A=\"x\\\"y\\\\z\"\n").unwrap(),
            [("A".to_string(), "x\"y\\z".to_string())]
        );
    }

    #[test]
    fn a_nul_in_a_value_is_refused() {
        // set_var panics on a NUL: the file is refused instead, read or written.
        assert!(parse("A=\"x\0y\"\n").unwrap_err().contains("NUL"));
        assert!(parse("A=x\0y\n").unwrap_err().contains("NUL"));
        assert!(render(&[("A", "x\0y")]).unwrap_err().contains("NUL"));
    }

    #[test]
    fn a_written_file_reads_back_the_same() {
        let pairs = [
            (
                "DATABASE_URL",
                "postgres://scannerpulse@localhost/scannerpulse?sslmode=disable",
            ),
            ("SCANNERPULSE_HOST", "radio-a"),
            ("ODD", r#"a "quoted" \ value"#),
        ];
        let text = render(&pairs).unwrap();
        assert!(text.starts_with("# ScannerPulse"));
        let back = parse(&text).unwrap();
        let back: Vec<(&str, &str)> = back.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        assert_eq!(back, pairs);
        assert!(render(&[("X", "two\nlines")]).is_err());
    }

    #[test]
    fn comments_blank_lines_and_bare_values_are_read_as_systemd_reads_them() {
        let pairs = parse("# a comment\n\nA=plain\n  B = \"spaced value\"  \n").unwrap();
        assert_eq!(
            pairs,
            [
                ("A".to_string(), "plain".to_string()),
                ("B".to_string(), "spaced value".to_string())
            ]
        );
        assert!(parse("not a pair").unwrap_err().starts_with("line 1"));
        assert!(parse("lower=1").is_err());
        assert!(parse("A=\"open").is_err());
    }

    #[test]
    fn a_variable_is_set_replaced_or_removed_and_the_rest_kept() {
        let text = render(&[("A", "1"), ("B", "2")]).unwrap();
        let read = |t: &str| parse(t).unwrap();
        let s = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            read(&changed(&text, "B", Some("3")).unwrap()),
            [s("A", "1"), s("B", "3")]
        );
        assert_eq!(
            read(&changed(&text, "C", Some("4")).unwrap()),
            [s("A", "1"), s("B", "2"), s("C", "4")]
        );
        assert_eq!(read(&changed(&text, "A", None).unwrap()), [s("B", "2")]);
        assert_eq!(
            read(&changed(&text, "C", None).unwrap()),
            [s("A", "1"), s("B", "2")]
        );
    }

    #[test]
    fn a_variable_already_set_wins_and_a_missing_file_gives_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scannerpulse.env");
        assert!(to_apply(&path, |_| false).unwrap().is_empty());
        std::fs::write(&path, "A=\"from file\"\nB=\"from file\"\n").unwrap();
        let taken = to_apply(&path, |name| name == "A").unwrap();
        assert_eq!(taken, [("B".to_string(), "from file".to_string())]);
    }
}
