use typethai_core::{completer::Bigrams, ModelFiles};
/// The `.ttm` in `TYPETHAI_MODEL` (packed before sections 12 and 13 existed when it is /tmp/ttmodel/typethai-en.ttm).
fn model() -> Option<ModelFiles> {
    let Some(t) = std::env::var_os("TYPETHAI_MODEL") else { eprintln!("TYPETHAI_MODEL unset; skipping"); return None };
    Some(ModelFiles::load(std::path::Path::new(&t)).unwrap())
}
fn followers(b: &Bigrams, p: &str) -> Vec<(String, f64)> { b.followers(p).to_vec() }
fn owned(v: &[(&str, f64)]) -> Vec<(String, f64)> { v.iter().map(|&(w, c)| (w.to_string(), c)).collect() }

#[test] fn model_without_bigram_sections_loads() {
    let Some(m) = model() else { return };
    assert_eq!((m.th_bigrams.len(), m.en_bigrams.len()), (0, 0));
    assert_eq!(m.thai_completer().complete("สวั", 1).words, ["สวัสดี"]);
}
#[test] fn bigram_sections_roundtrip_through_pack() {
    let Some(mut m) = model() else { return };
    (m.thai, m.english, m.thai_words, m.wordfreq) = Default::default(); // debug-build deflate of the full model is slow
    m.th_bigrams = Bigrams::new([("ฉัน", "ชอบ", 3.0), ("ฉัน", "จะ", 300.0), ("ชอบ", "กิน", 1.0)].map(|(p, n, c)| (p.into(), n.into(), c)));
    m.en_bigrams = Bigrams::new([("i", "want", 7.0), ("i", "am", 9.0), ("want", "to", 1_000_000.0)].map(|(p, n, c)| (p.into(), n.into(), c)));
    let back = ModelFiles::unpack(&m.pack()).unwrap();
    assert_eq!(followers(&back.th_bigrams, "ฉัน"), owned(&[("จะ", 300.0), ("ชอบ", 3.0)]));
    assert_eq!(followers(&back.th_bigrams, "ชอบ"), owned(&[("กิน", 1.0)]));
    assert_eq!(followers(&back.en_bigrams, "I"), owned(&[("am", 9.0), ("want", 7.0)]));
    assert_eq!(followers(&back.en_bigrams, "want"), owned(&[("to", 1_000_000.0)]));
    assert_eq!((back.th_bigrams.len(), back.en_bigrams.len()), (3, 3));
    assert_eq!(back.english_freq, m.english_freq);
    m.en_bigrams = Bigrams::default();
    let back = ModelFiles::unpack(&m.pack()).unwrap();
    assert_eq!((back.th_bigrams.len(), back.en_bigrams.len()), (3, 0));
}
