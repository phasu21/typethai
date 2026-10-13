//! Latency / memory benchmark. usage: tt-bench <model dir|.ttm> <data/parity dir>
use std::time::Instant;
use typethai_core::ModelFiles;
fn rss(k: &str) -> String { std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with(k)).map(|l| l.split_whitespace().nth(1).unwrap().to_string())).unwrap_or("?".into()) }
fn pct(v: &mut Vec<f64>, q: f64) -> f64 { v.sort_by(|a, b| a.partial_cmp(b).unwrap()); v[((v.len() - 1) as f64 * q) as usize] }
fn main() {
    let a: Vec<String> = std::env::args().collect();
    println!("rss before load: {} kB", rss("VmRSS:"));
    let t = Instant::now(); let m = ModelFiles::load(std::path::Path::new(&a[1])).unwrap(); let load = t.elapsed();
    let t = Instant::now(); let det = m.learn_detector(); let rules = m.rules_detector(); let corr = m.corrector(); let build = t.elapsed();
    println!("load {:.1} ms, build detector+corrector {:.1} ms, rss after {} kB (peak {} kB)", load.as_secs_f64() * 1e3, build.as_secs_f64() * 1e3, rss("VmRSS:"), rss("VmHWM:"));
    let lines: Vec<String> = std::fs::read_to_string(format!("{}/lines.jsonl", a[2])).unwrap().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["line"].as_str().unwrap().to_string()).collect();
    let words: Vec<String> = lines.iter().flat_map(|l| l.split_whitespace().map(String::from).collect::<Vec<_>>()).take(20000).collect();
    let mut tw = Vec::new(); for w in &words { let t = Instant::now(); std::hint::black_box(rules.score_token(w)); tw.push(t.elapsed().as_secs_f64() * 1e6); }
    let mut tl = Vec::new(); let mut tc = Vec::new(); let mut ntok = 0usize;
    for l in &lines { let t = Instant::now(); let (_, _ds) = rules.convert(l); tl.push(t.elapsed().as_secs_f64() * 1e6);
        let t = Instant::now(); let ds = det.convert(l).1; let p = m.classifier.line_probs(&ds); std::hint::black_box(p); tc.push(t.elapsed().as_secs_f64() * 1e6); ntok += ds.len(); }
    let cw: Vec<String> = std::fs::read_to_string(format!("{}/corrector.jsonl", a[2])).unwrap().lines().map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["w"].as_str().unwrap().to_string()).collect();
    let mut ts = Vec::new(); for w in &cw { let t = Instant::now(); std::hint::black_box(corr.suggest(w, 3, &rules.thai)); ts.push(t.elapsed().as_secs_f64() * 1e6); }
    let mut pm = typethai_core::personal::PersonalModel::new(m.classifier.clone(), None, typethai_core::personal::DEFAULT_PERSONAL);
    let mut tf = Vec::new(); for l in lines.iter().take(2000) { let ds = det.convert(l).1; if ds.is_empty() { continue } let t = Instant::now(); pm.feedback(&ds, 0, typethai_core::personal::Feedback::Undo); tf.push(t.elapsed().as_secs_f64() * 1e6); }
    for (name, v) in [("score_token (per word)", &mut tw), ("rules convert (per line)", &mut tl), ("classifier detect+lineProbs (per line)", &mut tc), ("corrector suggest (per word)", &mut ts), ("personal feedback (per event)", &mut tf)] {
        let mean = v.iter().sum::<f64>() / v.len() as f64; println!("{name}: n={} mean {mean:.2} us  p50 {:.2}  p99 {:.2}", v.len(), pct(v, 0.5), pct(v, 0.99));
    }
    println!("lines {} tokens {ntok}; personal state json {} bytes; corrector index postings {} ({} kB)", lines.len(), pm.to_json().len(), corr.index_entries(), corr.index_bytes() / 1024);
    println!("final rss {} kB, peak {} kB", rss("VmRSS:"), rss("VmHWM:"));
}
