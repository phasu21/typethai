//! English real-word fixes ("form" typed for "from", "their" for "there", "your" for "you're") from a fixed list of
//! confusion sets, scored with the English word pair counts in the model (section 13) and the English word frequencies
//! (section 11). Nothing here changes text: `RealWords::check` only says which word a typo-style row would offer, and
//! the user accepts it with a key press. See docs/research/confusion-pairs-spike.md.
//!
//! Score of a candidate `w` between `prev` and `next` (either may be missing):
//!   ln P(w | prev) + ln P(next | w), with P(b | a) = (c(a, b) + beta * Pu(b)) / (M(a) + beta)
//! where c is the stored pair count, M(a) the stored mass after `a`, and Pu the add-one unigram probability from the
//! frequency list. Without `prev`, P(w | prev) is Pu(w); without `next`, the right term is left out. A row is offered
//! when the best alternative beats the typed word by at least the margin (in nats) of the typed word's set.
use crate::completer::Bigrams;
use std::collections::HashMap;

/// The tinyfix list (MIT, paperweight/tinyfix), with its attribution header.
pub const TINYFIX_LIST: &str = include_str!("../../../../data/tinyfix-confusions.txt");
/// Pseudo-count of the unigram prior in each conditional (picked on dev, `tt-confusion-eval`).
pub const BETA: f64 = 200.0;
/// The gain an alternative needs over the typed word, in nats, unless `MARGINS` sets one for the typed word. Picked on
/// the dev half of the held-out text, with `MARGINS`, for at most 0.1 false flags per 1,000 words
/// (docs/research/confusion-pairs-spike.md).
pub const MARGIN: f64 = 6.5;
/// Typed words whose sets need a larger gain: common words that are right far more often than they are a slip, so
/// their few false flags would be most of all of them. Tuned on dev with model-v6:
/// `tt-confusion-eval --beta 200 --tune 0.08 --base 6.5 --min-seen 300` (only typed words seen 300+ times on dev).
pub const MARGINS: &[(&str, f64)] = &[("are", 9.0), ("do", 7.5), ("here", 8.5), ("it's", 8.0), ("its", 8.0), ("know", 8.5),
    ("new", 8.5), ("no", 9.5), ("off", 8.0), ("one", 7.0), ("our", 9.5), ("than", 9.0), ("that", 9.0), ("their", 8.5),
    ("there", 10.5), ("they're", 14.0), ("to", 10.5), ("too", 8.0), ("two", 8.5), ("we're", 15.5), ("were", 7.0), ("where", 9.0)];

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

/// Bigram scorer over the model's English pairs and frequency list (a lowercased copy of the list).
#[derive(Clone, Debug, Default)]
pub struct Scorer {
    freq: HashMap<String, f64>,
    total: f64,
    pub beta: f64,
}
impl Scorer {
    pub fn new(freq: &HashMap<String, f64>, beta: f64) -> Self {
        let mut f: HashMap<String, f64> = HashMap::new();
        for (w, c) in freq { if *c > 0.0 { *f.entry(w.to_ascii_lowercase()).or_default() += c } }
        let total = f.values().sum::<f64>() + f.len() as f64;
        Scorer { freq: f, total, beta }
    }
    pub fn has(&self, w: &str) -> bool { self.freq.contains_key(w) }
    pub fn unigram(&self, w: &str) -> f64 { (self.freq.get(w).copied().unwrap_or(0.0) + 1.0) / self.total }
    /// The stored count of the pair `a b` (a scan of `a`'s followers).
    pub fn count(bg: &Bigrams, a: &str, b: &str) -> f64 { bg.followers(a).find(|(n, _)| *n == b).map_or(0.0, |(_, c)| c) }
    pub fn cond(&self, bg: &Bigrams, a: &str, b: &str) -> f64 {
        (Self::count(bg, a, b) + self.beta * self.unigram(b)) / (bg.mass(a) + self.beta)
    }
    /// ln P(w | prev) + ln P(next | w), all lowercase.
    pub fn score(&self, bg: &Bigrams, prev: Option<&str>, w: &str, next: Option<&str>) -> f64 {
        let l = prev.map_or_else(|| self.unigram(w), |p| self.cond(bg, p, w)).ln();
        l + next.map_or(0.0, |n| self.cond(bg, w, n).ln())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix { pub word: String, pub gain: f64 }

impl Confusions {
    /// The best alternative to `typed` and its gain over it, whatever the margin (all words compared lowercase).
    /// Evidence guard: only words with stored followers (mass > 0 in section 13) are compared. A word without them (a
    /// word seen only at a run end, whose pairs were all pruned, or an apostrophe word in a model before v6) would lose
    /// to any word that has them, so such a typed word gets no row and such alternatives are not offered.
    pub fn best<'s>(&'s self, sc: &Scorer, bg: &Bigrams, prev: Option<&str>, typed: &str, next: Option<&str>) -> Option<(&'s str, f64)> {
        self.best_with(sc, bg, prev, typed, next, true)
    }
    /// `best`, with the evidence guard optional (`guard: false` is for the eval's ablation only).
    pub fn best_with<'s>(&'s self, sc: &Scorer, bg: &Bigrams, prev: Option<&str>, typed: &str, next: Option<&str>, guard: bool) -> Option<(&'s str, f64)> {
        let (p, t, n) = (prev.map(str::to_ascii_lowercase), typed.to_ascii_lowercase(), next.map(str::to_ascii_lowercase));
        let ok = |w: &str| !guard || bg.mass(w) > 0.0;
        if !ok(&t) { return None }
        let alts = self.alternatives(&t);
        if alts.is_empty() { return None }
        let base = sc.score(bg, p.as_deref(), &t, n.as_deref());
        alts.iter().filter(|a| ok(a))
            .map(|a| (a.as_str(), sc.score(bg, p.as_deref(), a, n.as_deref()) - base)).max_by(|x, y| x.1.total_cmp(&y.1))
    }
}

/// The confusion sets, the scorer and the margins: what the engine asks for a real-word row.
#[derive(Clone, Debug, Default)]
pub struct RealWords { pub sets: Confusions, pub scorer: Scorer, pub margin: f64, margins: HashMap<String, f64> }
impl RealWords {
    /// The tinyfix sets with the shipped `BETA`, `MARGIN` and `MARGINS`, over the model's English frequencies.
    pub fn new(freq: &HashMap<String, f64>) -> Self { Self::with(Confusions::tinyfix(), Scorer::new(freq, BETA), MARGIN, MARGINS) }
    pub fn with(sets: Confusions, scorer: Scorer, margin: f64, margins: &[(&str, f64)]) -> Self {
        RealWords { sets, scorer, margin, margins: margins.iter().map(|(w, m)| (w.to_string(), *m)).collect() }
    }
    /// The margin for the set of `typed` (lowercase).
    pub fn margin_for(&self, typed: &str) -> f64 { self.margins.get(typed).copied().unwrap_or(self.margin) }
    /// The row to offer for the finished word `typed` (any case), or none: the best alternative, when it beats the
    /// typed word by its set's margin. `prev` / `next` are the neighbouring words in the same run (no punctuation
    /// between). The offered word takes the typed word's case.
    pub fn check(&self, bg: &Bigrams, prev: Option<&str>, typed: &str, next: Option<&str>) -> Option<Fix> {
        let (best, gain) = self.sets.best(&self.scorer, bg, prev, typed, next)?;
        (gain >= self.margin_for(&typed.to_ascii_lowercase())).then(|| Fix { word: recase(best, typed), gain })
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
    fn toy() -> (Bigrams, RealWords) {
        let bg = Bigrams::new([("letter", "form", 1.0), ("letter", "from", 40.0), ("from", "the", 500.0), ("form", "the", 5.0), ("the", "form", 30.0), ("the", "from", 0.5)]);
        let freq: HashMap<String, f64> = [("form", 100.0), ("from", 3000.0), ("the", 50000.0), ("letter", 200.0)].into_iter().map(|(w, f)| (w.to_string(), f)).collect();
        (bg, RealWords::with(Confusions::tinyfix(), Scorer::new(&freq, 10.0), 1.0, &[]))
    }
    #[test]
    fn offers_the_word_the_pairs_prefer() {
        let (bg, rw) = toy();
        assert_eq!(rw.check(&bg, Some("letter"), "form", Some("the")).map(|f| f.word), Some("from".into()));
        assert_eq!(rw.check(&bg, Some("the"), "form", None), None);
        assert_eq!(rw.check(&bg, Some("letter"), "Form", None).map(|f| f.word), Some("From".into()));
        assert_eq!(rw.check(&bg, Some("letter"), "FORM", None).map(|f| f.word), Some("FROM".into()));
        assert_eq!(rw.check(&bg, Some("letter"), "hello", Some("the")), None, "not in a set");
    }
    #[test]
    fn a_set_margin_overrides_the_default() {
        let (bg, rw) = toy();
        let gain = rw.check(&bg, Some("letter"), "form", Some("the")).unwrap().gain;
        let strict = RealWords::with(Confusions::tinyfix(), rw.scorer.clone(), 1.0, &[("form", gain + 0.1)]);
        assert_eq!(strict.check(&bg, Some("letter"), "form", Some("the")), None);
        assert_eq!(strict.margin_for("from"), 1.0);
    }
    #[test]
    fn words_without_pairs_are_never_compared() {
        let (bg, rw) = toy();
        // `there` and `their` have no stored followers: no row either way
        assert_eq!(rw.check(&bg, Some("the"), "their", None), None);
    }
}
