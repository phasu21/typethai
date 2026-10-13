//! Learned two-stage LR flip classifier inference (mirrors src/classifier.ts).
use crate::detector::{core_of, js_len, TokenDecision};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Lr { pub w: Vec<f64>, pub b: f64, pub mu: Vec<f64>, pub sd: Vec<f64> }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlipModel { pub stage1: Lr, pub stage2: Lr, pub threshold: f64, pub names: Vec<String> }

fn sig(z: f64) -> f64 { 1.0 / (1.0 + (-z).exp()) }
pub fn lr_predict(m: &Lr, x: &[f64]) -> f64 {
    let mut z = m.b;
    for k in 0..x.len() { z += m.w[k] * ((x[k] - m.mu[k]) / m.sd[k]); }
    sig(z)
}
macro_rules! rx { ($pat:expr) => {{ static C: OnceLock<Regex> = OnceLock::new(); C.get_or_init(|| Regex::new($pat).unwrap()) }}; }

/// JS `/(.)\1\1/` operates on UTF-16 code units and `.` excludes line terminators.
fn triple_run(t: &str) -> bool {
    let c: Vec<u16> = t.encode_utf16().collect();
    c.windows(3).any(|w| w[0] == w[1] && w[1] == w[2] && ![0x0A, 0x0D, 0x2028, 0x2029].contains(&w[0]))
}
fn b(x: bool) -> f64 { if x { 1.0 } else { 0.0 } }

pub fn token_features(d: &TokenDecision) -> Vec<f64> {
    let t = d.token.as_str();
    let n0 = js_len(t); let n = if n0 == 0 { 1.0 } else { n0 as f64 };
    let conv = d.ll_th.is_finite() && d.convertible();
    let letters = t.chars().filter(|c| c.is_ascii_alphabetic()).count() as f64;
    let upper = t.chars().filter(|c| c.is_ascii_uppercase()).count() as f64;
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count() as f64;
    let keyp = t.chars().filter(|c| ";',.[]/`\\-=".contains(*c)).count() as f64;
    let other = n - letters - digits - keyp;
    let c = core_of(t); let core: &str = if c.is_empty() { t } else { &c };
    let margin = if conv { d.ll_th - d.ll_en } else { 0.0 };
    vec![
        b(conv), b(d.valid), margin, if conv { d.ll_th } else { 0.0 }, if conv { d.ll_en } else { 0.0 }, d.cov_th, b(d.en_hit), d.norm_edits,
        n.min(20.0) / 20.0, 1.0 / n, b(n <= 2.0), b(n <= 4.0),
        letters / n, upper / n, digits / n, keyp / n, other / n,
        b(rx!(r"^[A-Z][a-z]+$").is_match(core)), b(rx!(r"^[A-Za-z]?[a-z]+([A-Z][a-z]+)+$").is_match(core)), b(js_len(core) >= 2 && rx!(r"^[A-Z0-9]+$").is_match(core)),
        b(rx!(r"^[A-Za-z]+$").is_match(t)), b(rx!(r"^[0-9.,%+\-]+$").is_match(t)), b(t.starts_with(['#', '@'])),
        b(rx!(r"(?i)^(https?:|www\.)|\.(com|net|org|co|th|io)(/|$)").is_match(t)),
        b(rx!(r#"^(?:[:;=8xX][-'^o]?[()\[\]DPpOo3|\\/*$@]+|T[_^.\-]?T|[xX]D+|\^[_.\-]?\^|<3+|5{3,}\+*)$"#).is_match(core)),
        b(rx!(r"^[A-Za-z]{1,6}-?[0-9]+[A-Za-z+]*$").is_match(core)), b(triple_run(t)), b(core != t),
        if d.valid { margin } else { 0.0 },
    ]
}
pub fn context_features(p1: &[f64], i: usize) -> Vec<f64> {
    let win = 2usize;
    let lo = i.saturating_sub(win); let hi = (i + win).min(p1.len() - 1);
    let nb: Vec<f64> = (lo..=hi).filter(|j| *j != i).map(|j| p1[j]).collect();
    let others: Vec<f64> = p1.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, x)| *x).collect();
    let mean = |a: &[f64]| if a.is_empty() { 0.0 } else { a.iter().fold(0.0, |x, y| x + y) / a.len() as f64 };
    vec![
        p1[i], if i > 0 { p1[i - 1] } else { 0.0 }, *p1.get(i + 1).unwrap_or(&0.0),
        if nb.is_empty() { 0.0 } else { nb.iter().cloned().fold(f64::NEG_INFINITY, f64::max) }, mean(&nb), mean(&others),
        others.iter().filter(|x| **x > 0.5).count() as f64 / (others.len().max(1) as f64),
        (p1.len().min(30)) as f64 / 30.0, b(p1.len() == 1),
    ]
}
impl FlipModel {
    pub fn stage1_probs(&self, ds: &[TokenDecision]) -> Vec<f64> { ds.iter().map(|d| lr_predict(&self.stage1, &token_features(d))).collect() }
    pub fn line_probs(&self, ds: &[TokenDecision]) -> Vec<f64> {
        let p1 = self.stage1_probs(ds);
        ds.iter().enumerate().map(|(i, d)| {
            if !(d.valid && d.convertible()) { return 0.0 }
            let mut x = token_features(d); x.extend(context_features(&p1, i));
            lr_predict(&self.stage2, &x)
        }).collect()
    }
    pub fn line_flips(&self, ds: &[TokenDecision]) -> Vec<bool> { self.line_probs(ds).into_iter().map(|p| p > self.threshold).collect() }
}
