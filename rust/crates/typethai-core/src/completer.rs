//! Prefix completion over a frequency-ranked word list.
use crate::dict::Dict;
use std::collections::{BTreeMap, HashMap};

/// Sorted by word (byte order), so the proper extensions of any prefix are one contiguous run right after it.
pub struct Completer { words: Vec<(String, f64)> }
/// `confidence`: the top word's share of the frequency of all extensions plus the prefix itself when it is a word, so
/// a finished word (`you`, `ฉัน`) is not read as a confident prefix of its extensions.
#[derive(Clone, Debug, Default, PartialEq)] pub struct Completions { pub words: Vec<String>, pub confidence: f64 }

impl Completer {
    pub fn new<I: IntoIterator<Item = (String, f64)>>(it: I) -> Self {
        let mut m: BTreeMap<String, f64> = BTreeMap::new();
        for (w, f) in it {
            if w.is_empty() || f.is_nan() || f <= 0.0 || w.chars().any(char::is_whitespace) { continue }
            *m.entry(w.to_ascii_lowercase()).or_default() += f;
        }
        Completer { words: m.into_iter().collect() }
    }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }
    fn start(&self, prefix: &str) -> usize { self.words.partition_point(|(w, _)| w.as_str() <= prefix) }
    fn range(&self, prefix: &str) -> &[(String, f64)] {
        let end = self.words.partition_point(|(w, _)| w.as_str() <= prefix || w.starts_with(prefix));
        &self.words[self.start(prefix)..end]
    }
    pub fn extends(&self, prefix: &str) -> bool {
        let p = prefix.to_ascii_lowercase();
        self.words.get(self.start(&p)).is_some_and(|(w, _)| w.starts_with(&p))
    }
    pub fn complete(&self, prefix: &str, k: usize) -> Completions {
        let p = prefix.to_ascii_lowercase();
        let r = self.range(&p);
        let own = self.start(&p).checked_sub(1).map(|i| &self.words[i]).filter(|(w, _)| *w == p).map_or(0.0, |e| e.1);
        let (mut total, mut max) = (own, 0.0f64);
        let mut top: Vec<&(String, f64)> = Vec::new();
        for e in r {
            total += e.1; max = max.max(e.1);
            // `r` is word-ascending, so inserting after equal freqs keeps ties in word order.
            let at = top.partition_point(|t| t.1 >= e.1);
            if at < k { top.insert(at, e); top.truncate(k); }
        }
        let confidence = if total > 0.0 { max / total } else { 0.0 };
        Completions { words: top.into_iter().map(|(w, _)| recase(w, prefix)).collect(), confidence }
    }
}
/// Word pair counts from a corpus: each previous word (lowercase) with its next words, most frequent first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bigrams { next: HashMap<String, Vec<(String, f64)>> }
impl Bigrams {
    /// `(prev, next, count)` triples; repeats are summed, and pairs with an empty or spaced word or a count <= 0 dropped.
    pub fn new<I: IntoIterator<Item = (String, String, f64)>>(it: I) -> Self {
        let mut m: HashMap<String, BTreeMap<String, f64>> = HashMap::new();
        for (p, n, f) in it {
            let bad = |w: &str| w.is_empty() || w.chars().any(char::is_whitespace);
            if bad(&p) || bad(&n) || f.is_nan() || f <= 0.0 { continue }
            *m.entry(p.to_ascii_lowercase()).or_default().entry(n.to_ascii_lowercase()).or_default() += f;
        }
        let next = m.into_iter().map(|(p, ns)| {
            let mut v: Vec<(String, f64)> = ns.into_iter().collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            (p, v)
        }).collect();
        Bigrams { next }
    }
    pub fn is_empty(&self) -> bool { self.next.is_empty() }
    /// Number of pairs.
    pub fn len(&self) -> usize { self.next.values().map(Vec::len).sum() }
    /// The words seen after `prev`, most frequent first.
    pub fn followers(&self, prev: &str) -> &[(String, f64)] { self.next.get(&prev.to_ascii_lowercase()).map_or(&[], Vec::as_slice) }
    /// Every pair, in no particular order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, f64)> { self.next.iter().flat_map(|(p, v)| v.iter().map(move |(n, f)| (p.as_str(), n.as_str(), *f))) }
}
/// Re-applies the typed prefix's Latin case to a lowercase completion: "HEL" -> "HELP", "Hel" -> "Help".
fn recase(w: &str, prefix: &str) -> String {
    if !prefix.starts_with(|c: char| c.is_ascii_uppercase()) { return w.to_string() }
    if prefix.len() >= 2 && !prefix.chars().any(|c| c.is_ascii_lowercase()) { return w.to_ascii_uppercase() }
    w[..1].to_ascii_uppercase() + &w[1..]
}
/// Thai text has no spaces: the char index where the last (incomplete) word of `run` starts. Longest tail first
/// (at most 16 chars, at least `min`) that the completer can extend and whose head is fully dictionary-covered.
pub fn thai_tail(run: &str, dict: &Dict, completer: &Completer, min: usize) -> Option<usize> {
    let at: Vec<usize> = run.char_indices().map(|(b, _)| b).collect(); let n = at.len();
    (min.max(1)..=n.min(16)).rev().map(|len| n - len)
        .find(|&i| completer.extends(&run[at[i]..]) && (i == 0 || dict.coverage_min(&run[..at[i]], 1) == 1.0))
}
