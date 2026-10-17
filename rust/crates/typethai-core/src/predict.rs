//! Inputs for the contextual word predictor: typeThai text read as the words and the gaps between them that a word
//! n-gram model learns from.
//!
//! The predictor (`typethai-predict`, ported from oh-my-pi's `pi-predict`) reads space-separated prose: a word is
//! `[\p{L}\p{M}']+`, and the characters between two words say how they relate — a space keeps the words before it as
//! context, a full stop starts a sentence, and anything else (a symbol, a digit, Thai the dictionary does not cover)
//! breaks the chain. Thai does not separate words with spaces, so a Thai run is segmented with the dictionary first,
//! and each non-word run is rewritten as the one character the model reads it by: `" "` (join), `"\n"` (sentence) or
//! `"|"` (break).
//!
//! Segmentation and gap rules mirror `Engine::screen_words` (`typethai-desktop`), the words the engine already learns
//! into `WordMemory`, so the model's vocabulary and the user's completions agree.
use std::sync::Arc;
use typethai_predict::WordOracle;

use crate::dict::Dict;
use crate::keymap::is_thai;

/// How the model reads the gap in front of a word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gap {
    /// Space, tab, comma, semicolon, dash, a wrapping quote: the words before it stay context.
    Join,
    /// Line break, `.`, `!`, `?`, `:`: the words before it are a finished sentence.
    Stop,
    /// Anything else — a symbol, a digit, an unknown Thai piece, a script switch with no separator: no context.
    Break,
}

/// The one character the model reads a gap by. `"|"` reaches the model's own classifier as [`Gap::Break`].
fn gap_text(gap: Gap) -> &'static str {
    match gap { Gap::Join => " ", Gap::Stop => "\n", Gap::Break => "|" }
}

/// The gap a run of non-word characters forms: a break beats a stop, a stop beats a join.
fn classify(run: &str) -> Gap {
    let mut gap = Gap::Join;
    for c in run.chars() {
        match c {
            ' ' | '\t' | ',' | ';' | '-' | '\u{2013}' | '\u{2014}' | '(' | ')' | '"' | '*' | '\u{201C}' | '\u{201D}' | '\u{2018}' | '\u{2019}' => {}
            '\n' | '.' | '!' | '?' | ':' => if gap == Gap::Join { gap = Gap::Stop },
            _ => return Gap::Break,
        }
    }
    gap
}

/// Thai run pieces the dictionary covers and that carry no digit (the words `screen_words` hands the word memory).
fn piece(piece: &str, dict: &Dict) -> bool { dict.has(piece) && !piece.chars().any(char::is_numeric) }

/// The words of `line` with the gap in front of each, English lowercased and Thai split on `dict`. A Thai piece the
/// dictionary does not cover is dropped and breaks the chain, as it does for the word memory.
pub fn words(line: &str, dict: &Dict) -> Vec<(Gap, String)> {
    let ch: Vec<char> = line.chars().collect();
    let mut out: Vec<(Gap, String)> = vec![];
    let mut gap = String::new();
    let mut at = 0;
    let push = |gap: &mut String, word: Option<String>, out: &mut Vec<(Gap, String)>| {
        // a word at the very start reads as a sentence start; `engine_text` writes no gap in front of it
        let kind = if gap.is_empty() && out.is_empty() { Gap::Stop } else { classify(gap) };
        match word {
            Some(w) => { gap.clear(); out.push((kind, w)) }
            // an unknown piece is dropped: the next word gets a break in front of it
            None => { gap.clear(); gap.push('|') }
        }
    };
    while at < ch.len() {
        let c = ch[at];
        if is_thai(c) {
            let start = at;
            while at < ch.len() && is_thai(ch[at]) { at += 1 }
            let run: String = ch[start..at].iter().collect();
            for w in dict.segment(&run) { push(&mut gap, piece(w, dict).then(|| w.to_string()), &mut out) }
        } else if c.is_ascii_alphabetic() || c == '\'' {
            let start = at;
            while at < ch.len() && (ch[at].is_ascii_alphabetic() || ch[at] == '\'') { at += 1 }
            push(&mut gap, Some(ch[start..at].iter().collect::<String>().to_ascii_lowercase()), &mut out);
        } else {
            gap.push(c);
            at += 1;
        }
    }
    out
}

/// `line` as the model reads it: its words joined by the gap character between them.
pub fn engine_text(line: &str, dict: &Dict) -> String {
    let mut out = String::new();
    for (gap, word) in words(line, dict) {
        if !out.is_empty() { out.push_str(gap_text(gap)) }
        out.push_str(&word);
    }
    out
}

/// The words this user's dictionary knows: the model's Thai dictionary and its English word list. The predictor
/// trusts them from their first use, where a word it has never seen has to be typed twice to be offered.
pub struct KnownWords { thai: Arc<Dict>, english: Arc<std::collections::HashSet<String>> }
impl KnownWords {
    pub fn new(thai: Arc<Dict>, english: Arc<std::collections::HashSet<String>>) -> Self { KnownWords { thai, english } }
}
impl WordOracle for KnownWords {
    fn known(&self, word: &str) -> bool { self.thai.has(word) || self.english.contains(word) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(words: &[&str]) -> Dict { Dict::new(words.iter().map(|w| w.to_string())) }

    #[test]
    fn english_words_keep_their_gaps() {
        let d = dict(&[]);
        assert_eq!(words("can you refactor the ", &d).len(), 4);
        assert_eq!(engine_text("can you refactor the ", &d), "can you refactor the");
        assert_eq!(engine_text("done. Then fix", &d), "done\nthen fix");
        // a symbol or a digit is not prose: the chain breaks there
        assert_eq!(engine_text("run foo_bar", &d), "run foo|bar");
        assert_eq!(engine_text("x = 1 hello", &d), "x|hello");
    }

    #[test]
    fn thai_runs_are_segmented_and_unknown_pieces_break_the_chain() {
        let d = dict(&["ภาษา", "ไทย", "ดี"]);
        assert_eq!(engine_text("ภาษาไทยดี", &d), "ภาษา ไทย ดี");
        // a piece the dictionary does not cover breaks the chain; one with nothing after it leaves no word behind
        assert_eq!(engine_text("ภาษาไทยดี", &dict(&["ภาษา", "ดี"])), "ภาษา|ดี");
        assert_eq!(engine_text("ภาษาไทย", &dict(&["ภาษา"])), "ภาษา");
        // a Thai/Latin switch with no separator joins, as the model's own gap rules read it
        assert_eq!(engine_text("hello ภาษา", &d), "hello ภาษา");
    }

    #[test]
    fn the_first_word_of_a_line_starts_a_sentence() {
        let d = dict(&[]);
        assert_eq!(words("hello", &d), vec![(Gap::Stop, "hello".to_string())]);
        assert_eq!(engine_text("hello", &d), "hello");
    }

    #[test]
    fn known_words_come_from_both_dictionaries() {
        let k = KnownWords::new(Arc::new(dict(&["ภาษา"])), Arc::new(["hello".to_string()].into_iter().collect()));
        assert!(k.known("ภาษา"));
        assert!(k.known("hello"));
        assert!(!k.known("สวัสดี"));
    }
}
