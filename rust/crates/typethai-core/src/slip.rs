//! The typing channel: how likely the keys typed are when a word was meant, one slip of a finger at a time.
use crate::keymap::{reach, Reach};

/// One way the keys typed differ from the word meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slip {
    /// Two keys in the wrong order: `teh` for `the`.
    Swap,
    /// A neighbouring key instead of the meant one: `thw` for `the`.
    Near,
    /// The meant key with Shift the other way (Thai `ฏ` for `ก`).
    Shift,
    /// Any other key instead of the meant one.
    Far,
    /// A meant key left out: `hllo`.
    Drop,
    /// A key typed twice: `helllo`.
    Double,
    /// A key next to the one before or after it, typed as well: `hrello`.
    Extra,
    /// Any other key typed as well.
    Stray,
}
impl Slip {
    /// The kind's share of all slips, and how many slips of the kind one key allows (the share is split among them):
    /// six neighbours, about thirty other letters, about eight keys next to the key before or after. The shares follow
    /// the usual split of typing errors (substitutions ~40%, omissions ~25%, insertions ~20%, transpositions ~15%),
    /// most substitutions and insertions on neighbouring keys.
    fn table(self) -> (f64, f64) {
        match self {
            Slip::Swap => (0.15, 1.0), Slip::Near => (0.30, 6.0), Slip::Shift => (0.05, 1.0), Slip::Far => (0.05, 30.0),
            Slip::Drop => (0.25, 1.0), Slip::Double => (0.08, 1.0), Slip::Extra => (0.08, 8.0), Slip::Stray => (0.04, 30.0),
        }
    }
    /// The slip that types `typed` where `meant` was meant (they differ).
    pub fn instead(meant: char, typed: char) -> Slip {
        match reach(meant, typed) { Reach::SameKey => Slip::Shift, Reach::Near => Slip::Near, Reach::Far => Slip::Far }
    }
    /// The slip that adds `extra` between the keys typed `before` and `after` it.
    pub fn extra(extra: char, before: Option<char>, after: Option<char>) -> Slip {
        if before == Some(extra) { return Slip::Double }
        if before.into_iter().chain(after).any(|c| reach(c, extra) == Reach::Near) { Slip::Extra } else { Slip::Stray }
    }
}

/// How often fingers slip: `rate` is the chance that a key typed is a slip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slips { pub rate: f64 }
impl Default for Slips { fn default() -> Self { Slips { rate: 0.01 } } }
impl Slips {
    /// The chance of this one slip at one key.
    pub fn p(&self, s: Slip) -> f64 { let (share, ways) = s.table(); self.rate * share / ways }
    /// P(`typed` | `meant`), whole words: the likeliest way of slipping from one to the other, slip by slip (a weighted
    /// Damerau-Levenshtein distance). 1 when they are equal.
    pub fn chance(&self, typed: &str, meant: &str) -> f64 {
        let (t, m): (Vec<char>, Vec<char>) = (typed.chars().collect(), meant.chars().collect());
        let cost = |s: Slip| -self.p(s).ln();
        let mut d = vec![vec![f64::INFINITY; m.len() + 1]; t.len() + 1];
        d[0][0] = 0.0;
        for i in 0..=t.len() { for j in 0..=m.len() {
            let mut v = d[i][j];
            if i > 0 { v = v.min(d[i - 1][j] + cost(Slip::extra(t[i - 1], i.checked_sub(2).map(|k| t[k]), t.get(i).copied()))) }
            if j > 0 { v = v.min(d[i][j - 1] + cost(Slip::Drop)) }
            if i > 0 && j > 0 { v = v.min(d[i - 1][j - 1] + if t[i - 1] == m[j - 1] { 0.0 } else { cost(Slip::instead(m[j - 1], t[i - 1])) }) }
            if i > 1 && j > 1 && t[i - 1] == m[j - 2] && t[i - 2] == m[j - 1] && t[i - 1] != t[i - 2] { v = v.min(d[i - 2][j - 2] + cost(Slip::Swap)) }
            d[i][j] = v;
        } }
        (-d[t.len()][m.len()]).exp()
    }
}
