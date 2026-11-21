//! Spike: English real-word fixes ("form" typed for "from", "their" for "there") from a fixed list of confusion sets
//! and the English word pair counts already in the model (section 13) plus the English word frequencies (section 11).
//! Nothing here changes text: `Confusions::check` only says which word a typo-style row would offer, and the user
//! accepts it with a key press. See docs/research/confusion-pairs-spike.md.
//!
//! Score of a candidate `w` between `prev` and `next` (either may be missing):
//!   ln P(w | prev) + ln P(next | w), with P(b | a) = (c(a, b) + beta * Pu(b)) / (M(a) + beta)
//! where c is the stored pair count, M(a) the stored mass after `a`, and Pu the add-one unigram probability from the
//! frequency list. Without `prev`, P(w | prev) is Pu(w); without `next`, the right term is left out. A row is offered
//! when the best alternative beats the typed word by at least `margin` nats.
use crate::completer::Bigrams;
use std::collections::HashMap;

/// The tinyfix list (MIT, paperweight/tinyfix), with its attribution header.
pub const TINYFIX_LIST: &str = include_str!("../../../../data/tinyfix-confusions.txt");

#[derive(Clone, Debug, Default)]
pub struct Confusions { sets: HashMap<String, Vec<String>> }
impl Confusions {
    /// One set per line: the typed word, then the words it is mistaken for. `#` lines are comments.
    pub fn parse(text: &str) -> Self {
        let mut sets: HashMap<String, Vec<String>> = HashMap::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut w = line.split_whitespace().map(str::to_ascii_lowercase);
            let Some(typed) = w.next() else { continue };
            let e = sets.entry(typed.clone()).or_default();
            for a in w { if a != typed && !e.contains(&a) { e.push(a) } }
        }
        Confusions { sets }
    }
    pub fn tinyfix() -> Self { Self::parse(TINYFIX_LIST) }
    /// The words `typed` may have been meant as (lowercase), or none.
    pub fn alternatives(&self, typed: &str) -> &[String] { self.sets.get(&typed.to_ascii_lowercase()).map_or(&[], Vec::as_slice) }
    pub fn len(&self) -> usize { self.sets.len() }
    pub fn is_empty(&self) -> bool { self.sets.is_empty() }
    pub fn words(&self) -> impl Iterator<Item = (&str, &[String])> { self.sets.iter().map(|(k, v)| (k.as_str(), v.as_slice())) }
}

/// Bigram scorer over the model's English pairs and frequency list.
pub struct Scorer<'a> {
    pub bg: &'a Bigrams,
    freq: HashMap<String, f64>,
    total: f64,
    /// Pseudo-count of the unigram prior in each conditional.
    pub beta: f64,
}
impl<'a> Scorer<'a> {
    pub fn new(bg: &'a Bigrams, freq: &HashMap<String, f64>, beta: f64) -> Self {
        let mut f: HashMap<String, f64> = HashMap::new();
        for (w, c) in freq { if *c > 0.0 { *f.entry(w.to_ascii_lowercase()).or_default() += c } }
        let total = f.values().sum::<f64>() + f.len() as f64;
        Scorer { bg, freq: f, total, beta }
    }
    pub fn unigram(&self, w: &str) -> f64 { (self.freq.get(w).copied().unwrap_or(0.0) + 1.0) / self.total }
    /// The stored count of the pair `a b` (a scan of `a`'s followers).
    pub fn count(&self, a: &str, b: &str) -> f64 { self.bg.followers(a).find(|(n, _)| *n == b).map_or(0.0, |(_, c)| c) }
    pub fn cond(&self, a: &str, b: &str) -> f64 {
        (self.count(a, b) + self.beta * self.unigram(b)) / (self.bg.mass(a) + self.beta)
    }
    /// ln P(w | prev) + ln P(next | w), all lowercase.
    pub fn score(&self, prev: Option<&str>, w: &str, next: Option<&str>) -> f64 {
        let l = prev.map_or_else(|| self.unigram(w), |p| self.cond(p, w)).ln();
        l + next.map_or(0.0, |n| self.cond(w, n).ln())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix { pub word: String, pub gain: f64 }

impl Confusions {
    /// The row to offer for the finished word `typed` (any case), or none: the best alternative, when it beats the
    /// typed word by `margin`. `prev` / `next` are the neighbouring words in the same run (no punctuation between).
    /// The offered word takes the typed word's case.
    pub fn check(&self, sc: &Scorer, prev: Option<&str>, typed: &str, next: Option<&str>, margin: f64) -> Option<Fix> {
        let (best, gain) = self.best(sc, prev, typed, next)?;
        (gain >= margin).then(|| Fix { word: recase(best, typed), gain })
    }
    /// The best alternative to `typed` and its gain over it, whatever the margin (all words compared lowercase).
    /// Evidence guard: only words with stored followers (mass > 0 in section 13) are compared. A word without them
    /// (section 13 drops every word with an apostrophe; a word seen only at a run end or whose pairs were all pruned)
    /// would lose to any word that has them, so such a typed word gets no row and such alternatives are not offered.
    pub fn best<'s>(&'s self, sc: &Scorer, prev: Option<&str>, typed: &str, next: Option<&str>) -> Option<(&'s str, f64)> {
        self.best_with(sc, prev, typed, next, true)
    }
    /// `best`, with the evidence guard optional (`guard: false` is for the eval's ablation only).
    pub fn best_with<'s>(&'s self, sc: &Scorer, prev: Option<&str>, typed: &str, next: Option<&str>, guard: bool) -> Option<(&'s str, f64)> {
        let (p, t, n) = (prev.map(str::to_ascii_lowercase), typed.to_ascii_lowercase(), next.map(str::to_ascii_lowercase));
        let ok = |w: &str| !guard || sc.bg.mass(w) > 0.0;
        if !ok(&t) { return None }
        let base = sc.score(p.as_deref(), &t, n.as_deref());
        self.alternatives(&t).iter().filter(|a| ok(a))
            .map(|a| (a.as_str(), sc.score(p.as_deref(), a, n.as_deref()) - base)).max_by(|x, y| x.1.total_cmp(&y.1))
    }
}
fn recase(w: &str, typed: &str) -> String {
    if typed.len() > 1 && !typed.chars().any(|c| c.is_ascii_lowercase()) { return w.to_ascii_uppercase() }
    if typed.starts_with(|c: char| c.is_ascii_uppercase()) { return w[..1].to_ascii_uppercase() + &w[1..] }
    w.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_the_tinyfix_list() {
        let c = Confusions::tinyfix();
        assert!(c.len() > 200);
        assert_eq!(c.alternatives("Form"), ["from"]);
        assert_eq!(c.alternatives("their"), ["there", "they're"]);
        assert!(c.alternatives("hello").is_empty());
    }
    #[test]
    fn offers_the_word_the_pairs_prefer() {
        let bg = Bigrams::new([("letter", "form", 1.0), ("letter", "from", 40.0), ("from", "the", 500.0), ("form", "the", 5.0), ("the", "form", 30.0), ("the", "from", 0.5)]);
        let freq: HashMap<String, f64> = [("form", 100.0), ("from", 3000.0), ("the", 50000.0), ("letter", 200.0)].into_iter().map(|(w, f)| (w.to_string(), f)).collect();
        let sc = Scorer::new(&bg, &freq, 10.0);
        let c = Confusions::tinyfix();
        assert_eq!(c.check(&sc, Some("letter"), "form", Some("the"), 1.0).map(|f| f.word), Some("from".into()));
        assert_eq!(c.check(&sc, Some("the"), "form", None, 1.0), None);
        assert_eq!(c.check(&sc, Some("letter"), "Form", None, 1.0).map(|f| f.word), Some("From".into()));
    }
}
