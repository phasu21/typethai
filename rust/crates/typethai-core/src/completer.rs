//! Prefix completion over a frequency-ranked word list.
use crate::dict::Dict;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

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
    fn freq(&self, w: &str) -> Option<f64> { self.words.binary_search_by(|(x, _)| x.as_str().cmp(w)).ok().map(|i| self.words[i].1) }
    /// The best `k` words that extend `prefix`, ranked by the corpus frequencies and whatever `cx` adds.
    pub fn complete<'a>(&'a self, prefix: &str, k: usize, cx: Context<'a>) -> Completions {
        let p = prefix.to_ascii_lowercase();
        let n = CANDIDATES.max(k);
        // per signal, the weight of every word the prefix can become, the prefix itself included
        let mut total = [0.0; SIGNALS];
        total[UNI] = self.freq(&p).unwrap_or(0.0);
        let mut cands: Vec<Cand> = Vec::new();
        for (w, f) in self.range(&p) {
            total[UNI] += f;
            // `range` is word-ascending, so inserting after equal freqs keeps ties in word order.
            let at = cands.partition_point(|c| c.n[UNI] >= *f);
            if at < n { cands.insert(at, Cand { word: w, n: [*f, 0.0, 0.0, 0.0] }); cands.truncate(n); }
        }
        let mut add = |sig: usize, w: &'a str, c: f64, cands: &mut Vec<Cand<'a>>| {
            total[sig] += c;
            if w == p { return }
            match cands.iter_mut().find(|x| x.word == w) {
                Some(x) => x.n[sig] += c,
                None => { let mut n = [0.0; SIGNALS]; n[UNI] = self.freq(w).unwrap_or(0.0); n[sig] = c; cands.push(Cand { word: w, n }) }
            }
        };
        if let (Some(prev), Some(b)) = (cx.prev, cx.bigrams) {
            // best first, so the first matches are the ones that can rank
            for (w, c) in b.followers(prev).iter().filter(|(w, _)| w.starts_with(&p)).take(CANDIDATES) { add(BI, w, *c, &mut cands) }
        }
        if let Some(m) = cx.memory {
            // a word the corpus does not know may be a typo until it has been finished a few times
            let known = |w: &str| self.freq(w).is_some() || m.count(w) >= NEW_WORD_MIN;
            for (w, c) in m.with_prefix(&p) { if known(w) { add(MINE, w, c, &mut cands) } }
            if let Some(prev) = cx.prev { for (w, c) in m.after(prev, &p) { if known(w) { add(MINE_PAIR, w, c, &mut cands) } } }
        }
        let weight: [f64; SIGNALS] = std::array::from_fn(|i| if total[i] > 0.0 { WEIGHT[i] * trust(i, total[i]) } else { 0.0 });
        let sum: f64 = weight.iter().sum();
        if sum == 0.0 { return Completions::default() }
        // each signal is a distribution over every word the prefix can become (the prefix itself included), and so is
        // their weighted mean: a word's score is already its share
        let score = |n: &[f64; SIGNALS]| (0..SIGNALS).filter(|&i| weight[i] > 0.0).map(|i| weight[i] * n[i] / total[i]).sum::<f64>() / sum;
        let mut ranked: Vec<(f64, &str)> = cands.iter().map(|c| (score(&c.n), c.word)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(b.1)));
        let confidence = ranked.first().map_or(0.0, |r| r.0);
        Completions { words: ranked.into_iter().take(k).map(|(_, w)| recase(w, prefix)).collect(), confidence }
    }
}
/// What a completion is ranked by besides the corpus word frequencies. `Context::default()` ranks by those alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    /// The word before the one being completed, as it is on screen.
    pub prev: Option<&'a str>,
    pub bigrams: Option<&'a Bigrams>,
    pub memory: Option<&'a WordMemory>,
}
struct Cand<'a> { word: &'a str, n: [f64; SIGNALS] }
/// The ranking signals: corpus unigram, corpus bigram after `prev`, the user's words, the user's words after `prev`.
const SIGNALS: usize = 4;
const UNI: usize = 0; const BI: usize = 1; const MINE: usize = 2; const MINE_PAIR: usize = 3;
/// Interpolation weights by signal. A word seen after this very `prev` says more than its overall frequency (2:1, the
/// usual bigram:unigram split of an interpolated model). What this user types predicts them better than any corpus,
/// so their own words weigh 2 and their own pairs 3, scaled by `trust`.
const WEIGHT: [f64; SIGNALS] = [1.0, 2.0, 2.0, 3.0];
/// The user's counts reach half their weight at this many finished words: one finish is a hint, not a habit.
const TRUST_HALF: f64 = 4.0;
fn trust(sig: usize, total: f64) -> f64 { if sig == MINE || sig == MINE_PAIR { total / (total + TRUST_HALF) } else { 1.0 } }
/// Extensions taken from each list (corpus words, bigram followers); a word below them cannot reach the top rows.
const CANDIDATES: usize = 32;
/// Finishes before a word the corpus does not know is offered. Three recent finishes decay to just under 3.
pub const NEW_WORD_MIN: f64 = 2.5;

/// Words the user finished, and each with the word before it: counts that decay with every word learned since, so
/// recent habits outrank old ones. Bounded: past `WORD_CAP` words or `PAIR_CAP` pairs the weakest go.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WordMemory { clock: u64, words: BTreeMap<String, Seen>, pairs: BTreeMap<String, BTreeMap<String, Seen>> }
/// A count as of word `t` of the memory's clock.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Seen { n: f64, t: u64 }
/// Words learned until a count halves: about two weeks of steady typing, so a word used weekly stays.
const HALF_LIFE: f64 = 50_000.0;
pub const WORD_CAP: usize = 4000;
pub const PAIR_CAP: usize = 8000;
impl Seen {
    fn at(&self, now: u64) -> f64 { self.n * 0.5f64.powf(now.saturating_sub(self.t) as f64 / HALF_LIFE) }
    fn bump(&mut self, now: u64) { self.n = self.at(now) + 1.0; self.t = now; }
}
impl WordMemory {
    /// One finished `word`, after `prev` when there was one. Lowercased; an empty or spaced word is ignored.
    pub fn learn(&mut self, prev: Option<&str>, word: &str) {
        let clean = |w: &str| Some(w.to_ascii_lowercase()).filter(|w| !w.is_empty() && !w.chars().any(char::is_whitespace));
        let Some(w) = clean(word) else { return };
        self.clock += 1; let now = self.clock;
        let fresh = Seen { n: 0.0, t: now };
        if let Some(p) = prev.and_then(clean) { self.pairs.entry(p).or_default().entry(w.clone()).or_insert(fresh).bump(now) }
        self.words.entry(w).or_insert(fresh).bump(now);
        self.prune();
    }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }
    /// Distinct words remembered.
    pub fn len(&self) -> usize { self.words.len() }
    /// How many times `word` was finished, decayed.
    pub fn count(&self, word: &str) -> f64 { self.words.get(&word.to_ascii_lowercase()).map_or(0.0, |s| s.at(self.clock)) }
    fn with_prefix<'a: 'p, 'p>(&'a self, p: &'p str) -> impl Iterator<Item = (&'a str, f64)> + 'p { starting(&self.words, p, self.clock) }
    fn after<'a: 'p, 'p>(&'a self, prev: &str, p: &'p str) -> impl Iterator<Item = (&'a str, f64)> + 'p {
        self.pairs.get(&prev.to_ascii_lowercase()).into_iter().flat_map(move |m| starting(m, p, self.clock))
    }
    /// Over a cap, drop the weakest down to 90% of it, so pruning is rare.
    fn prune(&mut self) {
        let now = self.clock;
        if self.words.len() > WORD_CAP {
            let floor = nth_weakest(self.words.values().map(|s| s.at(now)), self.words.len() - WORD_CAP * 9 / 10);
            self.words.retain(|_, s| s.at(now) > floor);
        }
        if self.pairs.values().map(BTreeMap::len).sum::<usize>() > PAIR_CAP {
            let n = self.pairs.values().map(BTreeMap::len).sum::<usize>() - PAIR_CAP * 9 / 10;
            let floor = nth_weakest(self.pairs.values().flat_map(|m| m.values()).map(|s| s.at(now)), n);
            for m in self.pairs.values_mut() { m.retain(|_, s| s.at(now) > floor) }
            self.pairs.retain(|_, m| !m.is_empty());
        }
    }
    pub fn to_json(&self) -> String { serde_json::to_string(self).unwrap() }
    pub fn from_json(s: &str) -> Result<Self, String> { serde_json::from_str(s).map_err(|e| e.to_string()) }
}
/// The entries of `m` whose word starts with `p`, with their counts as of `now`.
fn starting<'a: 'p, 'p>(m: &'a BTreeMap<String, Seen>, p: &'p str, now: u64) -> impl Iterator<Item = (&'a str, f64)> + 'p {
    m.range::<str, _>((Bound::Included(p), Bound::Unbounded)).take_while(move |(w, _)| w.starts_with(p)).map(move |(w, s)| (w.as_str(), s.at(now)))
}
/// The `n`th smallest value (1-based): dropping everything at or below it drops at least `n` entries.
fn nth_weakest(it: impl Iterator<Item = f64>, n: usize) -> f64 {
    let mut v: Vec<f64> = it.collect();
    v.sort_by(f64::total_cmp);
    v[n.clamp(1, v.len()) - 1]
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
