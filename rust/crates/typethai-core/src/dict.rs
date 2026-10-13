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
}
