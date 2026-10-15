//! Dictionary coverage via DP (mirrors src/dict.ts).
use std::collections::HashSet;
pub struct Dict { pub words: HashSet<String>, max_len: usize }
impl Dict {
    pub fn new<I: IntoIterator<Item = String>>(it: I) -> Self {
        let mut words = HashSet::new(); let mut max_len = 1;
        for w in it { if w.is_empty() { continue } max_len = max_len.max(w.chars().count()); words.insert(w); }
        Dict { words, max_len: max_len.min(24) }
    }
    pub fn has(&self, w: &str) -> bool { self.words.contains(w) }
    pub fn coverage(&self, s: &str) -> f64 { self.coverage_min(s, 2) }
    pub fn coverage_min(&self, s: &str, min_len: usize) -> f64 {
        let ch: Vec<char> = s.chars().collect(); let n = ch.len();
        if n == 0 { return 0.0 }
        let mut best = vec![0usize; n + 1];
        let mut buf = String::new();
        for i in 1..=n {
            best[i] = best[i - 1];
            let mut l = min_len;
            while l <= self.max_len.min(i) {
                buf.clear(); buf.extend(&ch[i - l..i]);
                if self.words.contains(&buf) { best[i] = best[i].max(best[i - l] + l); }
                l += 1;
            }
        }
        best[n] as f64 / n as f64
    }
    /// `s` split into dictionary words: as much of it covered as possible, then as few words as possible. Each piece is
    /// (text, is a word); the uncovered stretches between words are the other pieces.
    pub fn segment<'s>(&self, s: &'s str) -> Vec<(&'s str, bool)> {
        let at: Vec<usize> = s.char_indices().map(|(b, _)| b).chain([s.len()]).collect(); let n = at.len() - 1;
        // best[i] = (chars covered, -words) over s[..i]; from[i] = where the last piece starts and whether it is a word
        let mut best = vec![(0usize, 0isize); n + 1]; let mut from = vec![(0usize, false); n + 1];
        for i in 1..=n {
            (best[i], from[i]) = (best[i - 1], (i - 1, false));
            for l in 1..=self.max_len.min(i) {
                let cand = (best[i - l].0 + l, best[i - l].1 - 1);
                if cand > best[i] && self.words.contains(&s[at[i - l]..at[i]]) { (best[i], from[i]) = (cand, (i - l, true)); }
            }
        }
        let mut out: Vec<(&str, bool)> = vec![]; let mut i = n;
        while i > 0 {
            let (j, word) = from[i];
            match out.last_mut() {
                // one gap piece for a run of uncovered chars (pieces are collected back to front)
                Some((g, false)) if !word => *g = &s[at[j]..at[i] + g.len()],
                _ => out.push((&s[at[j]..at[i]], word)),
            }
            i = j;
        }
        out.reverse();
        out
    }
}
