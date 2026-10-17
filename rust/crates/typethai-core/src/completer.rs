//! Prefix completion over a frequency-ranked word list, read through the slips a typist makes.
use crate::{dict::Dict, ngram::Ngram, slip::{Slip, Slips}};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;

/// Sorted by word (byte order), so the proper extensions of any prefix are one contiguous run right after it.
pub struct Completer {
    words: Vec<(String, f64)>,
    /// `cum[i]`: the frequency of `words[..i]`, so a prefix's share of the corpus is one subtraction.
    cum: Vec<f64>,
    trie: Trie,
}
/// `confidence`: the top word's share of the frequency of all extensions plus the prefix itself when it is a word, so
/// a finished word (`you`, `ฉัน`) is not read as a confident prefix of its extensions. With slips, the share is of
/// everything every reading of the keys can become, each reading weighted by how likely it is.
#[derive(Clone, Debug, Default, PartialEq)] pub struct Completions { pub words: Vec<String>, pub confidence: f64 }

impl Completer {
    pub fn new<I: IntoIterator<Item = (String, f64)>>(it: I) -> Self {
        let mut m: BTreeMap<String, f64> = BTreeMap::new();
        for (w, f) in it {
            if w.is_empty() || f.is_nan() || f <= 0.0 || w.chars().any(char::is_whitespace) { continue }
            *m.entry(w.to_ascii_lowercase()).or_default() += f;
        }
        let words: Vec<(String, f64)> = m.into_iter().collect();
        let mut cum: Vec<f64> = std::iter::once(0.0).chain(words.iter().scan(0.0, |a, (_, f)| { *a += f; Some(*a) })).collect();
        cum.shrink_to_fit();
        Completer { trie: Trie::new(&words), words, cum }
    }
    pub fn is_empty(&self) -> bool { self.words.is_empty() }
    fn start(&self, prefix: &str) -> usize { self.words.partition_point(|(w, _)| w.as_str() <= prefix) }
    /// The words that start with `prefix`, itself first when it is one.
    fn span(&self, prefix: &str) -> (usize, usize) {
        (self.words.partition_point(|(w, _)| w.as_str() < prefix), self.words.partition_point(|(w, _)| w.as_str() < prefix || w.starts_with(prefix)))
    }
    pub fn has(&self, word: &str) -> bool { let w = word.to_ascii_lowercase(); self.words.binary_search_by(|(x, _)| x.as_str().cmp(&w)).is_ok() }
    pub fn extends(&self, prefix: &str) -> bool {
        let p = prefix.to_ascii_lowercase();
        self.words.get(self.start(&p)).is_some_and(|(w, _)| w.starts_with(&p))
    }
    /// A word extends `prefix` read with one slip; never under `SLIP_MIN` chars.
    pub fn extends_slipped(&self, prefix: &str, slips: &Slips) -> bool { !self.slipped(&prefix.to_ascii_lowercase(), slips).is_empty() }
    /// `w` is in the list.
    pub fn knows(&self, w: &str) -> bool { self.freq(&w.to_ascii_lowercase()).is_some() }
    fn freq(&self, w: &str) -> Option<f64> { self.words.binary_search_by(|(x, _)| x.as_str().cmp(w)).ok().map(|i| self.words[i].1) }
    /// The prefixes one slip from `typed` that some word starts with, likeliest (by corpus share × slip chance)
    /// first, at most `READINGS`. Nothing under `SLIP_MIN` chars: one slip in two keys can mean anything.
    fn slipped(&self, typed: &str, s: &Slips) -> Vec<Reading> {
        let t: Vec<char> = typed.chars().collect();
        if t.len() < SLIP_MIN { return vec![] }
        let mut out = vec![];
        self.trie.walk(&t, 0, 0, &mut String::new(), None, &mut |node, q, slip| out.push(Reading { q: q.to_string(), p: s.p(slip), span: self.trie.span(node) }));
        // one prefix can be reached by several slips; the likeliest one counts
        out.sort_by(|a, b| a.q.cmp(&b.q).then(b.p.total_cmp(&a.p)));
        out.dedup_by(|b, a| a.q == b.q);
        let prior = |r: &Reading| (self.cum[r.span.1] - self.cum[r.span.0]) * r.p;
        out.sort_by(|a, b| prior(b).total_cmp(&prior(a)).then_with(|| a.q.cmp(&b.q)));
        out.truncate(READINGS);
        out
    }
    /// The best `k` words that `prefix` may be the start of, ranked by the corpus frequencies and whatever `cx` adds.
    /// Without `cx.slips` the keys are read as typed, and the words are its extensions.
    pub fn complete<'a>(&'a self, prefix: &str, k: usize, cx: Context<'a>) -> Completions {
        let p = prefix.to_ascii_lowercase();
        let n = CANDIDATES.max(k);
        let mut reads = vec![Reading { span: self.span(&p), q: p.clone(), p: 1.0 }];
        if let Some(s) = &cx.slips { reads.extend(self.slipped(&p, s)) }
        let mut spreads: Vec<Spread> = reads.into_iter().enumerate().map(|(i, r)| self.spread(r, &p, if i == 0 { n } else { SLIP_CANDIDATES.max(k) })).collect();
        if let (Some(prev), Some(b)) = (cx.prev, cx.bigrams) {
            // best first, so the first matches are the ones that can rank; a reading's words are one run of ids
            let ids: Vec<std::ops::Range<u32>> = spreads.iter().map(|sp| b.starting(&sp.read.q)).collect();
            for (id, w, c) in b.ids_of(b.find(prev)).take(FOLLOWER_SCAN) {
                for (sp, r) in spreads.iter_mut().zip(&ids) { if sp.bi < CANDIDATES && r.contains(&id) { sp.bi += 1; sp.add(self, BI, w, c, &p, sp.bi) } }
            }
        }
        if let Some(m) = cx.memory {
            // a word the corpus does not know may be a typo until it has been finished a few times
            let known = |w: &str| self.freq(w).is_some() || m.count(w) >= NEW_WORD_MIN;
            for sp in &mut spreads {
                let q = sp.read.q.clone();
                for (i, (w, c)) in m.with_prefix(&q).filter(|(w, _)| known(w)).enumerate() { sp.add(self, MINE, w, c, &p, i + 1) }
                if let Some(prev) = cx.prev { for (i, (w, c)) in m.after(prev, &q).filter(|(w, _)| known(w)).enumerate() { sp.add(self, MINE_PAIR, w, c, &p, i + 1) } }
            }
        }
        // each reading's chance: its words' share of all the signals can say, times the chance of its slip
        let mut whole = [0.0; SIGNALS];
        whole[UNI] = self.cum[self.words.len()];
        if let (Some(prev), Some(b)) = (cx.prev, cx.bigrams) { whole[BI] = b.mass(prev) }
        if let Some(m) = cx.memory {
            whole[MINE] = m.mass();
            if let Some(prev) = cx.prev { whole[MINE_PAIR] = m.count(prev) }
        }
        for sp in &spreads { for i in 0..SIGNALS { whole[i] = whole[i].max(sp.total[i]) } }
        let lambda: [f64; SIGNALS] = std::array::from_fn(|i| if whole[i] > 0.0 { WEIGHT[i] * trust(i, whole[i]) } else { 0.0 });
        let chance: Vec<f64> = spreads.iter().map(|sp| sp.read.p * (0..SIGNALS).filter(|&i| lambda[i] > 0.0).map(|i| lambda[i] * sp.total[i] / whole[i]).sum::<f64>()).collect();
        // the keys as typed may also start a word the list lacks: a reading with no row that takes its share. Only the
        // corpus signal can say how often such words come; the pairs and the user's words know none of them.
        let lacked = match (cx.unknown, cx.slips.is_some() && p.chars().count() >= SLIP_MIN) {
            (Some(ng), true) => lambda[UNI] * UNKNOWN * ng.log_prob_start(&p).exp(),
            _ => 0.0,
        };
        let all: f64 = chance.iter().sum::<f64>() + lacked;
        if all == 0.0 { return Completions::default() }
        // (word, score, the keys as typed reach it)
        let mut scored: Vec<(&str, f64, bool)> = spreads.iter().zip(&chance).enumerate().filter(|(_, (_, c))| **c > 0.0)
            .flat_map(|(i, (sp, c))| { let w = c / all; sp.shares().into_iter().map(move |(x, s)| (x, w * s, i == 0)) }).collect();
        // a word several readings reach gets each one's part
        scored.sort_by(|a, b| a.0.cmp(b.0));
        scored.dedup_by(|b, a| if a.0 == b.0 { a.1 += b.1; a.2 |= b.2; true } else { false });
        scored.retain(|x| x.2 || x.1 >= SLIP_FLOOR);
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let confidence = scored.first().map_or(0.0, |r| r.1);
        Completions { words: scored.into_iter().take(k).map(|(w, _, _)| recase(w, prefix)).collect(), confidence }
    }
    /// The words a finished `typed` may have meant, best first: `others` (another checker's guesses, which may be more
    /// than one slip away) and the words one slip away, by P(word | cx) × P(typed | word). Guesses this list does not
    /// know follow in their own order. `typed` itself is never one.
    pub fn correct(&self, typed: &str, k: usize, cx: Context, others: &[String]) -> Vec<String> {
        let t = typed.to_ascii_lowercase();
        let s = cx.slips.unwrap_or_default();
        let chars: Vec<char> = t.chars().collect();
        // the guesses first, so their spelling of a word (a capital) is kept
        let mut words: Vec<String> = others.to_vec();
        self.trie.walk(&chars, 0, 0, &mut String::new(), None, &mut |n, q, _| if self.words[self.trie.span(n).0].0 == q { words.push(q.to_string()) });
        let mut seen = std::collections::HashSet::new();
        words.retain(|w| !w.eq_ignore_ascii_case(&t) && seen.insert(w.to_ascii_lowercase()));
        let known = self.chances(&words, cx);
        let mut ranked: Vec<(f64, usize)> = words.iter().zip(&known).enumerate().map(|(i, (w, p))| (p * s.chance(&t, &w.to_ascii_lowercase()), i)).collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        ranked.into_iter().take(k).map(|(_, i)| if self.freq(&words[i]).is_some() { recase(&words[i], typed) } else { words[i].clone() }).collect()
    }
    /// P(word | cx) for each of `words`, with the signals weighed as `complete` weighs its readings (0 for a word no
    /// signal knows).
    fn chances(&self, words: &[String], cx: Context) -> Vec<f64> {
        let lower: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
        let mut n = vec![[0.0; SIGNALS]; words.len()];
        let mut whole = [0.0; SIGNALS];
        whole[UNI] = self.cum[self.words.len()];
        for (i, w) in lower.iter().enumerate() { n[i][UNI] = self.freq(w).unwrap_or(0.0) }
        if let (Some(prev), Some(b)) = (cx.prev, cx.bigrams) {
            whole[BI] = b.mass(prev);
            for (w, c) in b.followers(prev).take(FOLLOWER_SCAN) { for (i, x) in lower.iter().enumerate() { if x == w { n[i][BI] = c } } }
        }
        if let Some(m) = cx.memory {
            whole[MINE] = m.mass();
            for (i, w) in lower.iter().enumerate() { if n[i][UNI] > 0.0 || m.count(w) >= NEW_WORD_MIN { n[i][MINE] = m.count(w) } }
            if let Some(prev) = cx.prev {
                whole[MINE_PAIR] = m.count(prev);
                for (w, c) in m.after(prev, "") { for (i, x) in lower.iter().enumerate() { if x == w && n[i][MINE] > 0.0 { n[i][MINE_PAIR] = c } } }
            }
        }
        for v in &n { for i in 0..SIGNALS { whole[i] = whole[i].max(v[i]) } }
        let lambda: [f64; SIGNALS] = std::array::from_fn(|i| if whole[i] > 0.0 { WEIGHT[i] * trust(i, whole[i]) } else { 0.0 });
        let sum: f64 = lambda.iter().sum();
        n.iter().map(|v| if sum == 0.0 { 0.0 } else { (0..SIGNALS).filter(|&i| lambda[i] > 0.0).map(|i| lambda[i] * v[i] / whole[i]).sum::<f64>() / sum }).collect()
    }
    /// The corpus words of a reading, the best `n` by frequency as candidates; `typed` itself is never one.
    fn spread(&self, read: Reading, typed: &str, n: usize) -> Spread<'_> {
        let mut sp = Spread { total: [0.0; SIGNALS], cands: vec![], bi: 0, cap: n, read };
        for (w, f) in &self.words[sp.read.span.0..sp.read.span.1] {
            sp.total[UNI] += f;
            if w == typed { continue }
            // word-ascending, so inserting after equal freqs keeps ties in word order
            let at = sp.cands.partition_point(|c| c.n[UNI] >= *f);
            if at < n { sp.cands.insert(at, Cand { word: w, n: [*f, 0.0, 0.0, 0.0] }); sp.cands.truncate(n); }
        }
        sp
    }
}
/// A prefix the typed keys may have meant, the chance of the slip that typed it instead (1: as typed), and its words.
struct Reading { q: String, p: f64, span: (usize, usize) }
/// What one reading can become, per signal: the weight of all its words (the prefix itself included), and the best
/// (at most `cap` from each signal).
struct Spread<'a> { read: Reading, total: [f64; SIGNALS], cands: Vec<Cand<'a>>, bi: usize, cap: usize }
impl<'a> Spread<'a> {
    /// The `nth` word of a signal (1-based, best first) for this reading.
    fn add(&mut self, c: &'a Completer, sig: usize, w: &'a str, n: f64, typed: &str, nth: usize) {
        self.total[sig] += n;
        if w == typed || nth > self.cap { return }
        match self.cands.iter_mut().find(|x| x.word == w) {
            Some(x) => x.n[sig] += n,
            None => { let mut v = [0.0; SIGNALS]; v[UNI] = c.freq(w).unwrap_or(0.0); v[sig] = n; self.cands.push(Cand { word: w, n: v }) }
        }
    }
    /// Each candidate's share of everything the reading can become: per signal a distribution over its words, and so
    /// is their weighted mean.
    fn shares(&self) -> Vec<(&'a str, f64)> {
        let weight: [f64; SIGNALS] = std::array::from_fn(|i| if self.total[i] > 0.0 { WEIGHT[i] * trust(i, self.total[i]) } else { 0.0 });
        let sum: f64 = weight.iter().sum();
        if sum == 0.0 { return vec![] }
        self.cands.iter().map(|c| (c.word, (0..SIGNALS).filter(|&i| weight[i] > 0.0).map(|i| weight[i] * c.n[i] / self.total[i]).sum::<f64>() / sum)).collect()
    }
}
/// The word list's prefixes as a tree, to walk typed keys with a slip. Node `n` is a prefix whose words (the prefix
/// itself first, when it is one) are `words[span[n].0..span[n].1]`; its children by next char are `kids[at[n].0..at[n].1]`.
struct Trie { span: Vec<(u32, u32)>, at: Vec<(u32, u32)>, kids: Vec<(char, u32)> }
impl Trie {
    fn new(words: &[(String, f64)]) -> Self {
        let mut t = Trie { span: vec![], at: vec![], kids: vec![] };
        t.node(words, 0, words.len(), 0);
        t.span.shrink_to_fit(); t.at.shrink_to_fit(); t.kids.shrink_to_fit();
        t
    }
    /// Adds the node of `words[lo..hi]`, which share their first `d` bytes, and its subtree.
    fn node(&mut self, words: &[(String, f64)], lo: usize, hi: usize, d: usize) -> u32 {
        let id = self.span.len();
        self.span.push((lo as u32, hi as u32)); self.at.push((0, 0));
        let mut i = lo + usize::from(words.get(lo).is_some_and(|(w, _)| w.len() == d) && lo < hi);
        let mut kids = vec![];
        while i < hi {
            let c = words[i].0[d..].chars().next().unwrap();
            let end = i + words[i..hi].partition_point(|(w, _)| w[d..].starts_with(c));
            kids.push((c, i, end)); i = end;
        }
        let k0 = self.kids.len();
        self.kids.extend(kids.iter().map(|&(c, _, _)| (c, 0)));
        self.at[id] = (k0 as u32, self.kids.len() as u32);
        for (j, &(c, a, b)) in kids.iter().enumerate() { let n = self.node(words, a, b, d + c.len_utf8()); self.kids[k0 + j].1 = n; }
        id as u32
    }
    fn span(&self, n: u32) -> (usize, usize) { let s = self.span[n as usize]; (s.0 as usize, s.1 as usize) }
    fn kids(&self, n: u32) -> &[(char, u32)] { let a = self.at[n as usize]; &self.kids[a.0 as usize..a.1 as usize] }
    fn child(&self, n: u32, c: char) -> Option<u32> { let k = self.kids(n); k.binary_search_by_key(&c, |x| x.0).ok().map(|i| k[i].1) }
    /// Reads `t[i..]` from node `n` (prefix `q`), with at most one slip in all; `found` gets each node reached at the
    /// end with the slip that reached it. A key left out at the very end is not a slip: that is the word going on.
    fn walk(&self, t: &[char], n: u32, i: usize, q: &mut String, slip: Option<Slip>, found: &mut impl FnMut(u32, &str, Slip)) {
        if i == t.len() { if let Some(s) = slip { found(n, q, s) } return }
        if let Some(c) = self.child(n, t[i]) { q.push(t[i]); self.walk(t, c, i + 1, q, slip, found); q.pop(); }
        if slip.is_some() { return }
        self.walk(t, n, i + 1, q, Some(Slip::extra(t[i], i.checked_sub(1).map(|j| t[j]), t.get(i + 1).copied())), found);
        if let Some(&next) = t.get(i + 1).filter(|&&x| x != t[i]) {
            if let Some(c) = self.child(n, next).and_then(|a| self.child(a, t[i])) { q.push(next); q.push(t[i]); self.walk(t, c, i + 2, q, Some(Slip::Swap), found); q.pop(); q.pop(); }
        }
        for &(c, kid) in self.kids(n) {
            q.push(c);
            if c != t[i] { self.walk(t, kid, i + 1, q, Some(Slip::instead(c, t[i])), found) }
            self.walk(t, kid, i, q, Some(Slip::Drop), found);
            q.pop();
        }
    }
}
/// What a completion is ranked by besides the corpus word frequencies. `Context::default()` ranks by those alone.
#[derive(Clone, Copy, Debug, Default)]
pub struct Context<'a> {
    /// The word before the one being completed, as it is on screen.
    pub prev: Option<&'a str>,
    pub bigrams: Option<&'a Bigrams>,
    pub memory: Option<&'a WordMemory>,
    /// How the keys may differ from the word meant; None reads them as typed.
    pub slips: Option<Slips>,
    /// How words the list lacks are spelled (a character model): with slips, the keys may start one of them, read as
    /// typed. None: every word meant is in the list.
    pub unknown: Option<&'a Ngram>,
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
/// Share of the words typed that the list lacks (names, rare words): about one in ten words of 4+ letters in held-out
/// English, one in thirty in Thai.
const UNKNOWN: f64 = 0.05;
/// Keys typed before a slip is read into them.
pub const SLIP_MIN: usize = 3;
/// Least share of a word only a slip reaches: below it the keys were most likely typed as meant, and the word would
/// only take a row from the spell checker's completions.
const SLIP_FLOOR: f64 = 0.01;
/// Candidates a slipped reading takes from each signal: it holds a small share, so only its best words can rank.
const SLIP_CANDIDATES: usize = 8;
/// Slipped readings of the keys kept besides the keys as typed; the rest are too unlikely to reach the top rows.
const READINGS: usize = 8;
/// Extensions taken from each list (corpus words, bigram followers); a word below them cannot reach the top rows.
const CANDIDATES: usize = 32;
/// Followers of the previous word looked at, best first. The rest are rare after it, and a common word can have tens of
/// thousands: scanning them all for a prefix few match costs more than a keystroke may.
const FOLLOWER_SCAN: usize = 1000;
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
    /// Every finish ever learned, decayed as the counts are (the dropped words too): what a `count` is a share of.
    pub fn mass(&self) -> f64 { let r = 0.5f64.powf(1.0 / HALF_LIFE); (1.0 - r.powf(self.clock as f64)) / (1.0 - r) }
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
/// Compact: every distinct word is stored once in `text`, and a pair is a word id and a count, so the half a million
/// pairs of the shipped model take a few MB instead of the ~50 MB of a string per pair.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bigrams {
    /// The distinct words, sorted and concatenated; word `i` is `text[ends[i - 1]..ends[i]]`.
    text: String, ends: Vec<u32>,
    /// The followers of word `i` are `pairs[start[i]..start[i + 1]]`.
    start: Vec<u32>, pairs: Vec<(u32, f32)>,
    /// The count of all pairs after word `i`.
    after: Vec<f32>,
}
impl Bigrams {
    /// `(prev, next, count)` triples; repeats are summed, and pairs with an empty or spaced word or a count <= 0 dropped.
    pub fn new<S: AsRef<str>, I: IntoIterator<Item = (S, S, f64)>>(it: I) -> Self {
        let mut ids: HashMap<String, u32> = HashMap::new();
        let mut id = |w: &str| -> u32 {
            let w = w.to_ascii_lowercase();
            if let Some(&i) = ids.get(&w) { return i }
            let i = ids.len() as u32; ids.insert(w, i); i
        };
        let bad = |w: &str| w.is_empty() || w.chars().any(char::is_whitespace);
        let mut raw: Vec<(u32, u32, f64)> = Vec::new();
        for (p, n, f) in it {
            let (p, n) = (p.as_ref(), n.as_ref());
            if bad(p) || bad(n) || f.is_nan() || f <= 0.0 { continue }
            raw.push((id(p), id(n), f));
        }
        // renumber the words in sorted order, so a word is found by binary search and ties list in word order
        let mut words: Vec<(String, u32)> = ids.into_iter().collect();
        words.sort_unstable();
        let mut to = vec![0u32; words.len()];
        for (i, (_, old)) in words.iter().enumerate() { to[*old as usize] = i as u32; }
        for t in &mut raw { (t.0, t.1) = (to[t.0 as usize], to[t.1 as usize]); }
        raw.sort_unstable_by_key(|t| (t.0, t.1));
        let mut summed: Vec<(u32, u32, f64)> = Vec::with_capacity(raw.len());
        for t in raw { match summed.last_mut() { Some(l) if (l.0, l.1) == (t.0, t.1) => l.2 += t.2, _ => summed.push(t) } }
        summed.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.total_cmp(&a.2)).then(a.1.cmp(&b.1)));
        let (mut text, mut ends) = (String::new(), Vec::with_capacity(words.len()));
        for (w, _) in &words { text.push_str(w); ends.push(text.len() as u32); }
        let mut start = vec![0u32; words.len() + 1];
        for t in &summed { start[t.0 as usize + 1] += 1; }
        for i in 0..words.len() { start[i + 1] += start[i]; }
        let mut after = vec![0.0f32; words.len()];
        for t in &summed { after[t.0 as usize] += t.2 as f32 }
        let pairs = summed.into_iter().map(|(_, n, f)| (n, f as f32)).collect();
        Bigrams { text, ends, start, pairs, after }
    }
    pub fn is_empty(&self) -> bool { self.pairs.is_empty() }
    /// Number of pairs.
    pub fn len(&self) -> usize { self.pairs.len() }
    fn word(&self, i: u32) -> &str {
        let i = i as usize;
        &self.text[if i == 0 { 0 } else { self.ends[i - 1] as usize }..self.ends[i] as usize]
    }
    fn find(&self, w: &str) -> Option<u32> {
        let w = w.to_ascii_lowercase();
        let (mut lo, mut hi) = (0, self.ends.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            match self.word(mid as u32).cmp(&w) { std::cmp::Ordering::Less => lo = mid + 1, std::cmp::Ordering::Greater => hi = mid, std::cmp::Ordering::Equal => return Some(mid as u32) }
        }
        None
    }
    /// The followers of word `i`; none for `None`.
    fn of(&self, i: Option<u32>) -> impl ExactSizeIterator<Item = (&str, f64)> { self.ids_of(i).map(|(_, w, f)| (w, f)) }
    /// The followers of word `i` with their ids.
    fn ids_of(&self, i: Option<u32>) -> impl ExactSizeIterator<Item = (u32, &str, f64)> {
        let r = i.map_or(0..0, |i| self.start[i as usize] as usize..self.start[i as usize + 1] as usize);
        self.pairs[r].iter().map(move |&(n, f)| (n, self.word(n), f as f64))
    }
    /// The ids of the words that start with `prefix`: ids are in word order, so they are one run.
    fn starting(&self, prefix: &str) -> std::ops::Range<u32> {
        let first = |past: &dyn Fn(&str) -> bool| { let (mut lo, mut hi) = (0u32, self.ends.len() as u32); while lo < hi { let mid = (lo + hi) / 2; if past(self.word(mid)) { lo = mid + 1 } else { hi = mid } } lo };
        first(&|w| w < prefix)..first(&|w| w < prefix || w.starts_with(prefix))
    }
    /// The words seen after `prev`, most frequent first.
    pub fn followers(&self, prev: &str) -> impl ExactSizeIterator<Item = (&str, f64)> { self.of(self.find(prev)) }
    /// The count of all pairs after `prev`.
    pub fn mass(&self, prev: &str) -> f64 { self.find(prev).map_or(0.0, |i| self.after[i as usize] as f64) }
    /// Every pair, grouped by previous word.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, f64)> {
        (0..self.ends.len() as u32).flat_map(move |p| self.of(Some(p)).map(move |(n, f)| (self.word(p), n, f)))
    }
}
/// Re-applies the typed prefix's Latin case to a lowercase completion: "HEL" -> "HELP", "Hel" -> "Help".
fn recase(w: &str, prefix: &str) -> String {
    if !prefix.starts_with(|c: char| c.is_ascii_uppercase()) { return w.to_string() }
    if prefix.len() >= 2 && !prefix.chars().any(|c| c.is_ascii_lowercase()) { return w.to_ascii_uppercase() }
    w[..1].to_ascii_uppercase() + &w[1..]
}
/// Thai text has no spaces: the char index where the last (incomplete) word of `run` starts. Longest tail first
/// (at most 16 chars, at least `min`) that the completer can extend and whose head is fully dictionary-covered; when
/// none can, and `slips` is given, the longest that it can extend read with one slip.
pub fn thai_tail(run: &str, dict: &Dict, completer: &Completer, min: usize, slips: Option<&Slips>) -> Option<usize> {
    let at: Vec<usize> = run.char_indices().map(|(b, _)| b).collect(); let n = at.len();
    let tails = || (min.max(1)..=n.min(16)).rev().map(|len| n - len);
    let head_ok = |i: usize| i == 0 || dict.coverage_min(&run[..at[i]], 1) == 1.0;
    tails().find(|&i| completer.extends(&run[at[i]..]) && head_ok(i))
        .or_else(|| { let s = slips?; tails().find(|&i| completer.extends_slipped(&run[at[i]..], s) && head_ok(i)) })
}
