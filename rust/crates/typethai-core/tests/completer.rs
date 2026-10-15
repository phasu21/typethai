use typethai_core::{completer::{thai_tail, Completer}, dict::Dict};
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
