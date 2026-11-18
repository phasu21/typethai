//! Spike tool: typeThai's dict segmentation vs externally pre-segmented text (LATTE), Thai only.
//! ttspike seg   <ttm>                         stdin bench JSONL {id,text} -> {id,tokens} via dict.segment
//! ttspike count <ttm> <out.tsv> <mode> <min> files...   mode: dict | pre | hybrid   (pre/hybrid: tokens split by U+200B)
//! ttspike eval  <ttm> <pairs.tsv|model> <mode> files... mode: dict | pre | gold (gold: bench JSONL with spans)
//! ttspike tail  <ttm> files(bench JSONL)       partial-word cases + thai_tail result, JSONL on stdout
//! ttspike vocab <ttm>                          completer words (Thai, freq>0), one per line
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, Write};
use typethai_core::{completer::{thai_tail, Bigrams, Completer, Context}, corpus::{prune, to_tsv, word_runs, Lang, Prune}, dict::Dict, keymap::is_thai, normalize::normalize_thai, ModelFiles};

const ZW: char = '\u{200B}';

#[derive(Clone, Copy, PartialEq)]
enum Class { Thai, Latin, Space, Break }
fn class(c: char) -> Class {
    match c {
        '\u{0E01}'..='\u{0E3A}' | '\u{0E40}'..='\u{0E4E}' if !matches!(c, '\u{0E2F}' | '\u{0E46}') => Class::Thai,
        'a'..='z' | 'A'..='Z' | '\'' | '\u{2019}' => Class::Latin,
        '\n' | '\r' => Class::Break,
        ZW => Class::Space,
        c if c.is_whitespace() => Class::Space,
        _ => Class::Break,
    }
}

/// Thai word runs of a line whose words are already split by U+200B (mirror of corpus::word_runs, Thai only).
/// hybrid: a token the dictionary lacks is re-segmented with dict.segment instead of ending the run.
fn pre_runs(line: &str, dict: &Dict, hybrid: bool) -> Vec<Vec<String>> {
    let (mut done, mut cur): (Vec<Vec<String>>, Vec<String>) = (vec![], vec![]);
    let end = |cur: &mut Vec<String>, done: &mut Vec<Vec<String>>| { if cur.len() >= 2 { done.push(std::mem::take(cur)) } else { cur.clear() } };
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        let k = class(c);
        let len = rest.find(|x| class(x) != k).unwrap_or(rest.len());
        let (chunk, tail) = rest.split_at(len); rest = tail;
        match k {
            Class::Space => {}
            Class::Thai => {
                let p = normalize_thai(chunk);
                if dict.has(&p) { cur.push(p) }
                else if hybrid { for q in dict.segment(&p) { if dict.has(q) { cur.push(q.to_string()) } else { end(&mut cur, &mut done) } } }
                else { end(&mut cur, &mut done) }
            }
            _ => end(&mut cur, &mut done),
        }
    }
    end(&mut cur, &mut done);
    done
}
fn dict_runs(line: &str, dict: &Dict) -> Vec<Vec<String>> { word_runs(line, dict).into_iter().filter(|r| r.0 == Lang::Th).map(|r| r.1).collect() }
/// Gold words of a bench row joined by U+200B (whitespace kept as is).
fn gold_line(v: &Value) -> String {
    let text: Vec<char> = v["text"].as_str().unwrap().chars().collect();
    let mut out = String::new(); let mut pos = 0;
    for s in v["gold"].as_array().unwrap() {
        let (a, b) = (s[0].as_u64().unwrap() as usize, s[1].as_u64().unwrap() as usize);
        out.extend(&text[pos..a]); if !out.is_empty() { out.push(ZW) }
        out.extend(&text[a..b]); pos = b;
    }
    out.extend(&text[pos..]); out
}
fn runs_of(line: &str, mode: &str, dict: &Dict) -> Vec<Vec<String>> {
    match mode {
        "dict" => dict_runs(line, dict),
        "pre" | "gold" => pre_runs(line, dict, false),
        "hybrid" => pre_runs(line, dict, true),
        m => panic!("mode {m}"),
    }
}
fn lines_of(f: &str, mode: &str) -> Vec<String> {
    let r = std::io::BufReader::new(std::fs::File::open(f).expect(f));
    r.lines().map(|l| l.unwrap()).map(|l| if mode == "gold" { gold_line(&serde_json::from_str(&l).unwrap()) } else { l }).collect()
}

fn seg_tokens(text: &str, dict: &Dict) -> Vec<String> {
    let ch: Vec<char> = text.chars().collect();
    let mut out = vec![]; let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if is_thai(c) && !c.is_numeric() {
            let s = i; while i < ch.len() && is_thai(ch[i]) && !ch[i].is_numeric() { i += 1 }
            let run: String = ch[s..i].iter().collect();
            out.extend(dict.segment(&run).into_iter().map(String::from));
        } else if c.is_ascii_alphabetic() || c.is_numeric() {
            let s = i; let alpha = c.is_ascii_alphabetic();
            while i < ch.len() && (if alpha { ch[i].is_ascii_alphabetic() } else { ch[i].is_numeric() }) { i += 1 }
            out.push(ch[s..i].iter().collect());
        } else { out.push(c.to_string()); i += 1 }
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = ModelFiles::load(std::path::Path::new(&a[2])).expect("model");
    let dict = Dict::new(m.thai_words.iter().cloned());
    let out = std::io::stdout(); let mut out = std::io::BufWriter::new(out.lock());
    match a[1].as_str() {
        "seg" => for l in std::io::stdin().lock().lines() {
            let v: Value = serde_json::from_str(&l.unwrap()).unwrap();
            writeln!(out, "{}", json!({"id": v["id"], "tokens": seg_tokens(v["text"].as_str().unwrap(), &dict)})).unwrap();
        },
        "words" => for w in &m.thai_words { writeln!(out, "{w}").unwrap() },
        "vocab" => for w in &m.thai_words { if m.wordfreq.get(w).is_some_and(|f| *f > 0.0) { writeln!(out, "{w}").unwrap() } },
        "count" => {
            let (path, mode, min): (&str, &str, u64) = (&a[3], &a[4], a[5].parse().unwrap());
            let mut pairs: HashMap<(String, String), u64> = HashMap::new();
            let mut words = 0u64;
            for f in &a[6..] {
                for l in lines_of(f, mode) { for r in runs_of(&l, mode, &dict) { words += r.len() as u64; for p in r.windows(2) { *pairs.entry((p[0].clone(), p[1].clone())).or_default() += 1 } } }
                eprintln!("{f}: counted");
            }
            let prev: HashSet<&str> = m.thai_words.iter().map(String::as_str).collect();
            let next: HashSet<&str> = m.thai_words.iter().filter(|w| m.wordfreq.get(*w).is_some_and(|f| *f > 0.0)).map(String::as_str).collect();
            let raw = pairs.len();
            let v: Vec<(String, String, u64)> = pairs.into_iter().map(|((p, n), c)| (p, n, c)).collect();
            let keep = |p: &str, n: &str| prev.contains(p) && next.contains(n);
            let kept = prune(v, &Prune { min_count: min, top: usize::MAX, keep: &keep });
            std::fs::write(path, to_tsv(&kept)).unwrap();
            eprintln!("{path}: run words {words}, {} of {raw} pairs, {} occurrences", kept.len(), kept.iter().map(|p| p.2).sum::<u64>());
        }
        "eval" => {
            let mode = a[4].as_str();
            let bg = if a[3].ends_with(".tsv") {
                let t = std::fs::read_to_string(&a[3]).unwrap();
                Bigrams::new(t.lines().filter(|l| !l.is_empty()).map(|l| { let f: Vec<&str> = l.split('\t').collect(); (f[0].to_string(), f[1].to_string(), f[2].parse::<f64>().unwrap()) }))
            } else { ModelFiles::load(std::path::Path::new(&a[3])).unwrap().th_bigrams };
            let comp: Completer = m.thai_completer();
            let mut t: HashMap<&str, u64> = HashMap::new();
            let mut cache: HashMap<String, Vec<String>> = HashMap::new();
            for f in &a[5..] { for l in lines_of(f, mode) { for r in runs_of(&l, mode, &dict) { for p in r.windows(2) {
                let (prev, w) = (&p[0], &p[1]);
                *t.entry("pairs").or_default() += 1;
                let fol: Vec<&str> = bg.followers(prev).take(3).map(|x| x.0).collect();
                if !fol.is_empty() { *t.entry("prev_known").or_default() += 1 }
                if fol.first() == Some(&w.as_str()) { *t.entry("next_top1").or_default() += 1 }
                if fol.contains(&w.as_str()) { *t.entry("next_top3").or_default() += 1 }
                if w.chars().count() >= 3 {
                    *t.entry("targets").or_default() += 1;
                    if comp.knows(w) { *t.entry("in_vocab").or_default() += 1 }
                    let pre: String = w.chars().take(2).collect();
                    let uni = cache.entry(pre.clone()).or_insert_with(|| comp.complete(&pre, 3, Context::default()).words);
                    if uni.contains(w) { *t.entry("uni_top3").or_default() += 1 }
                    let s = comp.complete(&pre, 3, Context { prev: Some(prev), bigrams: Some(&bg), ..Context::default() }).words;
                    if s.first() == Some(w) { *t.entry("ship_top1").or_default() += 1 }
                    if s.contains(w) { *t.entry("ship_top3").or_default() += 1 }
                }
            } } } }
            writeln!(out, "{}", json!({"pairs_in_table": bg.len(), "tally": t})).unwrap();
        }
        "tail" => {
            let comp: Completer = m.thai_completer();
            for f in &a[3..] { for l in lines_of(f, "plain") {
                let v: Value = serde_json::from_str(&l).unwrap();
                let text: Vec<char> = v["text"].as_str().unwrap().chars().collect();
                let spans: Vec<(usize, usize)> = v["gold"].as_array().unwrap().iter().map(|s| (s[0].as_u64().unwrap() as usize, s[1].as_u64().unwrap() as usize)).collect();
                let thai_word = |s: &(usize, usize)| text[s.0..s.1].iter().all(|c| is_thai(*c) && !c.is_numeric());
                for k in 1..spans.len() {
                    let w: String = text[spans[k].0..spans[k].1].iter().collect();
                    let wl = w.chars().count();
                    if wl < 4 || !comp.knows(&w) || !thai_word(&spans[k]) { continue }
                    // up to 3 unspaced Thai gold words before it
                    let mut j = k;
                    while j > 0 && k - j < 3 && spans[j - 1].1 == spans[j].0 && thai_word(&spans[j - 1]) { j -= 1 }
                    if j == k { continue }
                    let head: String = text[spans[j].0..spans[k].0].iter().collect();
                    let hl = head.chars().count();
                    let head_words: Vec<String> = (j..k).map(|i| text[spans[i].0..spans[i].1].iter().collect()).collect();
                    for len in [3usize, wl - 1].into_iter().collect::<std::collections::BTreeSet<_>>() {
                        if len >= wl { continue }
                        let part: String = w.chars().take(len).collect();
                        let run = format!("{head}{part}");
                        let at = thai_tail(&run, &dict, &comp, 1, None);
                        let full = format!("{head}{w}");
                        writeln!(out, "{}", json!({"run": run, "head_len": hl, "len": len, "word": w, "head_words": head_words, "head_in_dict": head_words.iter().all(|h| dict.has(h)), "tt_at": at, "dict_seg_full": dict.segment(&full)})).unwrap();
                    }
                }
            } }
        }
        "runtime" => {
            // cases JSONL (len 3 tail cases + prev_short/prev_long from LATTE); a[3] pairs, a[4] prev source: dict|short|long|gold
            let t = std::fs::read_to_string(&a[3]).unwrap();
            let bg = Bigrams::new(t.lines().filter(|l| !l.is_empty()).map(|l| { let f: Vec<&str> = l.split('\t').collect(); (f[0].to_string(), f[1].to_string(), f[2].parse::<f64>().unwrap()) }));
            let comp: Completer = m.thai_completer();
            let mut t: HashMap<&str, u64> = HashMap::new();
            for l in std::io::BufReader::new(std::fs::File::open(&a[5]).unwrap()).lines() {
                let v: Value = serde_json::from_str(&l.unwrap()).unwrap();
                if v["len"].as_u64() != Some(3) { continue }
                let w = v["word"].as_str().unwrap().to_string();
                let head = v["run"].as_str().unwrap().chars().take(v["head_len"].as_u64().unwrap() as usize).collect::<String>();
                let prev: Option<String> = match a[4].as_str() {
                    "dict" => dict.segment(&head).last().filter(|p| dict.has(p)).map(|p| p.to_string()),
                    "gold" => v["head_words"].as_array().unwrap().last().map(|x| x.as_str().unwrap().to_string()).filter(|p| dict.has(p)),
                    k => v[format!("prev_{k}")].as_str().map(|x| normalize_thai(x)).filter(|p| dict.has(p)),
                };
                *t.entry("cases").or_default() += 1;
                let gold_prev = v["head_words"].as_array().unwrap().last().unwrap().as_str().unwrap().to_string();
                if prev.as_deref() == Some(gold_prev.as_str()) { *t.entry("prev_eq_gold").or_default() += 1 }
                let Some(prev) = prev else { continue };
                *t.entry("has_prev").or_default() += 1;
                let fol: Vec<&str> = bg.followers(&prev).take(3).map(|x| x.0).collect();
                if fol.first() == Some(&w.as_str()) { *t.entry("next_top1").or_default() += 1 }
                if fol.contains(&w.as_str()) { *t.entry("next_top3").or_default() += 1 }
                let pre: String = w.chars().take(2).collect();
                let s = comp.complete(&pre, 3, Context { prev: Some(&prev), bigrams: Some(&bg), ..Context::default() }).words;
                if s.first() == Some(&w) { *t.entry("ship_top1").or_default() += 1 }
                if s.contains(&w) { *t.entry("ship_top3").or_default() += 1 }
            }
            writeln!(out, "{}", json!({"pairs": a[3], "prev": a[4], "tally": t})).unwrap();
        }
        "time" => {
            let comp: Completer = m.thai_completer();
            let runs: Vec<String> = std::io::stdin().lock().lines().map(|l| l.unwrap()).collect();
            let mut ts: Vec<f64> = runs.iter().map(|r| { let t = std::time::Instant::now(); std::hint::black_box(dict.segment(r)); std::hint::black_box(thai_tail(r, &dict, &comp, 1, None)); t.elapsed().as_secs_f64() * 1e6 }).collect();
            ts.sort_by(f64::total_cmp);
            writeln!(out, "dict.segment+thai_tail: n {} p50 {:.1}us p90 {:.1}us p99 {:.1}us", ts.len(), ts[ts.len() / 2], ts[ts.len() * 9 / 10], ts[ts.len() * 99 / 100]).unwrap();
        }
        c => panic!("command {c}"),
    }
}
