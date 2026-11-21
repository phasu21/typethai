//! Corpus text -> word runs -> adjacent word pair counts, for the bigram model sections.
use crate::{dict::Dict, normalize::normalize_thai};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang { Th, En }
/// A pair: (previous word, next word, count).
pub type Pair = (String, String, u64);

#[derive(Clone, Copy, PartialEq)]
enum Class { Thai, Latin, Space, Break }
fn class(c: char) -> Class {
    match c {
        // Thai letters and marks; ฯ, ๆ, ๏, the digits and ๚ ๛ act as punctuation.
        '\u{0E01}'..='\u{0E3A}' | '\u{0E40}'..='\u{0E4E}' if !matches!(c, '\u{0E2F}' | '\u{0E46}') => Class::Thai,
        'a'..='z' | 'A'..='Z' | '\'' | '\u{2019}' => Class::Latin,
        '\n' | '\r' => Class::Break,
        '\u{200B}' => Class::Space,
        c if c.is_whitespace() => Class::Space,
        _ => Class::Break,
    }
}

/// Splits one line into runs of adjacent words of one language. A run ends at punctuation, a digit, a Thai/English
/// switch, a Thai piece the dictionary does not cover, or a quote mark; spaces do not end it. An apostrophe inside an
/// English word keeps the word whole (`you're`, `it's`; ’ is read as '), so contractions get pairs like any other word
/// (model-v6 on). One at the start or end of a token is a quote mark and ends the run there. Thai is normalised and
/// segmented over `dict`, English words are lowercased. Only runs of 2+ words are returned.
pub fn word_runs(line: &str, dict: &Dict) -> Vec<(Lang, Vec<String>)> { runs(line, dict, Thai::Segment) }
/// `word_runs` over a line whose Thai words are already split, by spaces or U+200B (the build-time LATTE
/// segmentation, `scripts/bigrams/latte/`): each Thai piece is one word, kept when the dictionary has it, else it ends
/// the run.
pub fn word_runs_presegmented(line: &str, dict: &Dict) -> Vec<(Lang, Vec<String>)> { runs(line, dict, Thai::Presegmented) }
/// `word_runs` for the English runs only: a Thai chunk ends the run it is in, as in `word_runs`, without being
/// segmented. The English runs are the same as `word_runs` gives.
pub fn english_runs(line: &str, dict: &Dict) -> Vec<Vec<String>> {
    runs(line, dict, Thai::Skip).into_iter().filter(|r| r.0 == Lang::En).map(|r| r.1).collect()
}
#[derive(Clone, Copy, PartialEq)]
enum Thai { Segment, Presegmented, Skip }
fn runs(line: &str, dict: &Dict, thai: Thai) -> Vec<(Lang, Vec<String>)> {
    let mut r = Runs::default();
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let k = class(c);
        let len = rest.find(|x| class(x) != k).unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(len); rest = tail;
        match k {
            Class::Space => {}
            Class::Thai if thai == Thai::Skip => r.end(),
            Class::Thai if thai == Thai::Presegmented => {
                let w = normalize_thai(chunk);
                if dict.has(&w) { r.push(Lang::Th, w) } else { r.end() }
            }
            Class::Thai => for p in dict.segment(&normalize_thai(chunk)) {
                if dict.has(p) { r.push(Lang::Th, p.to_string()) } else { r.end() }
            },
            Class::Latin => {
                let w = chunk.replace('\u{2019}', "'").to_ascii_lowercase();
                let core = w.trim_matches('\'');
                if w.starts_with('\'') { r.end() }
                if core.is_empty() || core.contains("''") { r.end(); continue }
                r.push(Lang::En, core.to_string());
                if w.ends_with('\'') { r.end() }
            }
            Class::Break => r.end(),
        }
    }
    r.end();
    r.done
}
/// Words in front of `'s` that make it a contraction (`it's` = it is / it has), not a possessive (`john's`).
const S_STEMS: &[&str] = &["it", "he", "she", "that", "what", "who", "where", "there", "here", "how", "when", "why", "let",
    "this", "everyone", "everybody", "everything", "someone", "somebody", "something", "nobody", "nothing", "one"];
/// An English contraction as `word_runs` writes it (lowercase, one inner apostrophe): `don't`, `can't`, `you're`,
/// `we've`, `i'll`, `i'd`, `i'm`, and `'s` after a pronoun or a question word (`it's`, `who's`, `let's`). Not a
/// possessive (`john's`) or a name (`o'neil`).
pub fn is_contraction(w: &str) -> bool {
    let Some((stem, tail)) = w.split_once('\'') else { return false };
    if stem.is_empty() || !stem.bytes().chain(tail.bytes()).all(|b| b.is_ascii_lowercase()) { return false }
    match tail {
        "t" => stem.len() >= 2 && stem.ends_with('n'),
        "re" | "ve" | "ll" | "d" | "m" => true,
        "s" => S_STEMS.contains(&stem),
        _ => false,
    }
}
#[derive(Default)]
struct Runs { done: Vec<(Lang, Vec<String>)>, cur: Option<(Lang, Vec<String>)> }
impl Runs {
    fn end(&mut self) { if let Some(r) = self.cur.take().filter(|r| r.1.len() >= 2) { self.done.push(r) } }
    fn push(&mut self, lang: Lang, w: String) {
        match &mut self.cur {
            Some((l, ws)) if *l == lang => ws.push(w),
            _ => { self.end(); self.cur = Some((lang, vec![w])) }
        }
    }
}

/// Pair counts for one language, with interned words.
#[derive(Default)]
pub struct PairCounts { ids: HashMap<String, u32>, words: Vec<String>, pairs: HashMap<(u32, u32), u64> }
impl PairCounts {
    fn id(&mut self, w: &str) -> u32 {
        if let Some(&i) = self.ids.get(w) { return i }
        let i = self.words.len() as u32; self.ids.insert(w.to_string(), i); self.words.push(w.to_string()); i
    }
    pub fn add_run(&mut self, words: &[String]) {
        for p in words.windows(2) { let k = (self.id(&p[0]), self.id(&p[1])); *self.pairs.entry(k).or_default() += 1; }
    }
    pub fn into_pairs(self) -> Vec<Pair> {
        let w = self.words;
        self.pairs.into_iter().map(|((p, n), c)| (w[p as usize].clone(), w[n as usize].clone(), c)).collect()
    }
}
/// Thai and English pair counts over a body of text.
#[derive(Default)]
pub struct Counts { pub th: PairCounts, pub en: PairCounts }
impl Counts {
    pub fn add_line(&mut self, line: &str, dict: &Dict) {
        for (lang, ws) in word_runs(line, dict) { match lang { Lang::Th => &mut self.th, Lang::En => &mut self.en }.add_run(&ws) }
    }
    /// `add_line` for the English pairs only (`english_runs`).
    pub fn add_english_line(&mut self, line: &str, dict: &Dict) { for ws in english_runs(line, dict) { self.en.add_run(&ws) } }
    /// `add_line` for a line whose Thai words are already split (`word_runs_presegmented`).
    pub fn add_presegmented_line(&mut self, line: &str, dict: &Dict) {
        for (lang, ws) in word_runs_presegmented(line, dict) { match lang { Lang::Th => &mut self.th, Lang::En => &mut self.en }.add_run(&ws) }
    }
}

/// Pruning: drop pairs `keep` rejects, then pairs seen fewer than `min_count` times, then all but the `top` most
/// frequent followers of each previous word.
pub struct Prune<'a> { pub min_count: u64, pub top: usize, pub keep: &'a dyn Fn(&str, &str) -> bool }
/// Sorted by previous word, then count descending, then next word.
pub fn prune(mut pairs: Vec<Pair>, p: &Prune) -> Vec<Pair> {
    pairs.retain(|(a, b, c)| *c >= p.min_count && (p.keep)(a, b));
    sort(&mut pairs);
    let mut out: Vec<Pair> = Vec::with_capacity(pairs.len());
    let mut kept = 0;
    for x in pairs {
        kept = if out.last().is_some_and(|l| l.0 == x.0) { kept + 1 } else { 1 };
        if kept <= p.top { out.push(x) }
    }
    out
}
pub fn sort(pairs: &mut [Pair]) { pairs.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.cmp(&a.2)).then_with(|| a.1.cmp(&b.1))); }

/// Pair lists summed: a pair in several lists gets the sum of its counts. Sorted like `prune`.
pub fn merge(lists: impl IntoIterator<Item = Vec<Pair>>) -> Vec<Pair> {
    let mut m: HashMap<(String, String), u64> = HashMap::new();
    for l in lists { for (a, b, c) in l { *m.entry((a, b)).or_default() += c } }
    let mut out: Vec<Pair> = m.into_iter().map(|((a, b), c)| (a, b, c)).collect();
    sort(&mut out);
    out
}
/// `prev\tnext\tcount` lines.
pub fn to_tsv(pairs: &[Pair]) -> String { pairs.iter().map(|(a, b, c)| format!("{a}\t{b}\t{c}\n")).collect() }
pub fn from_tsv(s: &str) -> Result<Vec<Pair>, String> {
    s.lines().enumerate().filter(|(_, l)| !l.is_empty()).map(|(i, l)| {
        let f: Vec<&str> = l.split('\t').collect();
        match f[..] {
            [a, b, c] => Ok((a.to_string(), b.to_string(), c.parse().map_err(|e| format!("line {}: {e}", i + 1))?)),
            _ => Err(format!("line {}: expected prev<TAB>next<TAB>count", i + 1)),
        }
    }).collect()
}
