//! Debug: print classifier token features for a line. usage: tt-feat <model> <line>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = typethai_core::ModelFiles::load(std::path::Path::new(&a[1])).unwrap();
    let ds = m.learn_detector().convert(&a[2]).1; println!("{:?} {:?} {} {}", m.classifier.stage1_probs(&ds), m.classifier.line_probs(&ds), m.classifier.stage2.w.len(), m.classifier.stage1.w.len()); for d in ds { println!("{} {:?}", d.token, typethai_core::classifier::token_features(&d)); }
}
