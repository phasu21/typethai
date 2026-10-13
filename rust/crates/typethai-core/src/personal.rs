//! On-device personal model (mirrors src/personal.ts); state is JSON-compatible with the TS PersonalState.
use crate::classifier::{context_features, lr_predict, token_features, FlipModel};
use crate::detector::TokenDecision;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonalConfig {
    pub lr: f64, pub l2: f64, pub max_drift: f64, pub mem_weight: f64, pub mem_decay: f64,
    /// `None` = Infinity (JSON null), like the TS store.
    pub mem_override: Option<f64>, #[serde(default)] pub trust: f64, #[serde(default)] pub mem_purity: f64,
}
pub const DEFAULT_PERSONAL: PersonalConfig = PersonalConfig { lr: 0.05, l2: 0.01, max_drift: 1.0, mem_weight: 2.0, mem_decay: 0.999, mem_override: Some(3.0), trust: 0.0, mem_purity: 0.0 };
#[derive(Clone, Copy, Debug, Serialize, Deserialize)] pub struct Mem { pub kept: f64, pub conv: f64, pub t: f64 }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PersonalState { pub version: u32, pub dw: Vec<f64>, pub db: f64, pub mem: BTreeMap<String, Mem>, pub events: f64, pub config: PersonalConfig }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Feedback { Undo, Retype }

fn sig(z: f64) -> f64 { 1.0 / (1.0 + (-z).exp()) }
fn logit(p: f64) -> f64 { (p.max(1e-9) / (1.0 - p).max(1e-9)).ln() }
fn clamp(v: f64, c: f64) -> f64 { (-c).max(c.min(v)) }
/// JS `toLowerCase` approximation (full Unicode lowercase).
fn key(d: &TokenDecision) -> String { d.token.to_lowercase() }

pub struct PersonalModel { pub base: FlipModel, pub state: PersonalState }
impl PersonalModel {
    pub fn new(base: FlipModel, state: Option<PersonalState>, config: PersonalConfig) -> Self {
        let state = state.unwrap_or_else(|| Self::fresh(&base, config));
        PersonalModel { base, state }
    }
    pub fn fresh(base: &FlipModel, config: PersonalConfig) -> PersonalState {
        PersonalState { version: 1, dw: vec![0.0; base.stage2.w.len()], db: 0.0, mem: BTreeMap::new(), events: 0.0, config }
    }
    pub fn reset(&mut self) { self.state = Self::fresh(&self.base, self.state.config); }
    fn mem_net(&self, k: &str) -> f64 {
        match self.state.mem.get(k) { None => 0.0, Some(m) => (m.conv - m.kept) * self.state.config.mem_decay.powf(self.state.events - m.t) }
    }
    fn mem_pure(&self, k: &str) -> bool {
        match self.state.mem.get(k) { None => false, Some(m) => m.conv.max(m.kept) / (m.conv + m.kept).max(1e-9) >= self.state.config.mem_purity }
    }
    fn z2(&self, ds: &[TokenDecision], p1: &[f64], i: usize) -> Vec<f64> {
        let mut x = token_features(&ds[i]); x.extend(context_features(p1, i));
        let s = &self.base.stage2; x.iter().enumerate().map(|(k, v)| (v - s.mu[k]) / s.sd[k]).collect()
    }
    fn p1(&self, ds: &[TokenDecision]) -> Vec<f64> { ds.iter().map(|d| lr_predict(&self.base.stage1, &token_features(d))).collect() }
    pub fn line_probs(&self, ds: &[TokenDecision]) -> Vec<f64> {
        let p1 = self.p1(ds); let s2 = &self.base.stage2; let c = &self.state.config;
        ds.iter().enumerate().map(|(i, d)| {
            if !(d.valid && d.convertible()) { return 0.0 }
            let z = self.z2(ds, &p1, i);
            let mut a = s2.b + self.state.db; for k in 0..z.len() { a += (s2.w[k] + self.state.dw[k]) * z[k]; }
            let kk = key(d); let net = self.mem_net(&kk);
            if net.abs() >= c.mem_override.unwrap_or(f64::INFINITY) && self.mem_pure(&kk) { return if net > 0.0 { 1.0 } else { 0.0 } }
            sig(a + c.mem_weight * (net / 2.0).tanh())
        }).collect()
    }
    pub fn line_flips(&self, ds: &[TokenDecision]) -> Vec<bool> { self.line_probs(ds).into_iter().map(|p| p > self.base.threshold).collect() }
    pub fn feedback(&mut self, ds: &[TokenDecision], i: usize, kind: Feedback) {
        let y = if kind == Feedback::Retype { 1.0 } else { 0.0 }; let c = self.state.config;
        if ds[i].valid && ds[i].convertible() {
            let p1 = self.p1(ds); let z = self.z2(ds, &p1, i); let s2 = &self.base.stage2;
            let mut a = s2.b + self.state.db; for k in 0..z.len() { a += (s2.w[k] + self.state.dw[k]) * z[k]; }
            let p = sig(a - logit(self.base.threshold));
            let mut ab = s2.b; for k in 0..z.len() { ab += s2.w[k] * z[k]; }
            let pb = sig(ab - logit(self.base.threshold)); let conf = (2.0 * pb - 1.0).abs();
            let g = (p - y) * (1.0 - c.trust * conf);
            for k in 0..z.len() { self.state.dw[k] = clamp(self.state.dw[k] - c.lr * (g * z[k] + c.l2 * self.state.dw[k]), c.max_drift); }
            self.state.db = clamp(self.state.db - c.lr * (g + c.l2 * self.state.db), c.max_drift);
        }
        self.state.events += 1.0; let ev = self.state.events;
        let m = self.state.mem.entry(key(&ds[i])).or_insert(Mem { kept: 0.0, conv: 0.0, t: ev });
        let f = c.mem_decay.powf(ev - m.t); m.kept *= f; m.conv *= f; m.t = ev;
        if y > 0.0 { m.conv += 1.0 } else { m.kept += 1.0 }
    }
    pub fn compact(&mut self, min: f64) {
        let (ev, dec) = (self.state.events, self.state.config.mem_decay);
        self.state.mem.retain(|_, m| (m.kept + m.conv) * dec.powf(ev - m.t) >= min);
    }
    pub fn to_json(&self) -> String { serde_json::to_string(&self.state).unwrap() }
    pub fn from_json(base: FlipModel, s: &str) -> Result<Self, String> {
        let st: PersonalState = serde_json::from_str(s).map_err(|e| e.to_string())?; Ok(PersonalModel { base, state: st })
    }
}
