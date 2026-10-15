use std::collections::HashMap;
use typethai_core::{completer::{thai_tail, Bigrams, Completer, Context, WordMemory, WORD_CAP}, dict::Dict, ModelFiles};
/// The unpacked model dir in the tree, else the `.ttm` in `TYPETHAI_MODEL`.
fn model() -> Option<ModelFiles> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/model");
    if p.join("model.json").exists() { return Some(ModelFiles::load_json_dir(&p).unwrap()) }
    if let Some(t) = std::env::var_os("TYPETHAI_MODEL") { return Some(ModelFiles::load(std::path::Path::new(&t)).unwrap()) }
    eprintln!("model files missing; skipping"); None
}
fn en() -> Completer { Completer::new([("hello", 10.0), ("help", 30.0), ("helmet", 5.0), ("he", 99.0), ("world", 1.0)].map(|(w, f)| (w.to_string(), f))) }
fn words(c: &Completer, p: &str, k: usize) -> Vec<String> { c.complete(p, k, Context::default()).words }
fn mine(m: &WordMemory) -> Context<'_> { Context { memory: Some(m), ..Context::default() } }

#[test] fn ranks_extensions_by_freq() {
    let c = en();
    assert_eq!(words(&c, "hel", 2), ["help", "hello"]); assert_eq!(c.complete("hel", 2, Context::default()).confidence, 30.0 / 45.0);
    assert_eq!(words(&c, "he", 5), ["help", "hello", "helmet"]); assert_eq!(c.complete("he", 1, Context::default()).confidence, 30.0 / 144.0); assert_eq!(words(&c, "he", usize::MAX), ["help", "hello", "helmet"]);
    assert_eq!(c.complete("xyz", 3, Context::default()), Default::default());
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
    (m.thai, m.english, m.thai_words, m.wordfreq) = Default::default(); // debug-build deflate of the full model takes ~50 s
    m.english_freq = HashMap::from([("hello".to_string(), 3.0), ("help".to_string(), 7.0)]);
    let back = ModelFiles::unpack(&m.pack()).unwrap();
    assert_eq!(back.english_freq, HashMap::from([("hello".to_string(), 3.0), ("help".to_string(), 7.0)]));
    assert_eq!(words(&back.english_completer(), "Hel", 2), ["Help", "Hello"]);
    m.english_freq.clear();
    assert_eq!(ModelFiles::unpack(&m.pack()).unwrap().english_freq.len(), 0);
}
#[test] fn bigrams_list_followers_by_count() {
    let b = Bigrams::new([("I", "want", 3.0), ("i", "was", 5.0), ("i", "Want", 4.0), ("i", "am", 7.0), ("to", "be", 2.0), ("a b", "c", 9.0), ("x", "y", 0.0)]
        .map(|(p, n, f)| (p.to_string(), n.to_string(), f)));
    let f = |p: &str| b.followers(p).collect::<Vec<_>>();
    assert_eq!(f("I"), [("am", 7.0), ("want", 7.0), ("was", 5.0)]);
    assert_eq!(f("to"), [("be", 2.0)]);
    assert_eq!(f("x"), []); assert_eq!(f("a b"), []);
    assert_eq!(b.len(), 4);
}
#[test] fn the_previous_word_reorders_by_bigram() {
    let c = Completer::new([("but", 50.0), ("be", 30.0), ("by", 20.0)].map(|(w, f)| (w.to_string(), f)));
    let b = Bigrams::new([("to", "be", 10.0), ("to", "buy", 2.0)].map(|(p, n, f)| (p.to_string(), n.to_string(), f)));
    let after = |prev| c.complete("b", 3, Context { prev, bigrams: Some(&b), memory: None });
    assert_eq!(words(&c, "b", 3), ["but", "be", "by"]);
    assert_eq!(after(Some("To")).words, ["be", "but", "buy"]);
    assert_eq!(after(Some("to")).confidence, (1.0 * 30.0 / 100.0 + 2.0 * 10.0 / 12.0) / 3.0);
    assert_eq!(after(Some("of")).words, ["but", "be", "by"], "no pairs after this word");
    assert_eq!(after(None).words, ["but", "be", "by"]);
}
#[test] fn finished_words_outrank_corpus_frequency() {
    let c = en();
    let mut m = WordMemory::default();
    m.learn(None, "helmet");
    assert_eq!(c.complete("hel", 3, mine(&m)).words, ["help", "helmet", "hello"], "one finish is a hint");
    m.learn(None, "Helmet"); m.learn(None, "helmet");
    assert_eq!(c.complete("hel", 3, mine(&m)).words, ["helmet", "help", "hello"]);
    assert_eq!(words(&c, "hel", 3), ["help", "hello", "helmet"], "without the memory");
}
#[test] fn a_new_word_is_offered_after_three_finishes() {
    let c = en();
    let mut m = WordMemory::default();
    m.learn(None, "helix"); m.learn(None, "helix");
    assert_eq!(c.complete("hel", 4, mine(&m)).words, ["help", "hello", "helmet"]);
    m.learn(None, "helix");
    assert_eq!(c.complete("hel", 4, mine(&m)).words, ["helix", "help", "hello", "helmet"]);
}
#[test] fn pairs_follow_the_previous_word() {
    let c = en();
    let mut m = WordMemory::default();
    for _ in 0..6 { m.learn(None, "helmet") }
    m.learn(Some("say"), "hello"); m.learn(Some("say"), "hello");
    let after = |prev| c.complete("hel", 3, Context { prev, memory: Some(&m), bigrams: None }).words;
    assert_eq!(after(None), ["helmet", "help", "hello"]);
    assert_eq!(after(Some("say")), ["hello", "helmet", "help"]);
}
#[test] fn memory_round_trips_through_json() {
    let mut m = WordMemory::default();
    for _ in 0..3 { m.learn(None, "helix") }
    m.learn(Some("ฉัน"), "ชอบ"); m.learn(Some("say"), "hello");
    let back = WordMemory::from_json(&m.to_json()).unwrap();
    assert_eq!(back, m);
    assert_eq!(en().complete("hel", 2, mine(&back)).words, ["helix", "help"]);
    assert!(WordMemory::from_json("{\"clock\": 3, \"words\": [").is_err());
}
#[test] fn memory_drops_its_weakest_words_past_the_cap() {
    let mut m = WordMemory::default();
    for _ in 0..5 { m.learn(None, "keep") }
    for i in 0..WORD_CAP { m.learn(None, &format!("w{i}")) }
    assert!(m.count("keep") > 4.5, "five finishes, 4000 words ago");
    assert_eq!((m.count("w0"), m.count("w400"), m.count("w401") > 0.9), (0.0, 0.0, true), "the oldest 401 of 4001 dropped, down to 3600");
}
#[test] fn segment_splits_a_run_into_dictionary_words() {
    let dict = Dict::new(["ฉัน", "ชอบ", "ภาษา", "ไทย", "ภาษาไทย", "มาก"].map(String::from));
    assert_eq!(dict.segment("ฉันชอบภาษาไทยxyzภาษ"), ["ฉัน", "ชอบ", "ภาษาไทย", "xyzภาษ"]);
    assert_eq!(dict.segment("ๆชอบมาก"), ["ๆ", "ชอบ", "มาก"]);
    assert_eq!(dict.segment(""), Vec::<&str>::new());
}
