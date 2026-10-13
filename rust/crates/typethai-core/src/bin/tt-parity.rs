//! Parity harness: compare Rust decisions against TS dumps from scripts/parity-dump.ts.
//! usage: tt-parity <model dir|.ttm> <data/parity dir>
use serde_json::Value;
use std::collections::BTreeMap;
use typethai_core::ModelFiles;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = ModelFiles::load(std::path::Path::new(&a[1])).expect("model");
    let det = m.rules_detector(); let learn = m.learn_detector(); let corr = m.corrector(); let thr = m.classifier.threshold;
    let mut per: BTreeMap<String, [usize; 7]> = BTreeMap::new(); // lines, out==, tokens, flip==, clsflip==, maxscore*1e12, maxprob*1e12
    let (mut max_sd, mut max_pd) = (0f64, 0f64); let mut shown = 0;
    for l in std::fs::read_to_string(format!("{}/lines.jsonl", a[2])).unwrap().lines() {
        let v: Value = serde_json::from_str(l).unwrap(); let line = v["line"].as_str().unwrap();
        let (out, ds) = det.convert(line); let probs = m.classifier.line_probs(&learn.convert(line).1);
        let e = per.entry(v["set"].as_str().unwrap().into()).or_default(); e[0] += 1;
        let same_out = out == v["output"].as_str().unwrap(); if same_out { e[1] += 1 }
        let tf = v["flips"].as_array().unwrap(); let tp = v["probs"].as_array().unwrap(); let ts = v["scores"].as_array().unwrap();
        if tf.len() != ds.len() { if shown < 10 { eprintln!("TOKCOUNT {line:?} ts={} rs={}", tf.len(), ds.len()); shown += 1; } continue }
        for i in 0..ds.len() {
            e[2] += 1;
            if tf[i].as_bool() == Some(ds[i].flip) { e[3] += 1 } else if shown < 10 { eprintln!("FLIP {:?} tok {:?}", line, ds[i].token); shown += 1; }
            let p = tp[i].as_f64().unwrap(); if (p > thr) == (probs[i] > thr) { e[4] += 1 } else if shown < 10 { eprintln!("CLS {:?} {} vs {}", ds[i].token, p, probs[i]); shown += 1; }
            if (p - probs[i]).abs() > 1e-9 && std::env::var("TT_DEBUG").is_ok() { eprintln!("PD {:?} {:?} {} {}", line, ds[i].token, p, probs[i]); }
            max_pd = max_pd.max((p - probs[i]).abs());
            match ts[i].as_f64() { Some(s) => max_sd = max_sd.max((s - ds[i].score).abs()), None => if ds[i].score.is_finite() { max_sd = f64::INFINITY } }
        }
    }
    println!("set\tlines\toutput_identical\ttokens\trule_flip_identical\tclassifier_flip_identical");
    let mut tot = [0usize; 5];
    for (k, e) in &per { println!("{k}\t{}\t{:.4}%\t{}\t{:.4}%\t{:.4}%", e[0], 100.0 * e[1] as f64 / e[0] as f64, e[2], 100.0 * e[3] as f64 / e[2] as f64, 100.0 * e[4] as f64 / e[2] as f64); for i in 0..5 { tot[i] += e[i] } }
    println!("ALL\t{}\t{:.4}%\t{}\t{:.4}%\t{:.4}%", tot[0], 100.0 * tot[1] as f64 / tot[0] as f64, tot[2], 100.0 * tot[3] as f64 / tot[2] as f64, 100.0 * tot[4] as f64 / tot[2] as f64);
    println!("max |score diff| {max_sd:e}   max |prob diff| {max_pd:e}");
    let (mut n, mut norm_eq, mut apply_eq, mut top_eq, mut max_cd) = (0, 0, 0, 0, 0f64);
    for l in std::fs::read_to_string(format!("{}/corrector.jsonl", a[2])).unwrap().lines() {
        let v: Value = serde_json::from_str(l).unwrap(); let w = v["w"].as_str().unwrap(); n += 1;
        let c = corr.suggest(w, 3, &det.thai);
        if c.normalized == v["norm"].as_str().unwrap() { norm_eq += 1 }
        if c.apply.as_deref() == v["apply"].as_str() { apply_eq += 1 } else if shown < 20 { eprintln!("APPLY {w:?} {:?} vs {:?}", v["apply"], c.apply); shown += 1 }
        let top: Vec<&str> = v["top"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect();
        if top == c.suggestions.iter().map(|s| s.word.as_str()).collect::<Vec<_>>() { top_eq += 1 } else if shown < 20 { eprintln!("TOP {w:?} {top:?} vs {:?}", c.suggestions.iter().map(|s| &s.word).collect::<Vec<_>>()); shown += 1 }
        for (x, s) in v["conf"].as_array().unwrap().iter().zip(&c.suggestions) { max_cd = max_cd.max((x.as_f64().unwrap() - s.confidence).abs()) }
    }
    println!("corrector words {n}: normalize identical {:.4}%  apply identical {:.4}%  top-3 list identical {:.4}%  max |conf diff| {max_cd:e}",
        100.0 * norm_eq as f64 / n as f64, 100.0 * apply_eq as f64 / n as f64, 100.0 * top_eq as f64 / n as f64);

    // personal model replay
    use typethai_core::personal::{Feedback, PersonalModel, DEFAULT_PERSONAL};
    let mut cfg = DEFAULT_PERSONAL; cfg.trust = 0.5; cfg.mem_purity = 0.8;
    let mut pm = PersonalModel::new(m.classifier.clone(), None, cfg);
    let (mut ev, mut same, mut tok, mut maxd) = (0, 0, 0, 0f64);
    for l in std::fs::read_to_string(format!("{}/personal.jsonl", a[2])).unwrap().lines() {
        let v: Value = serde_json::from_str(l).unwrap(); let ds = learn.convert(v["line"].as_str().unwrap()).1;
        pm.feedback(&ds, v["i"].as_u64().unwrap() as usize, if v["kind"] == "retype" { Feedback::Retype } else { Feedback::Undo }); ev += 1;
        for (x, p) in v["probs"].as_array().unwrap().iter().zip(pm.line_probs(&ds)) { tok += 1; let x = x.as_f64().unwrap(); if (x > thr) == (p > thr) { same += 1 } maxd = maxd.max((x - p).abs()); }
    }
    pm.compact(0.05);
    let ts: Value = serde_json::from_str(&std::fs::read_to_string(format!("{}/personal-state.json", a[2])).unwrap()).unwrap();
    let dwd = ts["dw"].as_array().unwrap().iter().zip(&pm.state.dw).map(|(x, y)| (x.as_f64().unwrap() - y).abs()).fold(0f64, f64::max);
    let mem_keys_eq = ts["mem"].as_object().unwrap().keys().cloned().collect::<std::collections::BTreeSet<_>>() == pm.state.mem.keys().cloned().collect();
    println!("personal: {ev} feedback events, {tok} token decisions after update, flip identical {:.4}%, max |prob diff| {maxd:e}, max |dw diff| {dwd:e}, mem keys identical {mem_keys_eq} ({} entries)",
        100.0 * same as f64 / tok as f64, pm.state.mem.len());
    // TS state loads in Rust
    PersonalModel::from_json(m.classifier.clone(), &serde_json::to_string(&ts).unwrap()).expect("TS state JSON loads");
}
