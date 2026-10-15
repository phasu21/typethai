use std::collections::HashMap;
use typethai_core::{completer::{thai_tail, Completer}, dict::Dict, ModelFiles};
/// The unpacked model dir in the tree, else the `.ttm` in `TYPETHAI_MODEL`.
fn model() -> Option<ModelFiles> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/model");
    if p.join("model.json").exists() { return Some(ModelFiles::load_json_dir(&p).unwrap()) }
    if let Some(t) = std::env::var_os("TYPETHAI_MODEL") { return Some(ModelFiles::load(std::path::Path::new(&t)).unwrap()) }
    eprintln!("model files missing; skipping"); None
}
fn en() -> Completer { Completer::new([("hello", 10.0), ("help", 30.0), ("helmet", 5.0), ("he", 99.0), ("world", 1.0)].map(|(w, f)| (w.to_string(), f))) }
fn words(c: &Completer, p: &str, k: usize) -> Vec<String> { c.complete(p, k).words }

#[test] fn ranks_extensions_by_freq() {
    let c = en();
    assert_eq!(words(&c, "hel", 2), ["help", "hello"]); assert_eq!(c.complete("hel", 2).confidence, 30.0 / 45.0);
    assert_eq!(words(&c, "he", 5), ["help", "hello", "helmet"]); assert_eq!(words(&c, "he", usize::MAX), ["help", "hello", "helmet"]);
    assert_eq!(c.complete("xyz", 3), Default::default());
    assert!(c.extends("hel")); assert!(!c.extends("help"));
}
#[test] fn keeps_typed_case() {
    let c = en();
    assert_eq!(words(&c, "Hel", 1), ["Help"]); assert_eq!(words(&c, "HEL", 1), ["HELP"]); assert_eq!(words(&c, "hEl", 1), ["help"]);
    assert_eq!(words(&c, "H", 2), ["He", "Help"]);
}
#[test] fn ties_break_by_word_and_input_is_cleaned() {
    let c = Completer::new([("tab", 2.0), ("Taa", 1.0), ("taa", 1.0), ("tac", 2.0), ("t x", 9.0), ("tz", 0.0)].map(|(w, f)| (w.to_string(), f)));
    assert_eq!(words(&c, "t", 9), ["taa", "tab", "tac"]);
}
#[test] fn thai_tail_finds_the_incomplete_word() {
    let dict = Dict::new(["ฉัน", "ชอบ", "ภาษา", "ภาษี"].map(String::from));
    let c = Completer::new([("ภาษา".to_string(), 10.0), ("ภาษี".to_string(), 5.0)]);
    assert_eq!(thai_tail("ฉันชอบภาษ", &dict, &c, 1), Some(6));
    assert_eq!(thai_tail("ฉันชอบภา", &dict, &c, 1), Some(6));
    assert_eq!(thai_tail("ภาษ", &dict, &c, 1), Some(0));
    assert_eq!(thai_tail("ฉันชอบภาษ", &dict, &c, 4), None);
    assert_eq!(thai_tail("ฉันชอบบภาษ", &dict, &c, 1), None);
    assert_eq!(words(&c, &"ฉันชอบภาษ".chars().skip(6).collect::<String>(), 3), ["ภาษา", "ภาษี"]);
}
#[test] fn thai_completer_ranks_by_corpus_freq() {
    let Some(m) = model() else { return };
    let c = m.thai_completer();
    assert_eq!(words(&c, "ภาษ", 4), ["ภาษา", "ภาษี", "ภาษาอังกฤษ", "ภาษาไทย"]);
    assert_eq!(words(&c, "ขอบ", 1), ["ขอบคุณ"]); assert_eq!(words(&c, "สวั", 1), ["สวัสดี"]);
}
#[test] fn english_freq_roundtrips_through_pack() {
    let Some(mut m) = model() else { return };
    if std::env::var_os("TYPETHAI_MODEL").is_some() { assert_eq!(m.english_freq.len(), 0, "a pre-EN_FREQ .ttm loads with none"); }
    (m.thai, m.english, m.thai_words, m.wordfreq) = Default::default(); // debug-build deflate of the full model takes ~50 s
    m.english_freq = HashMap::from([("hello".to_string(), 3.0), ("help".to_string(), 7.0)]);
    let back = ModelFiles::unpack(&m.pack()).unwrap();
    assert_eq!(back.english_freq, HashMap::from([("hello".to_string(), 3.0), ("help".to_string(), 7.0)]));
    assert_eq!(words(&back.english_completer(), "Hel", 2), ["Help", "Hello"]);
    m.english_freq.clear();
    assert_eq!(ModelFiles::unpack(&m.pack()).unwrap().english_freq.len(), 0);
}
