//! ScannerPulse command line. Subcommands arrive with the sub-projects that own them.

fn version_line() -> String {
    format!("scannerpulse {}", env!("CARGO_PKG_VERSION"))
}

fn main() {
    println!("{}", version_line());
}

#[cfg(test)]
mod tests {
    use super::version_line;

    #[test]
    fn version_line_names_the_program_then_a_version_number() {
        let line = version_line();
        let (name, version) = line.split_once(' ').expect("two words");
        assert_eq!(name, "scannerpulse");
        assert!(
            version.split('.').count() == 3 && version.chars().next().unwrap().is_ascii_digit(),
            "{version}"
        );
    }
}
