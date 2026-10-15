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
    /// Max-coverage segmentation (mirrors `segment` in src/corrector.ts): a dictionary piece of length L scores L²,
    /// an unknown char -1, and consecutive unknown chars come back as one piece.
    pub fn segment<'a>(&self, s: &'a str) -> Vec<&'a str> {
        const MAX_LEN: usize = 16;
        let at: Vec<usize> = s.char_indices().map(|(b, _)| b).chain([s.len()]).collect(); let n = at.len() - 1;
        let piece = |j: usize, i: usize| &s[at[j]..at[i]];
        let (mut best, mut back) = (vec![i64::MIN; n + 1], vec![0usize; n + 1]); best[0] = 0;
        for i in 1..=n {
            (best[i], back[i]) = (best[i - 1] - 1, i - 1);
            for l in 1..=MAX_LEN.min(i) {
                let v = best[i - l] + (l * l) as i64;
                if v > best[i] && self.has(piece(i - l, i)) { (best[i], back[i]) = (v, i - l); }
            }
        }
        let (mut out, mut i, mut unk_end) = (Vec::new(), n, None);
        while i > 0 {
            let j = back[i];
            if i - j == 1 && !self.has(piece(j, i)) { unk_end.get_or_insert(i); }
            else { if let Some(e) = unk_end.take() { out.push(piece(i, e)); } out.push(piece(j, i)); }
            i = j;
        }
        if let Some(e) = unk_end { out.push(piece(0, e)); }
        out.reverse(); out
    }
}
