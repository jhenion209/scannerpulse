//! Setup's first screen: Simple or Advanced, chosen with the arrow keys and
//! Enter, with a note beside the highlighted choice saying what it does.

use std::io::Write;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::{cursor, execute, style, terminal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Simple,
    Advanced,
}

pub const SIMPLE_NOTE: &str = "ScannerPulse and PostgreSQL are installed on this machine. \
Setup asks for the folder SDRTrunk saves its recordings in, the archive, the time zone, \
the first admin account and, if you like, a transcription server. \
Every advanced option can be set later in the admin page.";

pub const ADVANCED_NOTE: &str = "Also lets you choose: high availability (joining another \
ScannerPulse install, or adding a second machine for failover), where the database runs, \
the certificate (Let's Encrypt or your own), sending logs to a syslog server, and where \
the backups go. Advanced arrives in a later version of setup: choose Simple for now, \
and set these later in the admin page.";

const CHOICES: [(&str, Kind, &str); 2] = [
    ("Simple", Kind::Simple, SIMPLE_NOTE),
    ("Advanced", Kind::Advanced, ADVANCED_NOTE),
];

/// The width of the choices' column.
const LEFT: usize = 14;

/// `text` wrapped to `width` columns, by words.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split_whitespace() {
        let line = lines.last_mut().expect("never empty");
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            lines.push(word.to_string());
        } else {
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
    }
    lines
}

/// The screen's lines with `selected` highlighted (`>`), at `width`
/// columns: the note beside the choices, or below them when the terminal is
/// narrow.
pub fn lines(selected: usize, width: u16) -> Vec<String> {
    let width = usize::from(width).max(30);
    let mut out = vec![
        "ScannerPulse setup".to_string(),
        String::new(),
        "How do you want to set this machine up?".to_string(),
        String::new(),
    ];
    let choices: Vec<String> = CHOICES
        .iter()
        .enumerate()
        .map(|(n, (name, _, _))| format!("{} {name}", if n == selected { ">" } else { " " }))
        .collect();
    let note = CHOICES[selected].2;
    if width >= LEFT + 30 {
        // The note starts on the highlighted choice's row.
        let note = wrap(note, width - LEFT - 2);
        for row in 0..choices.len().max(selected + note.len()) {
            let left = choices.get(row).map(String::as_str).unwrap_or("");
            let right = row
                .checked_sub(selected)
                .and_then(|n| note.get(n))
                .map(String::as_str)
                .unwrap_or("");
            out.push(format!("{left:<LEFT$}  {right}").trim_end().to_string());
        }
    } else {
        out.extend(choices);
        out.push(String::new());
        out.extend(wrap(note, width));
    }
    out.push(String::new());
    out.extend(wrap(
        "Up and Down to choose, Enter to go on, Esc to leave setup.",
        width,
    ));
    out
}

/// Shows the screen until a choice is made; None when the person leaves.
pub fn choose() -> Result<Option<Kind>, String> {
    let fail = |error: std::io::Error| format!("the terminal: {error}");
    terminal::enable_raw_mode().map_err(fail)?;
    let result = (|| -> std::io::Result<Option<Kind>> {
        let mut selected = 0;
        let mut out = std::io::stdout();
        loop {
            let (width, _) = terminal::size()?;
            execute!(
                out,
                terminal::Clear(terminal::ClearType::All),
                cursor::MoveTo(0, 0)
            )?;
            for line in lines(selected, width) {
                write!(out, "{}\r\n", style::style(line))?;
            }
            out.flush()?;
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Up => selected = selected.saturating_sub(1),
                    KeyCode::Down => selected = (selected + 1).min(CHOICES.len() - 1),
                    KeyCode::Enter if CHOICES[selected].1 == Kind::Simple => {
                        return Ok(Some(Kind::Simple));
                    }
                    KeyCode::Enter => {} // Advanced arrives later; its note says so.
                    KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                    _ => {}
                }
            }
        }
    })();
    let _ = terminal::disable_raw_mode();
    result.map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::{ADVANCED_NOTE, lines, wrap};

    #[test]
    fn the_note_sits_beside_the_highlighted_choice() {
        let wide = lines(0, 100);
        let simple = wide.iter().find(|l| l.starts_with("> Simple")).unwrap();
        assert!(
            simple.contains("ScannerPulse and PostgreSQL are installed on this machine."),
            "{simple}"
        );
        assert!(wide.iter().any(|l| l.starts_with("  Advanced")));
        assert!(wide.iter().all(|l| l.len() <= 100));
        let advanced = lines(1, 100);
        assert!(
            advanced
                .iter()
                .any(|l| l.starts_with("> Advanced") && l.contains("Also lets you choose"))
        );
        assert!(
            advanced
                .join(" ")
                .contains("Advanced arrives in a later version")
        );
    }

    #[test]
    fn on_a_narrow_terminal_the_note_goes_below() {
        let narrow = lines(1, 40);
        let at = narrow.iter().position(|l| l == "> Advanced").unwrap();
        assert!(narrow[at + 2].starts_with("Also lets you choose"));
        assert!(narrow.iter().all(|l| l.len() <= 40));
        assert_eq!(
            wrap(ADVANCED_NOTE, 40).join(" "),
            ADVANCED_NOTE
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
