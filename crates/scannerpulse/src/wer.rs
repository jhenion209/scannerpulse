//! The word error rate of a transcription against a person's text.
//!
//! The rate is over all the calls together: the errors of every call added up,
//! divided by the words of every person's text. An average of each call's own
//! rate would weigh a call of three words as much as a call of thirty.

/// The words that are compared: the text in lowercase, with every character
/// that is not a letter, a digit, `_` or `-` read as a space. "Engine 13,"
/// and "engine 13" are the same words; "10-4" stays one word.
pub fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// The fewest words to replace, remove or add to turn `heard` into `said`.
pub fn errors(said: &[String], heard: &[String]) -> usize {
    let mut previous: Vec<usize> = (0..=heard.len()).collect();
    for (i, s) in said.iter().enumerate() {
        let mut current = vec![i + 1; heard.len() + 1];
        for (j, h) in heard.iter().enumerate() {
            current[j + 1] = (previous[j + 1] + 1)
                .min(current[j] + 1)
                .min(previous[j] + usize::from(s != h));
        }
        previous = current;
    }
    previous[heard.len()]
}

/// The score of one transcription source over a set of labeled calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Score {
    pub calls: usize,
    pub errors: usize,
    /// The words of the persons' texts.
    pub words: usize,
    /// The calls a person labeled as having no speech (an empty text).
    pub silent: usize,
    /// Of those, the calls that got text anyway.
    pub phantom: usize,
}

impl Score {
    /// Adds one call: what the person said it says, and what was heard.
    /// Returns the call's errors.
    pub fn add(&mut self, said: &str, heard: &str) -> usize {
        let said = words(said);
        let heard = words(heard);
        let errors = errors(&said, &heard);
        self.calls += 1;
        self.errors += errors;
        self.words += said.len();
        if said.is_empty() {
            self.silent += 1;
            if !heard.is_empty() {
                self.phantom += 1;
            }
        }
        errors
    }

    /// The word error rate; `None` while the texts have no word.
    pub fn wer(&self) -> Option<f64> {
        (self.words > 0).then(|| self.errors as f64 / self.words as f64)
    }

    /// One line: the rate, its counts, and the calls labeled silent.
    pub fn summary(&self) -> String {
        let rate = match self.wer() {
            Some(wer) => format!("WER {:.1}%", wer * 100.0),
            None => "WER not known".to_string(),
        };
        let mut line = format!(
            "{rate} ({} errors in {} words, {} calls)",
            self.errors, self.words, self.calls
        );
        if self.silent > 0 {
            line.push_str(&format!(
                "; text on {} of {} calls labeled silent",
                self.phantom, self.silent
            ));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::{Score, errors, words};

    fn w(text: &str) -> Vec<String> {
        words(text)
    }

    #[test]
    fn case_and_punctuation_are_not_errors() {
        assert_eq!(w("Engine 13, respond!"), w("engine 13 respond"));
        assert_eq!(w("10-4."), vec!["10-4"]);
        assert_eq!(w("  "), Vec::<String>::new());
        assert_eq!(w("Café"), vec!["café"]);
    }

    #[test]
    fn errors_count_replaced_removed_and_added_words() {
        assert_eq!(errors(&w("a b c"), &w("a b c")), 0);
        assert_eq!(errors(&w("a b c"), &w("a x c")), 1);
        assert_eq!(errors(&w("a b c"), &w("a c")), 1);
        assert_eq!(errors(&w("a b c"), &w("a b c d")), 1);
        assert_eq!(errors(&w("a b"), &w("")), 2);
        assert_eq!(errors(&w(""), &w("a b")), 2);
    }

    #[test]
    fn the_rate_is_over_all_the_words_not_an_average_of_calls() {
        let mut score = Score::default();
        assert_eq!(score.wer(), None);
        score.add(
            "one two three four five six seven eight nine ten",
            "one two three four five six seven eight nine ten",
        );
        score.add("ok", "no");
        // 1 error in 11 words, where an average of the calls would say 50%.
        assert_eq!(score.errors, 1);
        assert_eq!(score.words, 11);
        assert!((score.wer().unwrap() - 1.0 / 11.0).abs() < 1e-12);
    }

    #[test]
    fn text_on_a_call_labeled_silent_is_counted() {
        let mut score = Score::default();
        score.add("", "");
        score.add("", "thank you");
        assert_eq!((score.silent, score.phantom), (2, 1));
        assert_eq!(score.errors, 2);
        assert_eq!(
            score.summary(),
            "WER not known (2 errors in 0 words, 2 calls); text on 1 of 2 calls labeled silent"
        );
    }
}
