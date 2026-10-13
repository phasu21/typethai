//! Train + calibrate + evaluate the reverse (English typed on the Thai layout) classifier.
//! usage: tt-train-rev train <model.ttm|dir> <data dir> <out reverse-model.json>
//!        tt-train-rev eval  <model.ttm|dir> <data dir> [reverse-model.json]
//! Positives: English prose words (en.<split>.json), typed as their keys. Negatives: real Thai tokens (th.<split> gold =
//! wiki, chat/<split>.json, slang included) turned into the keys that type them on Kedmanee (tokens with Latin letters or
//! ASCII digits can't be typed on that layout and are skipped), plus a typo'd copy (1 random key edit, 20%).
//! Training lines: pure English, pure Thai, and Thai lines with a 1–3 word English run inserted (25%).
use serde_json::Value;
use std::path::Path;
use typethai_core::{classifier::{context_features, Lr, FlipModel}, reverse::{features, th_text_to_keys, Reverse, score}, Detector, ModelFiles};

struct Rng(u64);
impl Rng { fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 } fn f(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 } fn n(&mut self, k: usize) -> usize { (self.next() % k as u64) as usize } }

fn load_json(p: &Path) -> Value { serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap() }
fn en_lines(dir: &Path, split: &str, cap: usize) -> Vec<Vec<String>> {
    load_json(&dir.join(format!("splits/en.{split}.json"))).as_array().unwrap().iter().filter_map(|x| x.as_str()).take(cap)
        .map(|l| l.split_whitespace().map(String::from).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect()
}
/// Thai text lines -> key tokens (untypeable tokens dropped).
fn th_to_key_lines(lines: impl Iterator<Item = String>) -> Vec<Vec<String>> {
    lines.map(|l| l.split_whitespace().filter_map(th_text_to_keys).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect()
}
fn th_wiki(dir: &Path, split: &str) -> Vec<Vec<String>> { th_to_key_lines(load_json(&dir.join(format!("splits/th.{split}.json"))).as_array().unwrap().iter().filter_map(|x| x["gold"].as_str().map(String::from))) }
fn th_chat(dir: &Path, split: &str) -> Vec<Vec<String>> { th_to_key_lines(load_json(&dir.join(format!("chat/{split}.json"))).as_array().unwrap().iter().filter_map(|x| x.as_str().map(String::from))) }
fn th_list(p: &Path, cap: usize) -> Vec<Vec<String>> { th_to_key_lines(load_json(p).as_array().unwrap().iter().filter_map(|x| x.as_str().map(String::from)).take(cap)) }
fn th_jsonl(p: &Path, cap: usize) -> Vec<Vec<String>> {
    th_to_key_lines(std::fs::read_to_string(p).unwrap().lines().take(cap).filter_map(|l| serde_json::from_str::<Value>(l).ok()).flat_map(|v| v["text"].as_str().unwrap_or("").lines().map(String::from).collect::<Vec<_>>()))
}
const KEYS: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,./1234567890-=";
fn typo(r: &mut Rng, k: &str) -> String {
    let mut c: Vec<char> = k.chars().collect(); if c.is_empty() { return k.into() }
    let i = r.n(c.len()); let key = KEYS.chars().nth(r.n(KEYS.len())).unwrap();
    match r.n(4) { 0 => c[i] = key, 1 => { if c.len() > 1 { c.remove(i); } } 2 => c.insert(i, c[i]), _ => if i + 1 < c.len() { c.swap(i, i + 1) } }
    c.into_iter().collect()
}
/// (tokens, labels)
type Line = (Vec<String>, Vec<bool>);
fn build_train(dir: &Path, r: &mut Rng) -> Vec<Line> {
    let en = en_lines(dir, "train", 30000);
    let mut th = th_wiki(dir, "train"); th.extend(th_chat(dir, "train"));
    let pool: Vec<String> = en.iter().flatten().cloned().collect();
    let mut out: Vec<Line> = en.into_iter().map(|l| { let n = l.len(); (l, vec![true; n]) }).collect();
    for l in th {
        let mut toks: Vec<String> = l.iter().map(|k| if r.f() < 0.2 { typo(r, k) } else { k.clone() }).collect();
        let mut lab = vec![false; toks.len()];
        if r.f() < 0.25 { let at = r.n(toks.len() + 1); let run = 1 + r.n(3);
            for j in 0..run { toks.insert(at + j, pool[r.n(pool.len())].clone()); lab.insert(at + j, true); } }
        out.push((toks, lab));
    }
    out
}
fn train_lr(x: &[Vec<f64>], y: &[bool], epochs: usize) -> Lr {
    let d = x[0].len(); let n = x.len() as f64;
    let mut mu = vec![0.0; d]; let mut sd = vec![0.0; d];
    for r in x { for k in 0..d { mu[k] += r[k] / n } }
    for r in x { for k in 0..d { sd[k] += (r[k] - mu[k]).powi(2) / n } }
    for s in sd.iter_mut() { *s = s.sqrt(); if *s < 1e-9 { *s = 1.0 } }
    let z: Vec<Vec<f64>> = x.iter().map(|r| (0..d).map(|k| (r[k] - mu[k]) / sd[k]).collect()).collect();
    let npos = y.iter().filter(|v| **v).count() as f64; let (wp, wn) = (n / (2.0 * npos.max(1.0)), n / (2.0 * (n - npos).max(1.0)));
    let (mut w, mut b) = (vec![0.0; d], 0.0);
    let (mut m, mut v, mut mb, mut vb) = (vec![0.0; d], vec![0.0; d], 0.0, 0.0); let (b1, b2, lr, l2) = (0.9, 0.999, 0.05, 1e-4);
    for t in 1..=epochs {
        let mut g = vec![0.0; d]; let mut gb = 0.0;
        for (i, r) in z.iter().enumerate() {
            let s = b + (0..d).map(|k| w[k] * r[k]).sum::<f64>(); let p = 1.0 / (1.0 + (-s).exp());
            let e = (p - if y[i] { 1.0 } else { 0.0 }) * if y[i] { wp } else { wn };
            for k in 0..d { g[k] += e * r[k] / n } gb += e / n;
        }
        for k in 0..d { g[k] += l2 * w[k]; m[k] = b1 * m[k] + (1.0 - b1) * g[k]; v[k] = b2 * v[k] + (1.0 - b2) * g[k] * g[k];
            w[k] -= lr * (m[k] / (1.0 - b1.powi(t as i32))) / ((v[k] / (1.0 - b2.powi(t as i32))).sqrt() + 1e-8); }
        mb = b1 * mb + (1.0 - b1) * gb; vb = b2 * vb + (1.0 - b2) * gb * gb; b -= lr * (mb / (1.0 - b1.powi(t as i32))) / ((vb / (1.0 - b2.powi(t as i32))).sqrt() + 1e-8);
    }
    Lr { w, b, mu, sd }
}
fn train(det: &Detector, dir: &Path, out: &Path) {
    let mut r = Rng(0x9E3779B97F4A7C15);
    let lines = build_train(dir, &mut r);
    let ntok: usize = lines.iter().map(|l| l.0.len()).sum(); let npos: usize = lines.iter().map(|l| l.1.iter().filter(|x| **x).count()).sum();
    eprintln!("train lines {} tokens {ntok} (english {npos}, thai {})", lines.len(), ntok - npos);
    let ds: Vec<Vec<_>> = lines.iter().map(|(t, _)| t.iter().map(|k| score(det, k)).collect()).collect();
    let (mut x1, mut y1) = (vec![], vec![]);
    for (l, d) in lines.iter().zip(&ds) { for (i, dd) in d.iter().enumerate() { if dd.convertible { x1.push(features(dd)); y1.push(l.1[i]); } } }
    let s1 = train_lr(&x1, &y1, 300);
    let (mut x2, mut y2) = (vec![], vec![]);
    for (l, d) in lines.iter().zip(&ds) {
        let p1: Vec<f64> = d.iter().map(|dd| typethai_core::classifier::lr_predict(&s1, &features(dd))).collect();
        for (i, dd) in d.iter().enumerate() { if dd.convertible { let mut x = features(dd); x.extend(context_features(&p1, i)); x2.push(x); y2.push(l.1[i]); } }
    }
    let s2 = train_lr(&x2, &y2, 300);
    let m = FlipModel { stage1: s1, stage2: s2, threshold: 0.5, names: vec![] };
    std::fs::write(out, serde_json::to_string(&m).unwrap()).unwrap(); eprintln!("wrote {}", out.display());
}

fn probs(rev: &Reverse, det: &Detector, l: &[String]) -> Vec<f64> { let k: Vec<&str> = l.iter().map(|s| s.as_str()).collect(); rev.line_probs(&rev.decide_line(det, &k)) }
fn eval(det: &Detector, dir: &Path, rev: &Reverse) {
    let grid = [0.5, 0.7, 0.8, 0.9, 0.95, 0.97, 0.98, 0.99, 0.995, 0.998, 0.999];
    let mut r = Rng(12345);
    let rate = |sets: &[(String, Vec<Vec<String>>)], typo_it: bool, r: &mut Rng| -> Vec<(String, usize, Vec<usize>)> {
        sets.iter().map(|(name, ls)| {
            let mut n = 0; let mut c = vec![0usize; grid.len()];
            for l in ls { let l: Vec<String> = if typo_it { l.iter().map(|k| typo(r, k)).collect() } else { l.clone() };
                for p in probs(rev, det, &l) { n += 1; for (g, t) in grid.iter().enumerate() { if p > *t { c[g] += 1 } } } }
            (name.clone(), n, c)
        }).collect()
    };
    let show = |title: &str, rows: &[(String, usize, Vec<usize>)]| {
        println!("\n{title}"); print!("{:<34}", "set (tokens)"); for t in grid { print!("{t:>7}") } println!();
        for (nm, n, c) in rows { print!("{:<34}", format!("{nm} ({n})")); for x in c { print!("{:>6.2}%", 100.0 * *x as f64 / (*n).max(1) as f64) } println!(); }
    };
    for split in ["dev", "test"] {
        let th = vec![(format!("thai wiki {split}"), th_wiki(dir, split)), (format!("thai chat {split}"), if split == "dev" { th_chat(dir, "dev") } else { vec![] })];
        let th: Vec<_> = th.into_iter().filter(|x| !x.1.is_empty()).collect();
        show(&format!("[{split}] FALSE FLIP rate on real Thai (share of tokens with p > threshold)"), &rate(&th, false, &mut r));
        show(&format!("[{split}] false flip rate on typo'd Thai (1 random key edit per token)"), &rate(&th, true, &mut r));
        // recall on English typed on the Thai layout: pure lines, and short words (<=3 letters)
        let en = en_lines(dir, split, usize::MAX);
        let (mut n, mut ns) = (0, 0); let (mut c, mut cs) = (vec![0usize; grid.len()], vec![0usize; grid.len()]);
        for l in &en { for (k, p) in l.iter().zip(probs(rev, det, l)) {
            let short = k.trim_matches(|c: char| !c.is_ascii_alphanumeric()).chars().count() <= 3;
            n += 1; if short { ns += 1 } for (g, t) in grid.iter().enumerate() { if p > *t { c[g] += 1; if short { cs[g] += 1 } } } } }
        show(&format!("[{split}] RECALL on English typed on Thai layout"), &[(format!("english prose {split}"), n, c), (format!("  of which <=3 chars"), ns, cs)]);
        // mixed: English run of 1-3 words inside a Thai line
        let pool: Vec<String> = en.iter().flatten().cloned().collect(); let thl = th_wiki(dir, split);
        let (mut nm, mut cm) = (0, vec![0usize; grid.len()]);
        for l in thl.iter().take(2000) { let at = r.n(l.len() + 1); let run = 1 + r.n(3); let mut t = l.clone();
            for j in 0..run { t.insert(at + j, pool[r.n(pool.len())].clone()); }
            let p = probs(rev, det, &t); for j in 0..run { nm += 1; for (g, th) in grid.iter().enumerate() { if p[at + j] > *th { cm[g] += 1 } } } }
        show(&format!("[{split}] recall, English run inside a Thai line"), &[(format!("mixed {split}"), nm, cm)]);
    }
    let fresh = vec![("tweets (fresh)".to_string(), th_list(&dir.join("fresh/tweets_lines.json"), usize::MAX)),
        ("wongnai reviews (fresh)".to_string(), th_list(&dir.join("fresh/wongnai_lines.json"), 3000)),
        ("thwiki fresh".to_string(), th_jsonl(&dir.join("fresh/thwiki_fresh.jsonl"), 300))];
    show("[fresh] FALSE FLIP rate on real Thai", &rate(&fresh, false, &mut r));
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = ModelFiles::load(Path::new(&a[2])).or_else(|_| ModelFiles::load_json_dir(Path::new(&a[2]))).expect("model");
    let det = m.learn_detector(); let dir = Path::new(&a[3]);
    match a[1].as_str() {
        "train" => train(&det, dir, Path::new(&a[4])),
        "eval" => { let rev = match a.get(4) { Some(p) => Reverse { model: serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap() }, None => Reverse::embedded() }; eval(&det, dir, &rev) }
        _ => eprintln!("usage: tt-train-rev train|eval <model> <data dir> [out/model json]"),
    }
}
