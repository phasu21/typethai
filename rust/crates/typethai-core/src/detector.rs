//! v5 rule-based detector (mirrors src/detector.ts: scoreToken, rawScore, ctxEligibility guard 4, contextFlips).
use crate::{dict::Dict, keymap::en_to_th, ngram::Ngram, normalize::normalize_thai, orthography::check_thai};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    pub w_dict: f64, pub w_en_dict: f64, pub w_short: f64, pub threshold: f64,
    pub w_ctx: Option<f64>, pub ctx_window: Option<i64>, pub ctx_floor: Option<serde_json::Value>,
    pub w_line: Option<f64>, pub line_min_frac: Option<f64>, pub line_min_flips: Option<f64>, pub line_allow_en: Option<bool>,
    pub guard: Option<u8>, pub normalize: Option<bool>, pub w_norm: Option<f64>,
}
impl Params {
    /// ctxFloor may be a number, "-Infinity" (configs.json) or absent.
    pub fn ctx_floor(&self) -> f64 {
        match &self.ctx_floor { Some(serde_json::Value::Number(n)) => n.as_f64().unwrap_or(f64::NEG_INFINITY), _ => f64::NEG_INFINITY }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TokenDecision {
    pub token: String, pub candidate: String, pub flip: bool, pub score: f64, pub valid: bool,
    pub reason: Option<String>, pub ll_th: f64, pub ll_en: f64, pub cov_th: f64, pub en_hit: bool, pub norm_edits: f64,
}
impl TokenDecision { pub fn convertible(&self) -> bool { self.reason.as_deref() != Some("not convertible") } }

pub struct Detector { pub thai: Ngram, pub english: Ngram, pub th_dict: Dict, pub en_words: HashSet<String>, pub brands: HashSet<String>, pub collapse: bool, pub params: Params }

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex { cell.get_or_init(|| Regex::new(pat).expect("regex")) }
macro_rules! rx { ($pat:expr) => {{ static C: OnceLock<Regex> = OnceLock::new(); re(&C, $pat) }}; }

/// JS `String.prototype.length` (UTF-16 code units) for parity with the TS engine.
pub fn js_len(s: &str) -> usize { s.encode_utf16().count() }

pub fn collapse_runs(s: &str, keep: usize) -> String {
    let ch: Vec<char> = s.chars().collect(); let mut o = String::new(); let mut run = 0usize;
    for i in 0..ch.len() {
        run = if i > 0 && ch[i] == ch[i - 1] { run + 1 } else { 1 };
        let mut j = i; while j + 1 < ch.len() && ch[j + 1] == ch[i] { j += 1; }
        let len = j - i + run;
        if len >= 3 && run > keep { continue }
        o.push(ch[i]);
    }
    o
}
const EDGE: &[char] = &['(', ')', '[', ']', '{', '}', '"', '\'', '`', '.', ',', ';', ':', '!', '?', '<', '>'];
pub fn core_of(t: &str) -> String { t.trim_start_matches(EDGE).trim_end_matches(EDGE).to_string() }

pub fn chat_shape(token: &str, core: &str, brands: Option<&HashSet<String>>) -> bool {
    let emo = rx!(r#"^(?:[:;=8xX][-'^o]?[()\[\]DPpOo3|\\/*$@]+|[()\[\]DPp][-']?[:;=]|T[_^.\-]?T|[;:]_+[;:]|;[-_wWoO.];|[xX]D+|[oO0][_.][oO0]|>[_.]<|\^[_.\-]?\^|-[_.]-|<3+|[:;]v|:'\(|555+)$"#);
    let unit = rx!(r"(?i)^[0-9]+(?:[.,][0-9]+)?(?:ml|mg|g|kg|km|cm|mm|k|gb|mb|tb|hp|cc|w|kw|mah|ghz|hz|ms|px|th|st|nd|rd|x)$");
    let model = rx!(r"^[A-Za-z]{1,6}-?[0-9]+[A-Za-z+]*$");
    let url = rx!(r"(?i)^(https?:|www\.)|\.(com|net|org|co|th|io|ly|me)(/|$)");
    if token.starts_with(['#', '@']) || core.starts_with(['#', '@']) { return true }
    if url.is_match(token) { return true }
    if emo.is_match(token) || emo.is_match(core) { return true }
    if unit.is_match(core) || model.is_match(core) { return true }
    if let Some(b) = brands { if b.contains(&core.to_lowercase()) { return true } }
    false
}
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Elig { Ok, EnWord, Never }
pub fn ctx_eligibility(token: &str, en_hit: bool, guard: u8, brands: Option<&HashSet<String>>) -> Elig {
    let c = core_of(token);
    let t: &str = if guard >= 3 { if c.is_empty() { token } else { &c } } else { token };
    if guard >= 4 && chat_shape(token, t, brands) { return Elig::Never }
    if rx!(r"^[0-9.,%\-]+$").is_match(t) { return Elig::Never }
    if rx!(r"^[A-Z][a-z]+$").is_match(t) { return Elig::Never }
    if rx!(r"^[A-Z0-9][A-Za-z0-9]*(-[A-Za-z0-9]+)+$").is_match(t) { return Elig::Never }
    if js_len(t) >= 2 && rx!(r"^[A-Z0-9]+$").is_match(t) { return Elig::Never }
    if guard >= 3 && rx!(r"^[A-Za-z]?[a-z]+([A-Z][a-z]+)+$").is_match(t) { return Elig::Never }
    if en_hit && rx!(r"^[A-Za-z]+$").is_match(token) { return Elig::EnWord }
    Elig::Ok
}
pub struct CtxToken { pub valid: bool, pub score: f64, pub elig: Elig, pub len: usize }

pub fn context_flips(toks: &[CtxToken], p: &Params) -> Vec<bool> {
    let mut flip: Vec<bool> = toks.iter().map(|t| t.valid && t.score > p.threshold).collect();
    let wl = p.w_line.unwrap_or(0.0);
    let mut bonus = vec![0.0; toks.len()];
    if wl > 0.0 {
        let n_flip = flip.iter().filter(|x| **x).count();
        let n_conv = toks.iter().enumerate().filter(|(i, t)| t.score.is_finite() && (flip[*i] || t.len > 2)).count();
        if n_flip as f64 >= p.line_min_flips.unwrap_or(2.0) && n_flip as f64 / (n_conv.max(1) as f64) >= p.line_min_frac.unwrap_or(0.8) {
            for (i, t) in toks.iter().enumerate() { if t.elig == Elig::Ok || (t.elig == Elig::EnWord && p.line_allow_en.unwrap_or(false)) { bonus[i] = wl; } }
            for (i, t) in toks.iter().enumerate() { if !flip[i] && t.valid && t.score + bonus[i] > p.threshold { flip[i] = true; } }
        }
    }
    let w = p.w_ctx.unwrap_or(0.0); let win = p.ctx_window.unwrap_or(1) as isize; let floor = p.ctx_floor();
    if w <= 0.0 { return flip }
    for _ in 0..5 {
        let mut changed = false;
        for i in 0..toks.len() {
            let t = &toks[i];
            if flip[i] || !t.valid || t.elig != Elig::Ok || t.score < floor { continue }
            let mut n = 0;
            let lo = (i as isize - win).max(0) as usize; let hi = ((i as isize + win) as usize).min(toks.len() - 1);
            for j in lo..=hi { if j != i && flip[j] { n += 1 } }
            if n > 0 && t.score + bonus[i] + w * n as f64 > p.threshold { flip[i] = true; changed = true; }
        }
        if !changed { break }
    }
    flip
}

pub fn raw_score(d: &TokenDecision, p: &Params) -> f64 {
    (d.ll_th - d.ll_en) + p.w_dict * d.cov_th - p.w_en_dict * (if d.en_hit { 1.0 } else { 0.0 }) - p.w_short / js_len(&d.token) as f64 - p.w_norm.unwrap_or(0.0) * d.norm_edits
}

impl Detector {
    pub fn score_token(&self, token: &str) -> TokenDecision {
        let raw = en_to_th(token);
        let candidate = if self.params.normalize.unwrap_or(false) && raw != token { normalize_thai(&raw) } else { raw.clone() };
        let all_keys = !token.is_empty() && token.chars().all(|c| ('\x21'..='\x7e').contains(&c));
        let has_latin_key = rx!(r#"[A-Za-z;'\[\],./`\\\-=0-9~!@#$%^&*()_+{}|:"<>?]"#).is_match(token);
        let urlish = rx!(r"(?i)^(https?:|www\.)|@[a-z0-9-]+\.[a-z]").is_match(token);
        if !all_keys || !has_latin_key || raw == token || urlish {
            return TokenDecision { token: token.into(), candidate: raw, flip: false, score: f64::NEG_INFINITY, valid: false, reason: Some("not convertible".into()),
                ll_th: 0.0, ll_en: 0.0, cov_th: 0.0, en_hit: false, norm_edits: 0.0 };
        }
        let v = check_thai(&candidate);
        let th_s = if self.collapse { collapse_runs(&candidate, 1) } else { candidate.clone() };
        let en_s = if self.collapse { collapse_runs(token, 2) } else { token.to_string() };
        let ll_th = self.thai.mean_log_prob(&th_s);
        let ll_en = self.english.mean_log_prob(&en_s);
        let cov_th = self.th_dict.coverage(&th_s);
        let lower = en_s.to_lowercase();
        let stripped = lower.trim_start_matches(|c: char| !c.is_ascii_lowercase()).trim_end_matches(|c: char| !c.is_ascii_lowercase());
        let en_hit = !stripped.is_empty() && self.en_words.contains(stripped);
        let norm_edits = if candidate == raw { 0.0 } else { 1.0 + (raw.chars().count() as f64 - candidate.chars().count() as f64).abs() };
        let mut d = TokenDecision { token: token.into(), candidate, flip: false, score: 0.0, valid: v.is_ok(), reason: v.err(), ll_th, ll_en, cov_th, en_hit, norm_edits };
        d.score = raw_score(&d, &self.params);
        d.flip = d.valid && d.score > self.params.threshold;
        d
    }
    pub fn eligibility(&self, d: &TokenDecision) -> Elig {
        ctx_eligibility(&d.token, d.en_hit, self.params.guard.unwrap_or(3), Some(&self.brands))
    }
    /// Score a line of whitespace-separated tokens and apply context flips; returns decisions with `flip` set.
    pub fn decide_line(&self, tokens: &[&str]) -> Vec<TokenDecision> {
        let mut ds: Vec<TokenDecision> = tokens.iter().map(|t| self.score_token(t)).collect();
        let ctx: Vec<CtxToken> = ds.iter().map(|d| CtxToken { valid: d.valid, score: d.score, elig: self.eligibility(d), len: js_len(&d.token) }).collect();
        let f = context_flips(&ctx, &self.params);
        for (d, x) in ds.iter_mut().zip(f) { d.flip = x; }
        ds
    }
    pub fn convert(&self, line: &str) -> (String, Vec<TokenDecision>) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        let ds = self.decide_line(&toks);
        // rebuild preserving whitespace
        let mut out = String::new(); let mut k = 0; let mut rest = line;
        while !rest.is_empty() {
            let ws = rest.len() - rest.trim_start().len(); out.push_str(&rest[..ws]); rest = &rest[ws..];
            if rest.is_empty() { break }
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push_str(if ds[k].flip { &ds[k].candidate } else { &rest[..end] }); k += 1; rest = &rest[end..];
        }
        (out, ds)
    }
}
