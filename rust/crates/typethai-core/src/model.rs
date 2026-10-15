//! Model loading: the TS JSON files in data/model/, or the compact "TTM1" binary pack.
//! TTM1 layout: b"TTM1" then a raw-deflate stream of sections `[tag u8][len u32 LE][bytes]`.
//!   1 PARAMS(json)  2 NG_TH  3 NG_EN  4 WORDS_TH  5 WORDS_EN  6 BRANDS (newline-joined UTF-8)
//!   7 WORDFREQ([n u32]{[len u16][utf8][f64]})  8 CLASSIFIER(json)  9 CORRECTOR(json)  10 COLLAPSE(u8)
//!   11 EN_FREQ (as WORDFREQ; optional, written only when non-empty)
//!   NG_* = [vocab f64][4×lambda f64][n u32]{[len u8][key utf8][count u32]}
use crate::{classifier::FlipModel, completer::Completer, corrector::{Corrector, CorrectorParams}, detector::{Detector, Params}, dict::Dict, ngram::Ngram};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub struct ModelFiles {
    pub thai: Ngram, pub english: Ngram, pub thai_words: Vec<String>, pub english_words: Vec<String>, pub brands: Vec<String>,
    pub collapse: bool, pub params: Params, pub wordfreq: HashMap<String, f64>, pub classifier: FlipModel, pub corrector: CorrectorParams, pub w_norm: f64,
    /// English word -> corpus count, for completion only.
    pub english_freq: HashMap<String, f64>,
}
fn rd(p: &Path) -> Result<Value, String> { serde_json::from_str(&std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?).map_err(|e| e.to_string()) }
fn strs(v: &Value) -> Vec<String> { v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default() }
fn freqs(v: &Value) -> Result<HashMap<String, f64>, String> { Ok(v.as_object().ok_or("expected {word: count}")?.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(0.0))).collect()) }
fn freq_bytes(m: &HashMap<String, f64>) -> Vec<u8> {
    let mut wf: Vec<_> = m.iter().collect(); wf.sort_by(|a, b| a.0.cmp(b.0));
    let mut b = (wf.len() as u32).to_le_bytes().to_vec();
    for (k, v) in wf { b.extend((k.len() as u16).to_le_bytes()); b.extend(k.as_bytes()); b.extend(v.to_le_bytes()); }
    b
}
fn freq_map(b: &[u8]) -> HashMap<String, f64> {
    let n = u32::from_le_bytes(b[..4].try_into().unwrap()); let mut p = 4; let mut m = HashMap::new();
    for _ in 0..n { let l = u16::from_le_bytes([b[p], b[p + 1]]) as usize; m.insert(String::from_utf8_lossy(&b[p + 2..p + 2 + l]).into_owned(), f64::from_le_bytes(b[p + 2 + l..p + 10 + l].try_into().unwrap())); p += 10 + l; }
    m
}

impl ModelFiles {
    pub fn load_json_dir(dir: &Path) -> Result<Self, String> {
        let m = rd(&dir.join("model.json"))?; let c = rd(&dir.join("corrector.json"))?;
        let wf = rd(&dir.join("wordfreq.json"))?; let en = dir.join("enfreq.json");
        Ok(ModelFiles {
            thai: Ngram::from_json(&m["thai"])?, english: Ngram::from_json(&m["english"])?,
            thai_words: strs(&m["thaiWords"]), english_words: strs(&m["englishWords"]), brands: strs(&m["brands"]),
            collapse: m["collapse"].as_bool().unwrap_or(false),
            params: serde_json::from_value(m["params"].clone()).map_err(|e| e.to_string())?,
            wordfreq: freqs(&wf)?,
            english_freq: if en.exists() { freqs(&rd(&en)?)? } else { HashMap::new() },
            classifier: serde_json::from_value(rd(&dir.join("flip-classifier.json"))?).map_err(|e| e.to_string())?,
            corrector: serde_json::from_value(c.clone()).map_err(|e| e.to_string())?, w_norm: c["wNorm"].as_f64().unwrap_or(0.0),
        })
    }
    /// v5 rules detector (bundle params + normalisation + wNorm, guard 4, brands) — the TS `detRules`.
    pub fn rules_detector(&self) -> Detector {
        let mut p = self.params.clone(); p.normalize = Some(true); p.w_norm = Some(self.w_norm);
        Detector { thai: self.thai.clone(), english: self.english.clone(), th_dict: Dict::new(self.thai_words.iter().cloned()),
            en_words: self.english_words.iter().cloned().collect(), brands: self.brands.iter().cloned().collect(), collapse: self.collapse, params: p }
    }
    /// Detector feeding the classifier (TS `detLearn`: brands empty, normalisation on, no wNorm).
    pub fn learn_detector(&self) -> Detector {
        let mut d = self.rules_detector(); d.brands.clear(); d.params.w_norm = self.params.w_norm; d
    }
    pub fn corrector(&self) -> Corrector { Corrector::new(&self.thai_words, self.wordfreq.clone(), self.corrector) }
    pub fn thai_completer(&self) -> Completer { Completer::new(self.thai_words.iter().filter_map(|w| Some((w.clone(), *self.wordfreq.get(w)?)))) }
    pub fn english_completer(&self) -> Completer { Completer::new(self.english_freq.iter().map(|(w, f)| (w.clone(), *f))) }

    /// TTM1 file = b"TTM1" + raw-deflate(sections).
    pub fn pack(&self) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::DeflateEncoder::new(b"TTM1".to_vec(), flate2::Compression::best());
        e.write_all(&self.pack_sections()).unwrap(); e.finish().unwrap()
    }
    fn pack_sections(&self) -> Vec<u8> {
        let mut o = Vec::new();
        let sec = |o: &mut Vec<u8>, tag: u8, b: Vec<u8>| { o.push(tag); o.extend((b.len() as u32).to_le_bytes()); o.extend(b); };
        let mut params = serde_json::to_value(&self.params).unwrap(); params["wNorm_v5"] = self.w_norm.into();
        sec(&mut o, 1, serde_json::to_vec(&params).unwrap());
        for (tag, ng) in [(2u8, &self.thai), (3, &self.english)] {
            let mut b = Vec::new(); b.extend(ng.vocab.to_le_bytes()); for l in ng.lambdas { b.extend(l.to_le_bytes()); }
            let keys = ng.to_keys(); b.extend((keys.len() as u32).to_le_bytes());
            for (k, v) in keys { b.push(k.len() as u8); b.extend(k.as_bytes()); b.extend(v.to_le_bytes()); }
            sec(&mut o, tag, b);
        }
        sec(&mut o, 4, self.thai_words.join("\n").into_bytes()); sec(&mut o, 5, self.english_words.join("\n").into_bytes()); sec(&mut o, 6, self.brands.join("\n").into_bytes());
        sec(&mut o, 7, freq_bytes(&self.wordfreq));
        sec(&mut o, 8, serde_json::to_vec(&self.classifier).unwrap()); sec(&mut o, 9, serde_json::to_vec(&self.corrector).unwrap());
        sec(&mut o, 10, vec![self.collapse as u8]);
        if !self.english_freq.is_empty() { sec(&mut o, 11, freq_bytes(&self.english_freq)); }
        o
    }
    pub fn unpack(buf: &[u8]) -> Result<Self, String> {
        if buf.len() < 4 || &buf[..4] != b"TTM1" { return Err("not a TTM1 file".into()) }
        let mut raw = Vec::new(); use std::io::Read;
        flate2::read::DeflateDecoder::new(&buf[4..]).read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let buf = &raw[..];
        let mut secs: HashMap<u8, &[u8]> = HashMap::new(); let mut i = 0;
        while i < buf.len() {
            let tag = buf[i]; let n = u32::from_le_bytes(buf.get(i + 1..i + 5).ok_or("trunc")?.try_into().unwrap()) as usize;
            secs.insert(tag, buf.get(i + 5..i + 5 + n).ok_or("trunc")?); i += 5 + n;
        }
        let g = |t: u8| secs.get(&t).copied().ok_or(format!("missing section {t}"));
        let f64at = |b: &[u8], p: usize| f64::from_le_bytes(b[p..p + 8].try_into().unwrap());
        let u32at = |b: &[u8], p: usize| u32::from_le_bytes(b[p..p + 4].try_into().unwrap());
        let ng = |b: &[u8]| -> Result<Ngram, String> {
            let mut m = Ngram { vocab: f64at(b, 0), ..Default::default() };
            for k in 0..4 { m.lambdas[k] = f64at(b, 8 + 8 * k); }
            let n = u32at(b, 40); let mut p = 44;
            for _ in 0..n { let l = b[p] as usize; let k = std::str::from_utf8(&b[p + 1..p + 1 + l]).map_err(|e| e.to_string())?; m.insert_key(k, u32at(b, p + 1 + l))?; p += 5 + l; }
            Ok(m)
        };
        let lines = |b: &[u8]| -> Vec<String> { let s = String::from_utf8_lossy(b); if s.is_empty() { vec![] } else { s.split('\n').map(String::from).collect() } };
        let pv: Value = serde_json::from_slice(g(1)?).map_err(|e| e.to_string())?;
        Ok(ModelFiles {
            thai: ng(g(2)?)?, english: ng(g(3)?)?, thai_words: lines(g(4)?), english_words: lines(g(5)?), brands: lines(g(6)?),
            collapse: g(10)?.first() == Some(&1), w_norm: pv["wNorm_v5"].as_f64().unwrap_or(0.0),
            params: serde_json::from_value(pv).map_err(|e| e.to_string())?, wordfreq: freq_map(g(7)?),
            english_freq: secs.get(&11).copied().map(freq_map).unwrap_or_default(),
            classifier: serde_json::from_slice(g(8)?).map_err(|e| e.to_string())?, corrector: serde_json::from_slice(g(9)?).map_err(|e| e.to_string())?,
        })
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        if path.is_dir() { Self::load_json_dir(path) } else { Self::unpack(&std::fs::read(path).map_err(|e| e.to_string())?) }
    }
}
