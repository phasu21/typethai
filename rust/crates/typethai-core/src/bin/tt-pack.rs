//! Pack data/model/*.json into the compact TTM1 binary. usage: tt-pack <model dir> <out.ttm>
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = typethai_core::ModelFiles::load_json_dir(std::path::Path::new(&a[1])).expect("load");
    let b = m.pack(); std::fs::write(&a[2], &b).unwrap();
    let back = typethai_core::ModelFiles::unpack(&b).unwrap();
    assert_eq!(back.thai.entries(), m.thai.entries()); assert_eq!(back.thai_words, m.thai_words);
    println!("wrote {} ({} bytes)", a[2], b.len());
}
