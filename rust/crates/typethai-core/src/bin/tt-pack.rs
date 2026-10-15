//! Pack a model (data/model/*.json dir or an existing .ttm) into the compact TTM1 binary.
//! usage: tt-pack <model dir|in.ttm> <out.ttm> [--enfreq <freq.txt>] [--thbigrams <pairs.tsv>] [--enbigrams <pairs.tsv>]
//! `--enfreq` sets the English completion frequencies from a `word count` per line list (FrequencyWords en_50k.txt).
//! `--thbigrams` / `--enbigrams` set the word pair sections from `prev<TAB>next<TAB>count` files (`tt-bigrams`).
use std::collections::HashMap;
/// The stems OpenSubtitles leaves when it splits "wasn't" into "wasn" + "t" (not `won`, a word).
const NT_STEMS: [&str; 16] = ["aren", "couldn", "didn", "doesn", "hadn", "hasn", "haven", "isn", "mustn", "needn", "shouldn", "wasn", "weren", "wouldn", "ain", "don"];
fn enfreq(txt: &str) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    for l in txt.lines() {
        let mut it = l.split_whitespace();
        let (Some(w), Some(Ok(c))) = (it.next(), it.next().map(str::parse::<f64>)) else { continue };
        if w.len() >= 2 && w.starts_with(|c: char| c.is_ascii_alphabetic()) && w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') && c > 0.0 && !NT_STEMS.contains(&w.to_ascii_lowercase().as_str()) {
            *m.entry(w.to_ascii_lowercase()).or_default() += c;
        }
    }
    m
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = typethai_core::ModelFiles::load(std::path::Path::new(&a[1])).expect("load");
    let flag = |f: &str| a.iter().position(|x| x == f).map(|i| std::path::Path::new(&a[i + 1]));
    if let Some(p) = flag("--enfreq") { m.english_freq = enfreq(&std::fs::read_to_string(p).expect("enfreq")); }
    if let Some(p) = flag("--thbigrams") { m.th_bigrams = typethai_core::model::bigrams_tsv(p).expect("thbigrams"); }
    if let Some(p) = flag("--enbigrams") { m.en_bigrams = typethai_core::model::bigrams_tsv(p).expect("enbigrams"); }
    let b = m.pack(); std::fs::write(&a[2], &b).unwrap();
    let back = typethai_core::ModelFiles::unpack(&b).unwrap();
    assert_eq!(back.thai.entries(), m.thai.entries()); assert_eq!(back.thai_words, m.thai_words); assert_eq!(back.english_freq, m.english_freq);
    assert_eq!(back.th_bigrams, m.th_bigrams); assert_eq!(back.en_bigrams, m.en_bigrams);
    println!("wrote {} ({} bytes, {} English freqs, {} Thai / {} English word pairs)", a[2], b.len(), m.english_freq.len(), m.th_bigrams.len(), m.en_bigrams.len());
}
