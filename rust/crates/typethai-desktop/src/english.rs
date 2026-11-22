//! English typed on the English layout whose Thai reading is not Thai (`grid` -> เพรก): no conversion is offered.
//!
//! The flip classifier only knows the 8,583-word English list it was trained with, so a word outside it is judged by
//! its letters alone, and a Thai reading that splits into dictionary pieces (เพ|รก) scores as Thai. The model also
//! carries 46,000+ English words for completion; a word from any of its English lists stays English unless its Thai
//! reading is Thai too, in which case the scores decide as before.
use std::collections::BTreeSet;
use std::ops::Bound;
use typethai_core::{completer::Completer, detector::Detector, keymap::en_to_th, normalize::normalize_thai};

/// What joins English words into one (grid-cols, grid_size, grid/flex).
const JOINS: [char; 3] = ['-', '_', '/'];
/// Words of the trade the model's lists lack.
const TERMS: &str = include_str!("english-terms.txt");
/// Thai that the dictionary lacks and that is spelled by the keys of an English word: particles, slang, names and
/// loanwords seen as words in Thai chat (Wisesight, VISTEC-TP-TH-2021). Like a dictionary word, both readings are
/// real, so the scores decide.
const THAI_TOO: &[&str] = &[
    "ๆ", "ณ", "ก้", "ก่", "กิ", "ท่", "เน้", "หนะ", "น้น", "นิ่", "นนน", "ปป", // marks, particles, spoken forms
    "ทนนะ", "ดีทำ", "ทำไหม", "สนไหม",                                 // two words run together
    "เรด", "แอร", "เพน", "เรฟ", "นทร",                               // loanwords and short forms
    "เรน", "เทน", "เทย", "ริท", "ฟราน", "รสา", "อรา",                 // names
];

/// `word` in a case English is written in (grid, Grid, GRID, TikTok): other mixes of case are Shift typing Thai.
fn english_case(word: &str) -> bool {
    let c: Vec<char> = word.chars().collect();
    let upper = |x: &char| x.is_ascii_uppercase();
    !c.iter().any(upper) || !c.iter().any(char::is_ascii_lowercase) || (c.first().is_some_and(upper) && !c.windows(2).any(|w| upper(&w[0]) && upper(&w[1])))
}
/// `typed` without the punctuation English puts around a word. `[ ] '` stay (they are บ ล ง), and of the marks that
/// are Thai letters (`,` ม, `;` ว, `:` ซ, and `-` ข, `_` `/` ฝ that join English words) only one comes off, from a word
/// long enough to be sure of: `g,,` is เมม and `c''` แงง.
fn core(typed: &str) -> &str {
    let safe = ['.', '!', '?', ')', '}', '"', '`'];
    let t = typed.trim_start_matches(['(', '{', '"', '`']).trim_end_matches(safe);
    match t.strip_suffix([',', ';', ':', '-', '_', '/']).map(|w| w.trim_end_matches(safe)) {
        Some(w) if w.chars().count() >= 3 && w.ends_with(|c: char| c.is_ascii_alphanumeric()) => w,
        _ => t,
    }
}

/// The English words beside the completer's: `TERMS`, and the classifier's word list and brands once the model's
/// lists are loaded (`add`). Sorted, so a prefix is one lookup.
pub struct English { listed: BTreeSet<String> }
impl Default for English {
    fn default() -> Self { English { listed: TERMS.lines().filter(|l| !l.starts_with('#')).flat_map(str::split_whitespace).map(String::from).collect() } }
}
impl English {
    pub fn add(&mut self, words: &[String]) { self.listed.extend(words.iter().map(|w| w.to_ascii_lowercase())) }
    fn knows(&self, word: &str, words: Option<&Completer>) -> bool {
        let k = word.to_ascii_lowercase();
        english_case(word) && (self.listed.contains(&k) || words.is_some_and(|c| c.knows(&k)))
    }
    /// `typed` is English and nothing else: it is an English word, or English words joined by `- _ /`, give or take
    /// the punctuation around it (`core`), and it has no Thai reading, with that punctuation or without. Joined
    /// words need one of four letters or more (grid-id): `lug-up;` is สีเขียว, `me-jk;` ทำข่าว.
    pub fn only(&self, typed: &str, det: &Detector, words: Option<&Completer>) -> bool {
        let w = core(typed);
        if w.is_empty() { return false }
        let english = self.knows(w, words) || (w.contains(JOINS)
            && w.split(JOINS).all(|part| part.len() >= 2 && self.knows(part, words))
            && w.split(JOINS).any(|part| part.len() >= 4));
        english && !is_thai_word(typed, det) && !is_thai_word(w, det)
    }
    /// `typed` is the start of an English word (`gri` of grid) and not a Thai word as far as it goes: too early to
    /// offer Thai. If Thai it is (`gri` of เพราะ), the next keys say so and it is offered then.
    pub fn starts(&self, typed: &str, det: &Detector, words: Option<&Completer>) -> bool {
        let w = typed.trim_start_matches(['(', '{', '"', '`']);
        let (head, last) = w.rsplit_once(JOINS).map_or(("", w), |(head, last)| (head, last));
        if !head.is_empty() && !self.only(head, det, words) { return false }
        if last.is_empty() || !english_case(last) || is_thai_word(typed, det) { return false }
        let k = last.to_ascii_lowercase();
        let after = (Bound::Excluded(k.as_str()), Bound::Unbounded);
        self.listed.range::<str, _>(after).next().is_some_and(|w| w.starts_with(&k)) || words.is_some_and(|c| c.extends(&k))
    }
}
/// The keys `typed` read as Thai: a dictionary word, as typed or with its marks put in order, or `THAI_TOO`. A word
/// got by dropping stray marks is not one (`yoko` ันาน is not นาน).
fn is_thai_word(typed: &str, det: &Detector) -> bool {
    let thai = en_to_th(typed);
    let ordered = normalize_thai(&thai);
    det.th_dict.has(&thai) || THAI_TOO.contains(&thai.as_str())
        || (ordered.chars().count() == thai.chars().count() && det.th_dict.has(&ordered))
}
