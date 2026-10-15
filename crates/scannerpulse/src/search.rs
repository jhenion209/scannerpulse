//! The search box: its text becomes a PostgreSQL full-text query over the
//! transcripts.
//!
//! - `engine 5`: every word (AND);
//! - `"shots fired"`: the words next to each other;
//! - `fire OR smoke`: either side;
//! - `fire -smoke`, `fire NOT smoke`: without the word;
//! - `struct*`: words that start with it.
//!
//! `OR`, `AND` and `NOT` are operators only in capitals. `NOT` binds tightest,
//! then AND, then OR. Words are split as the transcripts' search column splits
//! them, on every character that is not a letter or a digit, so `10-4` is
//! `10` and `4`. Odd input never fails: it gives a smaller query, or none.

/// More terms than a person types; it bounds the query's size.
const MAX_TERMS: usize = 20;
/// The most words the whole query holds: one term the column splits, such
/// as `a-a-a-...`, is one word per piece.
const MAX_WORDS: usize = 32;
/// A longer word is cut to this many characters.
const MAX_WORD_CHARS: usize = 100;

/// The query for `to_tsquery('simple', ...)`, or `None` when the text holds
/// nothing to search for. Every word is a quoted lexeme, so no text of the
/// box is read as an operator.
pub fn tsquery(text: &str) -> Option<String> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new()];
    let mut negate = false;
    let mut words = MAX_WORDS;
    for token in tokens(text).into_iter().take(MAX_TERMS) {
        match token.as_str() {
            "OR" => {
                negate = false;
                groups.push(Vec::new());
                continue;
            }
            "AND" => continue,
            "NOT" => {
                negate = true;
                continue;
            }
            _ => {}
        }
        let (minus, body) = match token.strip_prefix('-') {
            Some(rest) if !rest.is_empty() => (true, rest),
            _ => (false, token.as_str()),
        };
        let term = match body.strip_prefix('"') {
            Some(rest) => phrase(rest.strip_suffix('"').unwrap_or(rest), &mut words),
            None => word(body, &mut words),
        };
        if let Some(term) = term {
            let group = groups.last_mut().expect("there is always a group");
            group.push(if negate || minus {
                format!("!{term}")
            } else {
                term
            });
        }
        negate = false;
    }
    // A group of exclusions alone would match nearly every call.
    let groups: Vec<String> = groups
        .into_iter()
        .filter(|g| g.iter().any(|t| !t.starts_with('!')))
        .map(|g| format!("({})", g.join(" & ")))
        .collect();
    (!groups.is_empty()).then(|| groups.join(" | "))
}

/// Phrases in quotes, then the other runs of non-space characters.
fn tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        let quoted = rest.starts_with('"') || rest.starts_with("-\"");
        let end = if quoted {
            let open = rest.find('"').expect("starts with a quote") + 1;
            match rest[open..].find('"') {
                Some(close) => open + close + 1,
                None => rest.len(),
            }
        } else {
            rest.find(char::is_whitespace).unwrap_or(rest.len())
        };
        tokens.push(rest[..end].to_string());
        rest = rest[end..].trim_start();
    }
    tokens
}

/// The pieces of `text` that the search column keeps: runs of letters and
/// digits, in lowercase, at most `words` of them, each cut to
/// `MAX_WORD_CHARS`; `words` is lowered by those taken.
fn pieces(text: &str, words: &mut usize) -> Vec<String> {
    let pieces: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .take(*words)
        .map(|p| {
            p.chars()
                .take(MAX_WORD_CHARS)
                .collect::<String>()
                .to_lowercase()
        })
        .collect();
    *words -= pieces.len();
    pieces
}

fn lexeme(piece: &str) -> String {
    format!("'{}'", piece.replace('\\', "\\\\").replace('\'', "''"))
}

/// A word, or a prefix when it ends with `*`; a word the column would split
/// is all its pieces.
fn word(text: &str, words: &mut usize) -> Option<String> {
    let (body, prefix) = match text.strip_suffix('*') {
        Some(body) => (body, true),
        None => (text, false),
    };
    let pieces = pieces(body, words);
    let last = pieces.len().checked_sub(1)?;
    let terms: Vec<String> = pieces
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if prefix && i == last {
                format!("{}:*", lexeme(p))
            } else {
                lexeme(p)
            }
        })
        .collect();
    Some(if terms.len() == 1 {
        terms.into_iter().next().expect("one term")
    } else {
        format!("({})", terms.join(" & "))
    })
}

fn phrase(text: &str, words: &mut usize) -> Option<String> {
    let pieces = pieces(text, words);
    match pieces.len() {
        0 => None,
        1 => Some(lexeme(&pieces[0])),
        _ => Some(format!(
            "({})",
            pieces
                .iter()
                .map(|p| lexeme(p))
                .collect::<Vec<_>>()
                .join(" <-> ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::tsquery;

    fn q(text: &str) -> Option<String> {
        tsquery(text)
    }

    #[test]
    fn words_are_all_required() {
        assert_eq!(q("Engine 5").as_deref(), Some("('engine' & '5')"));
        assert_eq!(q("engine AND 5").as_deref(), Some("('engine' & '5')"));
    }

    #[test]
    fn a_phrase_keeps_its_words_together() {
        assert_eq!(
            q("\"shots fired\" north").as_deref(),
            Some("(('shots' <-> 'fired') & 'north')")
        );
        assert_eq!(
            q("\"unclosed phrase").as_deref(),
            Some("(('unclosed' <-> 'phrase'))")
        );
    }

    #[test]
    fn or_not_and_minus_follow_their_precedence() {
        assert_eq!(q("fire OR smoke").as_deref(), Some("('fire') | ('smoke')"));
        assert_eq!(q("fire -smoke").as_deref(), Some("('fire' & !'smoke')"));
        assert_eq!(q("fire NOT smoke").as_deref(), Some("('fire' & !'smoke')"));
        assert_eq!(
            q("a b OR c -\"d e\"").as_deref(),
            Some("('a' & 'b') | ('c' & !('d' <-> 'e'))")
        );
        // Lowercase operators are words.
        assert_eq!(
            q("fire or smoke").as_deref(),
            Some("('fire' & 'or' & 'smoke')")
        );
    }

    #[test]
    fn a_prefix_and_a_split_word() {
        assert_eq!(q("struct*").as_deref(), Some("('struct':*)"));
        assert_eq!(q("10-4").as_deref(), Some("(('10' & '4'))"));
        assert_eq!(q("med-4*").as_deref(), Some("(('med' & '4':*))"));
    }

    #[test]
    fn odd_input_gives_a_smaller_query_or_none() {
        assert_eq!(q(""), None);
        assert_eq!(q("   OR AND NOT  "), None);
        assert_eq!(
            q("-smoke"),
            None,
            "exclusions alone match nearly everything"
        );
        assert_eq!(q("!!! ''' \\\\"), None);
        assert_eq!(q("it's").as_deref(), Some("(('it' & 's'))"));
        assert_eq!(q("O'Brien*").as_deref(), Some("(('o' & 'brien':*))"));
        let long = "w ".repeat(500);
        assert_eq!(q(&long).unwrap().matches("'w'").count(), 20);
    }

    #[test]
    fn the_query_holds_a_bounded_number_of_words_of_bounded_length() {
        // One token the column splits into 30,000 words.
        let split = "a-".repeat(30_000);
        assert_eq!(q(&split).unwrap().matches("'a'").count(), 32);
        let long = "b".repeat(60_000);
        assert_eq!(q(&long).unwrap(), format!("('{}')", "b".repeat(100)));
        let phrase = format!("\"{}\"", "c ".repeat(30_000));
        assert_eq!(q(&phrase).unwrap().matches("'c'").count(), 32);
    }

    #[test]
    fn query_syntax_typed_in_the_box_is_only_text() {
        assert_eq!(
            q("fire & smoke | (x) <-> y:*").as_deref(),
            Some("('fire' & 'smoke' & 'x' & 'y':*)")
        );
        assert_eq!(
            q("NOT \"shots fired\" engine").as_deref(),
            Some("(!('shots' <-> 'fired') & 'engine')")
        );
        assert_eq!(
            q("engine -med-4").as_deref(),
            Some("('engine' & !('med' & '4'))")
        );
        assert_eq!(q("a OR -b").as_deref(), Some("('a')"));
        assert_eq!(q("NOT AND x"), None);
    }
}
