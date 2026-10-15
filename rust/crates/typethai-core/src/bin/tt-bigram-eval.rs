//! Offline eval of word pair counts for completion, on held-out text never used to count pairs.
//! usage: tt-bigram-eval <model.ttm> [--lambda L] <held-out text file>...
//! For each word of 3+ chars that has a previous word in its run (corpus::word_runs), is it among the top 3
//! completions of its first 2 chars?
//!   unigram: the model's completer (corpus frequency).
//!   context: score = L·c(prev,w)/Σc(prev,·) + (1-L)·f(w)/Σf, over the unigram top 3+k and the k stored followers
//!            of prev that extend the prefix (L = 0.5 by default; the sum is over the followers kept in the model).
//! --marginal: ablation that ignores prev, scoring with c(·,w)/Σc (the stored pairs as in-domain word counts), to
//!   separate the effect of the previous word from that of the pair corpus's domain.
//!   shipped: `Completer::complete` with the previous word and the pairs, as the app ranks (no word memory).
//! Columns: targets, in completion vocab (the ceiling for re-ranking), prev has stored followers, unigram hits,
//! context hits, shipped hits.
use std::{cell::RefCell, collections::HashMap};
use typethai_core::{completer::{Bigrams, Completer, Context}, corpus::{word_runs, Lang}, dict::Dict, ModelFiles};

/// `complete(prefix, CACHED)` results are cached per prefix (held-out words share few 2-char prefixes).
const CACHED: usize = 64;
struct Lm<'a> { comp: Completer, freq: HashMap<String, f64>, total: f64, bg: &'a Bigrams, cache: RefCell<HashMap<String, Vec<String>>>, marginal: Option<HashMap<String, f64>> }
impl<'a> Lm<'a> {
    fn new(words: Vec<(String, f64)>, bg: &'a Bigrams, marginal: bool) -> Self {
        let mut freq: HashMap<String, f64> = HashMap::new();
        for (w, f) in &words { if *f > 0.0 { *freq.entry(w.to_ascii_lowercase()).or_default() += f } }
        let marginal = marginal.then(|| {
            let mut m: HashMap<&str, f64> = HashMap::new();
            for (_, n, c) in bg.iter() { *m.entry(n).or_default() += c }
            m.into_iter().map(|(w, c)| (w.to_string(), c)).collect()
        });
        Lm { total: freq.values().sum(), freq, comp: Completer::new(words), bg, cache: Default::default(), marginal }
    }
    fn top(&self, prefix: &str, k: usize) -> Vec<String> {
        if k > CACHED { return self.comp.complete(prefix, k, Context::default()).words }
        let mut c = self.cache.borrow_mut();
        c.entry(prefix.to_string()).or_insert_with(|| self.comp.complete(prefix, CACHED, Context::default()).words).iter().take(k).cloned().collect()
    }
    fn unigram(&self, prefix: &str) -> Vec<String> { self.top(prefix, 3) }
    fn shipped(&self, prev: &str, prefix: &str) -> Vec<String> { self.comp.complete(prefix, 3, Context { prev: Some(prev), bigrams: Some(self.bg), ..Context::default() }).words }
    fn context(&self, prev: &str, prefix: &str, lambda: f64) -> Vec<String> {
        let Some(m) = &self.marginal else { return self.rank(&self.bg.followers(prev).collect::<Vec<_>>(), prefix, lambda) };
        let key = format!("ctx\t{prefix}");
        if let Some(r) = self.cache.borrow().get(&key) { return r.clone() }
        let sum: f64 = m.values().sum();
        let mut cands: Vec<&str> = self.freq.keys().chain(m.keys()).map(String::as_str).filter(|w| w.len() > prefix.len() && w.starts_with(prefix)).collect();
        cands.sort(); cands.dedup();
        let score = |w: &str| lambda * m.get(w).map_or(0.0, |c| c / sum) + (1.0 - lambda) * self.freq.get(w).map_or(0.0, |f| f / self.total);
        cands.sort_by(|a, b| score(b).total_cmp(&score(a)).then_with(|| a.cmp(b)));
        let r: Vec<String> = cands.into_iter().take(3).map(String::from).collect();
        self.cache.borrow_mut().insert(key, r.clone()); r
    }
    fn rank(&self, fol: &[(&str, f64)], prefix: &str, lambda: f64) -> Vec<String> {
        let sum: f64 = fol.iter().map(|f| f.1).sum();
        let ext: HashMap<&str, f64> = fol.iter().filter(|(w, _)| w.len() > prefix.len() && w.starts_with(prefix)).map(|(w, c)| (*w, *c)).collect();
        let mut cands = self.top(prefix, 3 + ext.len());
        cands.extend(ext.keys().map(|w| w.to_string())); cands.sort(); cands.dedup();
        let score = |w: &str| lambda * ext.get(w).map_or(0.0, |c| c / sum) + (1.0 - lambda) * self.freq.get(w).map_or(0.0, |f| f / self.total);
        cands.sort_by(|a, b| score(b).total_cmp(&score(a)).then_with(|| a.cmp(b)));
        cands.truncate(3); cands
    }
}
#[derive(Default)] struct Tally { n: u64, in_vocab: u64, covered: u64, uni: u64, ctx: u64, ship: u64 }
fn pct(a: u64, n: u64) -> String { format!("{:5.1}%", 100.0 * a as f64 / n.max(1) as f64) }

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = ModelFiles::load(std::path::Path::new(&a[1])).expect("model");
    let lambda: f64 = a.iter().position(|x| x == "--lambda").map_or(0.5, |i| a[i + 1].parse().expect("--lambda"));
    let (mut files, mut i) = (Vec::new(), 2);
    while i < a.len() { match a[i].as_str() { "--lambda" => i += 1, "--marginal" => {} f => files.push(f) } i += 1; }
    let marginal = a.iter().any(|x| x == "--marginal");
    let dict = Dict::new(m.thai_words.iter().cloned());
    let th = Lm::new(m.thai_words.iter().filter_map(|w| Some((w.clone(), *m.wordfreq.get(w)?))).collect(), &m.th_bigrams, marginal);
    let en = Lm::new(m.english_freq.iter().map(|(w, f)| (w.clone(), *f)).collect(), &m.en_bigrams, marginal);
    println!("model {} | th pairs {} | en pairs {} | lambda {lambda}{}", a[1], m.th_bigrams.len(), m.en_bigrams.len(), if marginal { " | MARGINAL ablation" } else { "" });
    println!("{:<44} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7}", "file / lang", "targets", "inVocab", "covered", "unigram", "context", "shipped");
    let mut totals: HashMap<&str, Tally> = HashMap::new();
    for f in files {
        let mut t: HashMap<&str, Tally> = HashMap::new();
        for line in std::fs::read_to_string(f).expect(f).lines() {
            for (lang, ws) in word_runs(line, &dict) {
                let (name, lm) = match lang { Lang::Th => ("th", &th), Lang::En => ("en", &en) };
                for p in ws.windows(2) {
                    let (prev, w) = (&p[0], &p[1]);
                    if w.chars().count() < 3 { continue }
                    let prefix: String = w.chars().take(2).collect();
                    let e = t.entry(name).or_default();
                    e.n += 1; e.in_vocab += lm.freq.contains_key(w) as u64; e.covered += (lm.bg.followers(prev).len() > 0) as u64;
                    e.uni += lm.unigram(&prefix).contains(w) as u64; e.ctx += lm.context(prev, &prefix, lambda).contains(w) as u64;
                    e.ship += lm.shipped(prev, &prefix).contains(w) as u64;
                }
            }
        }
        let short = std::path::Path::new(f).file_name().unwrap().to_string_lossy().into_owned();
        for (lang, x) in ["th", "en"].into_iter().filter_map(|l| Some((l, t.get(l)?))) {
            println!("{:<44} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7}", format!("{short} / {lang}"), x.n, pct(x.in_vocab, x.n), pct(x.covered, x.n), pct(x.uni, x.n), pct(x.ctx, x.n), pct(x.ship, x.n));
            let s = totals.entry(lang).or_default();
            (s.n, s.in_vocab, s.covered, s.uni, s.ctx, s.ship) = (s.n + x.n, s.in_vocab + x.in_vocab, s.covered + x.covered, s.uni + x.uni, s.ctx + x.ctx, s.ship + x.ship);
        }
    }
    for lang in ["th", "en"] {
        let Some(x) = totals.get(lang) else { continue };
        println!("{:<44} {:>8} {:>7} {:>7} {:>7} {:>7} {:>7}", format!("ALL / {lang}"), x.n, pct(x.in_vocab, x.n), pct(x.covered, x.n), pct(x.uni, x.n), pct(x.ctx, x.n), pct(x.ship, x.n));
    }
}
