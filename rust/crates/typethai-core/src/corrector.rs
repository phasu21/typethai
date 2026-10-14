//! Word-level correction suggestions (mirrors src/corrector.ts).
use crate::keymap::{EN_SHIFTED, EN_UNSHIFTED, TH_SHIFTED, TH_UNSHIFTED};
use crate::{ngram::Ngram, normalize::normalize_thai};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const ROWS: [&str; 4] = ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"];
#[derive(Clone, Copy)] struct Pos { r: i32, c: i32, shift: bool }
fn pos_map() -> &'static HashMap<char, Pos> {
    static M: OnceLock<HashMap<char, Pos>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        let en: Vec<char> = EN_UNSHIFTED.chars().collect();
        let th: Vec<char> = TH_UNSHIFTED.chars().collect(); let ths: Vec<char> = TH_SHIFTED.chars().collect();
        let _ = EN_SHIFTED;
        for (i, k) in en.iter().enumerate() {
            let r = ROWS.iter().position(|row| row.contains(*k)).unwrap();
            let c = ROWS[r].chars().position(|x| x == *k).unwrap();
            m.entry(th[i]).or_insert(Pos { r: r as i32, c: c as i32, shift: false });
            m.entry(ths[i]).or_insert(Pos { r: r as i32, c: c as i32, shift: true });
        }
        m
    })
}
fn pos_map_en() -> &'static HashMap<char, Pos> {
    static M: OnceLock<HashMap<char, Pos>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        let en: Vec<char> = EN_UNSHIFTED.chars().collect();
        let ens: Vec<char> = EN_SHIFTED.chars().collect();
        for (i, k) in en.iter().enumerate() {
            let r = ROWS.iter().position(|row| row.contains(*k)).unwrap();
            let c = ROWS[r].chars().position(|x| x == *k).unwrap();
            m.entry(*k).or_insert(Pos { r: r as i32, c: c as i32, shift: false });
            if let Some(sk) = ens.get(i) { m.entry(*sk).or_insert(Pos { r: r as i32, c: c as i32, shift: true }); }
        }
        m
    })
}
fn adjacent(a: Pos, b: Pos) -> bool {
    if a.r == b.r { return (a.c - b.c).abs() == 1 }
    if b.r == a.r - 1 { return b.c == a.c || b.c == a.c + 1 }
    if b.r == a.r + 1 { return b.c == a.c || b.c == a.c - 1 }
    false
}
#[derive(Clone, Copy)]
pub struct Costs { pub shift: f64, pub adj: f64, pub adj_shift: f64, pub sub: f64, pub dup: f64, pub ins_adj: f64, pub ins: f64, pub del: f64, pub trans: f64 }
pub const DEFAULT_COSTS: Costs = Costs { shift: 0.3, adj: 0.5, adj_shift: 0.8, sub: 1.0, dup: 0.4, ins_adj: 0.7, ins: 1.0, del: 0.8, trans: 0.5 };
fn sub_cost(a: char, b: char, k: &Costs) -> f64 {
    if a == b { return 0.0 }
    // Thai (Kedmanee) key layout first, then the plain QWERTY layout for Latin text.
    let maps = [pos_map(), pos_map_en()];
    for m in maps {
        if let (Some(p), Some(q)) = (m.get(&a), m.get(&b)) {
            if p.r == q.r && p.c == q.c { return k.shift }
            if adjacent(*p, *q) { return if p.shift == q.shift { k.adj } else { k.adj_shift } }
            // same known layout but not adjacent: explicit substitution cost (no further map needed)
            return k.sub;
        }
    }
    k.sub
}
/// Weighted Damerau-Levenshtein: cost of turning intended `w` into observed `t`.
pub fn key_distance(t: &str, w: &str, k: &Costs) -> f64 {
    let a: Vec<char> = t.chars().collect(); let b: Vec<char> = w.chars().collect(); let (n, m) = (a.len(), b.len());
    let get = |v: &Vec<char>, i: isize| if i >= 0 && (i as usize) < v.len() { Some(v[i as usize]) } else { None };
    let ins_cost = |i: usize, j: usize| -> f64 {
        let x = a[i - 1];
        let prev = get(&a, i as isize - 2).or_else(|| get(&b, j as isize - 1));
        if prev == Some(x) { return k.dup }
        let m = if pos_map().contains_key(&x) { pos_map() } else { pos_map_en() };
        if let Some(px) = m.get(&x) {
            for c in [get(&b, j as isize - 1), get(&b, j as isize)].into_iter().flatten() {
                if let Some(q) = m.get(&c) { if adjacent(*px, *q) { return k.ins_adj } }
            }
        }
        k.ins
    };
    let mut d = vec![vec![0.0f64; m + 1]; n + 1];
    for i in 1..=n { d[i][0] = d[i - 1][0] + ins_cost(i, 0); }
    for j in 1..=m { d[0][j] = d[0][j - 1] + k.del; }
    for i in 1..=n { for j in 1..=m {
        let mut v = (d[i - 1][j] + ins_cost(i, j)).min(d[i][j - 1] + k.del).min(d[i - 1][j - 1] + sub_cost(b[j - 1], a[i - 1], k));
        if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] { v = v.min(d[i - 2][j - 2] + k.trans); }
        d[i][j] = v;
    } }
    d[n][m]
}
fn fnv(s: &str) -> u64 { let mut h: u64 = 0xcbf29ce484222325; for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } h }
/// Ordered delete set (word itself first, then BFS by distance), matching the TS Set iteration order.
fn deletes(w: &[char], max_d: usize) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new(); let mut out = Vec::new();
    let s0: String = w.iter().collect(); seen.insert(s0.clone()); out.push(s0);
    let mut frontier: Vec<Vec<char>> = vec![w.to_vec()];
    for _ in 0..max_d {
        let mut next = Vec::new();
        for x in &frontier { for i in 0..x.len() {
            let mut y = x.clone(); y.remove(i); let s: String = y.iter().collect();
            if seen.insert(s.clone()) { out.push(s); next.push(y); }
        } }
        frontier = next;
    }
    out
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectorParams { pub alpha: f64, pub beta: f64, pub gamma: f64, pub keep_bias: f64, pub tau: f64, pub max_dist: f64 }
#[derive(Clone, Debug)] pub struct Candidate { pub word: String, pub dist: f64, pub log_freq: f64, pub ll: f64 }
#[derive(Clone, Debug, Serialize)] pub struct Suggestion { pub word: String, pub score: f64, pub confidence: f64, pub dist: f64 }
#[derive(Clone, Debug, Serialize)]
pub struct Correction { pub input: String, pub normalized: String, pub in_dict: bool, pub suggestions: Vec<Suggestion>, pub keep_confidence: f64, pub apply: Option<String> }

/// The delete index is a flat, sorted `(fnv64(delete), word id)` array (binary-searched): ~12 B per posting instead of
/// a String-keyed HashMap. Within one key ids stay ascending, so candidate order (and thus ranking ties) matches TS.
pub struct Corrector { index: Vec<(u64, u32)>, words: Vec<String>, word_set: HashSet<String>, freq: HashMap<String, f64>, pub params: CorrectorParams, max_del: usize }
impl Corrector {
    pub fn new(words: &[String], freq: HashMap<String, f64>, params: CorrectorParams) -> Self {
        let mut seen = HashSet::new(); let mut ws = Vec::new();
        for w in words { let n = w.chars().count(); if !w.chars().any(char::is_whitespace) && (2..=16).contains(&n) && seen.insert(w.clone()) { ws.push(w.clone()); } }
        let mut index: Vec<(u64, u32)> = Vec::new();
        for (id, w) in ws.iter().enumerate() {
            let ch: Vec<char> = w.chars().collect();
            for d in deletes(&ch, if ch.len() <= 3 { 1 } else { 2 }) { index.push((fnv(&d), id as u32)); }
        }
        index.sort_unstable(); index.dedup(); index.shrink_to_fit();
        Corrector { word_set: ws.iter().cloned().collect(), words: ws, index, freq, params, max_del: 2 }
    }
    pub fn has(&self, w: &str) -> bool { self.word_set.contains(w) }
    pub fn candidates(&self, t: &str, thai: &Ngram) -> Vec<Candidate> {
        let ch: Vec<char> = t.chars().collect();
        let mut seen = HashSet::new(); let mut ids = Vec::new();
        for d in deletes(&ch, self.max_del) {
            let h = fnv(&d); let start = self.index.partition_point(|e| e.0 < h);
            for e in &self.index[start..] { if e.0 != h { break } if seen.insert(e.1) { ids.push(e.1) } }
        }
        let mut out = Vec::new();
        for id in ids {
            let w = &self.words[id as usize]; if w == t { continue }
            let dist = key_distance(t, w, &DEFAULT_COSTS);
            if dist <= self.params.max_dist { out.push(Candidate { word: w.clone(), dist, log_freq: (self.freq.get(w).copied().unwrap_or(0.0)).ln_1p(), ll: thai.mean_log_prob(w) }); }
        }
        out
    }
    pub fn rank(cands: &[Candidate], keep_ll: f64, p: &CorrectorParams) -> (Vec<Suggestion>, f64) {
        let sc: Vec<f64> = cands.iter().map(|c| -p.alpha * c.dist + p.gamma * c.log_freq + p.beta * c.ll).collect();
        let keep = p.keep_bias + p.beta * keep_ll;
        let mx = sc.iter().cloned().fold(keep, f64::max);
        let z = sc.iter().fold((keep - mx).exp(), |a, s| a + (s - mx).exp());
        let mut idx: Vec<usize> = (0..cands.len()).collect();
        idx.sort_by(|a, b| sc[*b].partial_cmp(&sc[*a]).unwrap()); // stable, like JS Array.sort
        let sugg = idx.into_iter().map(|i| Suggestion { word: cands[i].word.clone(), score: sc[i], dist: cands[i].dist, confidence: (sc[i] - mx).exp() / z }).collect();
        (sugg, (keep - mx).exp() / z)
    }
    pub fn suggest(&self, token: &str, k: usize, thai: &Ngram) -> Correction {
        let normalized = normalize_thai(token);
        if self.has(&normalized) {
            let apply = if normalized != token { Some(normalized.clone()) } else { None };
            return Correction { input: token.into(), normalized, in_dict: true, suggestions: vec![], keep_confidence: 1.0, apply };
        }
        let cands = self.candidates(&normalized, thai);
        let (mut s, keep) = Corrector::rank(&cands, thai.mean_log_prob(&normalized), &self.params);
        let apply = match s.first() { Some(t) if t.confidence >= self.params.tau => Some(t.word.clone()), _ => if normalized != token { Some(normalized.clone()) } else { None } };
        s.truncate(k);
        Correction { input: token.into(), normalized, in_dict: false, suggestions: s, keep_confidence: keep, apply }
    }
    pub fn index_entries(&self) -> usize { self.index.len() }
    pub fn index_bytes(&self) -> usize { self.index.capacity() * std::mem::size_of::<(u64, u32)>() }
}
