use typethai_core::{corrector::{key_distance, DEFAULT_COSTS}, keymap::{en_to_th, th_to_en}, normalize::normalize_thai, orthography::check_thai, ModelFiles};
fn model() -> Option<ModelFiles> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/model");
    if p.join("model.json").exists() { Some(ModelFiles::load_json_dir(&p).unwrap()) } else { eprintln!("model files missing; skipping"); None }
}
#[test] fn keymap_roundtrip() { assert_eq!(en_to_th("l;ylfu"), "สวัสดี"); assert_eq!(th_to_en("สวัสดี").as_deref(), Some("l;ylfu")); }
#[test] fn orthography() { assert!(check_thai("สวัสดี").is_ok()); assert!(check_thai("ัก").is_err()); }
#[test] fn normalize_sara_am() { assert_eq!(normalize_thai("น\u{0E4D}\u{0E49}\u{0E32}"), "น\u{0E49}\u{0E33}"); }
#[test] fn distance() { assert_eq!(key_distance("สวัสดี", "สวัสดี", &DEFAULT_COSTS), 0.0); assert!(key_distance("สวสดี", "สวัสดี", &DEFAULT_COSTS) > 0.0); }
#[test] fn detector_and_pack() {
    let Some(m) = model() else { return };
    let det = m.rules_detector();
    let (out, _) = det.convert("l;ylfu ihbjk"); assert!(out.starts_with("สวัสดี"), "{out}");
    let (out, _) = det.convert("the quick brown fox jumps over the lazy dog"); assert_eq!(out, "the quick brown fox jumps over the lazy dog");
    let b = m.pack(); let back = ModelFiles::unpack(&b).unwrap();
    assert_eq!(back.rules_detector().convert("l;ylfu ihbjk").0, det.convert("l;ylfu ihbjk").0);
    assert_eq!(back.thai_words.len(), m.thai_words.len()); assert_eq!(back.wordfreq.len(), m.wordfreq.len());
}
#[test] fn personal_roundtrip() {
    let Some(m) = model() else { return };
    use typethai_core::personal::*;
    let det = m.learn_detector(); let ds = det.convert("l;ylfu ihbjk").1;
    let mut pm = PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL);
    for _ in 0..5 { pm.feedback(&ds, 0, Feedback::Undo); }
    let j = pm.to_json(); let pm2 = PersonalModel::from_json(m.classifier.clone(), &j).unwrap();
    assert_eq!(pm.line_probs(&ds), pm2.line_probs(&ds)); assert!(pm.line_probs(&ds)[0] < m.classifier.line_probs(&ds)[0]);
}
