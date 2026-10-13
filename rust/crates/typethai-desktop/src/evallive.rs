//! `--eval-live SPLITS_DIR [--split dev|test]`: prefix replay of the live (mid-word) bubble through the real Engine.
//! Thai: th.<split>.json `toks` (label pos = mistyped Thai, neg = embedded English/numbers, skip = already Thai and
//! bypassed like the Thai-layout check). English prose: en.<split>.json, code: first 5000 lines of code.<split>.json.
//! For every threshold it reports: share of mistyped Thai words shown before their end, keys-before-end (median, p90)
//! at the first show, how often the shown Thai is a prefix of the gold word, and live false shows per 1,000 words.
use crate::{app::{App, Cli}, engine::{Action, BubbleKind, Key, Boundary, Outcome}};
use serde_json::Value;

fn live_shown(o: &Outcome) -> Option<String> {
    o.actions.iter().find_map(|a| match a { Action::ShowBubble(v) => match &v[0].kind { BubbleKind::Live { thai, .. } => Some(thai.clone()), _ => None }, _ => None })
}
/// Feed one token key by key; returns (keystroke index of the first live show (1-based), shown Thai).
fn feed(app: &mut App, tok: &str) -> Option<(usize, String)> {
    let mut first = None;
    for (i, c) in tok.chars().enumerate() {
        let o = app.engine.on_key(Key::Char(c));
        if first.is_none() { if let Some(t) = live_shown(&o) { first = Some((i + 1, t)) } }
    }
    app.engine.on_key(Key::Boundary(Boundary::Space));
    first
}
fn pct(v: &mut [usize], q: f64) -> usize { if v.is_empty() { return 0 } v.sort(); v[((v.len() - 1) as f64 * q).round() as usize] }

pub fn run(cli: &Cli, dir: &str, split: &str) {
    let read = |n: &str| -> Value { serde_json::from_str(&std::fs::read_to_string(format!("{dir}/{n}.{split}.json")).expect("split file")).expect("json") };
    let th = read("th"); let en = read("en"); let code = read("code");
    let lines = |v: &Value, cap: usize| -> Vec<String> { v.as_array().unwrap().iter().take(cap).filter_map(|x| x.as_str().map(String::from)).collect() };
    let en_lines = lines(&en, usize::MAX); let code_lines = lines(&code, 5000);
    println!("split={split}  th lines={}  en lines={}  code lines={} (first 5000)", th.as_array().unwrap().len(), en_lines.len(), code_lines.len());
    println!("thr  stable | thai words  shown-before-end  keys-before-end med/p90  shown-correct | false/1k: en (words)  code (words)  embedded-en (words) | auto-convert unchanged");
    let grid: Vec<(usize, f64)> = if std::env::var_os("TT_EVAL_DEFAULT").is_some() { vec![(2, crate::engine::LIVE_SHOW)] }
        else { [1usize, 2].iter().flat_map(|&s| [0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.98].map(|t| (s, t))).collect() };
    for (stable, thr) in grid { {
        let mut app = App::load(cli).expect("model"); app.engine.opts.live_show = thr; app.engine.opts.live_stable = stable;
        let (mut n_pos, mut shown, mut correct, mut before, mut at) = (0usize, 0usize, 0usize, Vec::new(), Vec::new());
        let (mut n_emb, mut fs_emb) = (0usize, 0usize);
        for line in th.as_array().unwrap() {
            app.engine.reset();
            for t in line["toks"].as_array().into_iter().flatten() {
                let (typed, gold, label) = (t["typed"].as_str().unwrap_or(""), t["gold"].as_str().unwrap_or(""), t["label"].as_str().unwrap_or(""));
                match label {
                    "pos" => { n_pos += 1; if let Some((k, s)) = feed(&mut app, typed) {
                        let len = typed.chars().count(); if k < len { shown += 1; before.push(len - k); at.push(k); if gold.starts_with(&s) { correct += 1 } } } }
                    "neg" => { n_emb += 1; if feed(&mut app, typed).is_some() { fs_emb += 1 } }
                    _ => { app.engine.reset(); }
                }
            }
        }
        let mut prose = |ls: &[String]| { let (mut n, mut f) = (0usize, 0usize); for l in ls { app.engine.reset();
            for w in l.split_whitespace() { n += 1; if feed(&mut app, w).is_some() { f += 1 } } } (n, f) };
        let (n_en, fs_en) = prose(&en_lines); let (n_code, fs_code) = prose(&code_lines);
        let per = |f: usize, n: usize| 1000.0 * f as f64 / n.max(1) as f64;
        let (med, p90) = (pct(&mut before.clone(), 0.5), pct(&mut before, 0.9));
        let (amed, ap90) = (pct(&mut at.clone(), 0.5), pct(&mut at, 0.9));
        println!("{thr:.2} {stable}      | {n_pos:6}  {:5.1}%  {med:3} / {p90:3}  {:5.1}% | {:6.2} ({n_en})  {:6.2} ({n_code})  {:6.2} ({n_emb})",
            100.0 * shown as f64 / n_pos.max(1) as f64, 100.0 * correct as f64 / shown.max(1) as f64, per(fs_en, n_en), per(fs_code, n_code), per(fs_emb, n_emb));
        println!("            keys typed when it first appears: median {amed}, p90 {ap90}");
    } }
}

/// `--eval-live DIR --reverse`: the same replay through the real Engine with the Thai input source active.
/// Thai negatives: wiki (th.<split> gold) + chat (chat/dev.json on dev), typed as their Kedmanee keys (untypeable
/// tokens skipped). Positives: English prose (en.<split>) typed on the Thai layout. Counts every engine Replace, so the
/// retroactive previous-word flip is included.
pub fn run_reverse(cli: &Cli, dir: &str, split: &str) {
    use typethai_core::reverse::th_text_to_keys;
    let rd = |p: String| -> Value { serde_json::from_str(&std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{p}: {e}"))).unwrap() };
    let th_lines = |v: &Value, key: Option<&str>| -> Vec<Vec<String>> { v.as_array().unwrap().iter().filter_map(|x| match key { Some(k) => x[k].as_str(), None => x.as_str() })
        .map(|l| l.split_whitespace().filter_map(th_text_to_keys).collect::<Vec<_>>()).filter(|v| !v.is_empty()).collect() };
    let mut sets = vec![("thai wiki", th_lines(&rd(format!("{dir}/splits/th.{split}.json")), Some("gold")))];
    if split == "dev" { sets.push(("thai chat", th_lines(&rd(format!("{dir}/chat/dev.json")), None))); }
    else { sets.push(("thai tweets (fresh)", th_lines(&rd(format!("{dir}/fresh/tweets_lines.json")), None))); }
    let en: Vec<Vec<String>> = rd(format!("{dir}/splits/en.{split}.json")).as_array().unwrap().iter().filter_map(|x| x.as_str()).map(|l| l.split_whitespace().map(String::from).collect()).collect();
    let mut app = App::load(cli).expect("model");
    // one line through the engine; returns (tokens, tokens covered by a Replace, keystroke of first live show per token)
    let run_line = |app: &mut App, l: &[String]| -> (usize, usize, Vec<Option<usize>>) {
        app.engine.reset(); app.engine.set_thai_active(true);
        let mut flipped = 0usize; let mut first = vec![];
        for (wi, w) in l.iter().enumerate() {
            let mut f = None;
            for (i, c) in w.chars().enumerate() { let o = app.engine.on_key(Key::Char(c)); if f.is_none() && live_shown(&o).is_some() { f = Some(i + 1) } }
            first.push(f);
            let o = app.engine.on_key(if wi + 1 == l.len() { Key::Boundary(Boundary::Enter) } else { Key::Boundary(Boundary::Space) });
            for a in &o.actions { if let Action::Replace { insert, .. } = a { flipped += insert.split_whitespace().count() } }
            app.engine.set_thai_active(true); // a flip would switch the OS to ABC; replay keeps typing on the Thai layout
        }
        (l.len(), flipped, first)
    };
    println!("reverse split={split} (rev_auto {} rev_live {} rev_prev {})", app.engine.opts.rev_auto, app.engine.opts.rev_live, app.engine.opts.rev_prev);
    for (name, ls) in &sets {
        let (mut n, mut f, mut live) = (0, 0, 0);
        for l in ls { let (a, b, c) = run_line(&mut app, l); n += a; f += b; live += c.iter().filter(|x| x.is_some()).count(); }
        println!("  {name}: {n} words, false flips {f} ({:.3}%), live false shows {:.2} per 1,000 words", 100.0 * f as f64 / n.max(1) as f64, 1000.0 * live as f64 / n.max(1) as f64);
    }
    let (mut n, mut f, mut ns, mut fs) = (0, 0, 0, 0); let mut at = vec![]; let mut long = 0;
    for l in &en {
        let (a, b, c) = run_line(&mut app, l); n += a; f += b;
        for (w, x) in l.iter().zip(&c) { let len = w.chars().count(); if w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).chars().count() <= 3 { ns += 1 } if len >= 3 { long += 1; if let Some(k) = x { at.push(*k) } } }
        // per-word short recall needs per-word flips: approximate by re-running word-level below
        let _ = fs;
    }
    // short-word recall at engine level: count words <= 3 chars covered by a flip (replay word by word in line context)
    fs = 0; let mut ns2 = 0;
    for l in &en { app.engine.reset(); app.engine.set_thai_active(true);
        for (wi, w) in l.iter().enumerate() {
            for c in w.chars() { app.engine.on_key(Key::Char(c)); }
            let o = app.engine.on_key(if wi + 1 == l.len() { Key::Boundary(Boundary::Enter) } else { Key::Boundary(Boundary::Space) });
            app.engine.set_thai_active(true);
            let short = w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).chars().count() <= 3;
            if short { ns2 += 1;
                let ins: Vec<String> = o.actions.iter().filter_map(|a| if let Action::Replace { insert, .. } = a { Some(insert.clone()) } else { None }).collect();
                if ins.iter().any(|s| s.split_whitespace().last() == Some(w.as_str())) { fs += 1 }
                // a retro flip of this short word happens at the *next* boundary; counted below via next word's Replace
            }
        }
    }
    let _ = ns;
    let mut at2 = at.clone(); at2.sort(); let q = |v: &[usize], p: f64| if v.is_empty() { 0 } else { v[((v.len() - 1) as f64 * p).round() as usize] };
    println!("  english prose typed on Thai layout: {n} words, flipped {f} ({:.2}% recall)", 100.0 * f as f64 / n.max(1) as f64);
    println!("    words <=3 chars flipped at their own boundary: {fs}/{ns2} ({:.2}%) (retroactive flips at the next boundary not included here)", 100.0 * fs as f64 / ns2.max(1) as f64);
    println!("    live bubble on words >=3 keys: shown on {}/{long} ({:.1}%), at keystroke median {} p90 {}", at.len(), 100.0 * at.len() as f64 / long.max(1) as f64, q(&at2, 0.5), q(&at2, 0.9));
}
