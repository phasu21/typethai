//! Character trigram model with interpolated backoff (mirrors src/ngram.ts meanLogProb exactly).
use std::collections::HashMap;

const BOS: char = '\u{0002}';
const EOS: char = '\u{0003}';

#[inline] fn k2(a: char, b: char) -> u64 { ((a as u64) << 21) | b as u64 }
#[inline] fn k3(a: char, b: char, c: char) -> u64 { ((a as u64) << 42) | ((b as u64) << 21) | c as u64 }

#[derive(Default, Clone, Debug)]
pub struct Ngram {
    pub vocab: f64,
    pub lambdas: [f64; 4],
    pub total: Option<f64>, // "1|"
    pub c1: HashMap<u32, u32>,
    pub c2: HashMap<u64, u32>,
    pub c2c: HashMap<u32, u32>,
    pub c3: HashMap<u64, u32>,
    pub c3c: HashMap<u64, u32>,
}

impl Ngram {
    /// Insert a TS-style key ("1|x", "1|", "2|bx", "2c|b", "3|abx", "3c|ab").
    pub fn insert_key(&mut self, key: &str, v: u32) -> Result<(), String> {
        let (kind, rest) = key.split_once('|').ok_or_else(|| format!("bad key {key:?}"))?;
        let c: Vec<char> = rest.chars().collect();
        match (kind, c.len()) {
            ("1", 0) => self.total = Some(v as f64),
            ("1", 1) => { self.c1.insert(c[0] as u32, v); }
            ("2", 2) => { self.c2.insert(k2(c[0], c[1]), v); }
            ("2c", 1) => { self.c2c.insert(c[0] as u32, v); }
            ("3", 3) => { self.c3.insert(k3(c[0], c[1], c[2]), v); }
            ("3c", 2) => { self.c3c.insert(k2(c[0], c[1]), v); }
            _ => return Err(format!("bad key {key:?}")),
        }
        Ok(())
    }
    pub fn from_json(v: &serde_json::Value) -> Result<Self, String> {
        let mut m = Ngram { vocab: v["vocab"].as_f64().ok_or("vocab")?, ..Default::default() };
        let l = v["lambdas"].as_array().ok_or("lambdas")?;
        for i in 0..4 { m.lambdas[i] = l[i].as_f64().ok_or("lambda")?; }
        for (k, c) in v["counts"].as_object().ok_or("counts")? { m.insert_key(k, c.as_u64().ok_or("count")? as u32)?; }
        Ok(m)
    }
    /// Mean natural-log probability per character (incl. end marker).
    pub fn mean_log_prob(&self, t: &str) -> f64 {
        let [l3, l2, l1, l0] = self.lambdas;
        let mut s: Vec<char> = Vec::with_capacity(t.len() + 3);
        s.push(BOS); s.push(BOS); s.extend(t.chars()); s.push(EOS);
        let total = self.total.unwrap_or(1.0);
        let mut lp = 0.0;
        for i in 2..s.len() {
            let (a, b, x) = (s[i - 2], s[i - 1], s[i]);
            let c3d = self.c3c.get(&k2(a, b)).copied().unwrap_or(0);
            let c2d = self.c2c.get(&(b as u32)).copied().unwrap_or(0);
            let p3 = if c3d != 0 { self.c3.get(&k3(a, b, x)).copied().unwrap_or(0) as f64 / c3d as f64 } else { 0.0 };
            let p2 = if c2d != 0 { self.c2.get(&k2(b, x)).copied().unwrap_or(0) as f64 / c2d as f64 } else { 0.0 };
            let p1 = self.c1.get(&(x as u32)).copied().unwrap_or(0) as f64 / total;
            lp += (l3 * p3 + l2 * p2 + l1 * p1 + l0 / self.vocab).ln();
        }
        lp / (s.len() - 2) as f64
    }
    /// Natural-log probability that a word starts with `t` (no end marker).
    pub fn log_prob_start(&self, t: &str) -> f64 {
        let [l3, l2, l1, l0] = self.lambdas;
        let total = self.total.unwrap_or(1.0);
        let (mut a, mut b, mut lp) = (BOS, BOS, 0.0);
        for x in t.chars() {
            let c3d = self.c3c.get(&k2(a, b)).copied().unwrap_or(0);
            let c2d = self.c2c.get(&(b as u32)).copied().unwrap_or(0);
            let p3 = if c3d != 0 { self.c3.get(&k3(a, b, x)).copied().unwrap_or(0) as f64 / c3d as f64 } else { 0.0 };
            let p2 = if c2d != 0 { self.c2.get(&k2(b, x)).copied().unwrap_or(0) as f64 / c2d as f64 } else { 0.0 };
            let p1 = self.c1.get(&(x as u32)).copied().unwrap_or(0) as f64 / total;
            lp += (l3 * p3 + l2 * p2 + l1 * p1 + l0 / self.vocab).ln();
            (a, b) = (b, x);
        }
        lp
    }
    pub fn entries(&self) -> usize { self.c1.len() + self.c2.len() + self.c2c.len() + self.c3.len() + self.c3c.len() }
}

impl Ngram {
    /// All entries as TS-style keys (used by the binary packer).
    pub fn to_keys(&self) -> Vec<(String, u32)> {
        let ch = |x: u64| char::from_u32((x & 0x1F_FFFF) as u32).unwrap();
        let mut o = Vec::with_capacity(self.entries() + 1);
        if let Some(t) = self.total { o.push(("1|".to_string(), t as u32)); }
        for (k, v) in &self.c1 { o.push((format!("1|{}", ch(*k as u64)), *v)); }
        for (k, v) in &self.c2 { o.push((format!("2|{}{}", ch(k >> 21), ch(*k)), *v)); }
        for (k, v) in &self.c2c { o.push((format!("2c|{}", ch(*k as u64)), *v)); }
        for (k, v) in &self.c3 { o.push((format!("3|{}{}{}", ch(k >> 42), ch(k >> 21), ch(*k)), *v)); }
        for (k, v) in &self.c3c { o.push((format!("3c|{}{}", ch(k >> 21), ch(*k)), *v)); }
        o.sort(); o
    }
}
