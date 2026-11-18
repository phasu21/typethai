//! `--eval-auto`: how often "Auto-convert when sure" would change text the user meant as typed, and how much of the
//! wrong-layout typing it catches, per level and over a grid of rules. Everything is built from the model's own word
//! lists and the tricky lines in `eval/` (the Wikipedia splits `--eval-live` reads are not needed), with a fresh
//! personal model, so the numbers are the same on every machine with the same `.ttm`.
//!
//! Forward (English layout): negatives are the top English words (in lines of 8, and the top 3,000 alone on a line) and
//! `eval/auto-english.txt` (short words, chat slang, names, brands, acronyms, code, addresses, sentences); positives are
//! the top Thai words typed as their Kedmanee keys (lines of 6), two-word unspaced runs of them, and `eval/auto-thai.txt`.
//! Reverse (Thai layout): negatives are the same Thai text typed on the Thai layout, positives the top English words.
use crate::app::Cli;
use crate::engine::{looks_like_address, AutoLevel, AutoRule, AutoScore, Boundary, CompleteMode, Engine, Key, Options, Action};
use typethai_core::personal::{PersonalModel, DEFAULT_PERSONAL};
use typethai_core::reverse::{th_text_to_keys, Reverse};

const ENGLISH: &str = include_str!("../eval/auto-english.txt");
const THAI: &str = include_str!("../eval/auto-thai.txt");
/// How many words of each frequency list are used.
const TOP: usize = 20_000;

/// One word as typed, with what the rule reads for it.
struct Tok { typed: String, score: AutoScore }
struct Set { name: &'static str, rev: bool, positive: bool, lines: Vec<Vec<String>> }

fn fixture(text: &str) -> Vec<Vec<String>> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(|l| l.split_whitespace().map(String::from).collect()).collect()
}
fn chunk(words: &[String], n: usize) -> Vec<Vec<String>> { words.chunks(n).map(<[String]>::to_vec).collect() }
fn keys_of(lines: &[Vec<String>]) -> Vec<Vec<String>> {
    lines.iter().map(|l| l.iter().filter_map(|w| th_text_to_keys(w)).collect::<Vec<_>>()).filter(|l| !l.is_empty()).collect()
}

/// The engine the app would run, with a fresh personal model and nothing the user set. The bubble's extras are off
/// (and the typo corrector, which only feeds bubble rows: the rule reads the Thai dictionary), which keeps a run to
/// minutes.
pub(crate) fn engine(cli: &Cli) -> Engine {
    let mut m = typethai_core::ModelFiles::load(&cli.model).unwrap_or_else(|e| panic!("model {}: {e}", cli.model.display()));
    let opts = Options { live: false, complete: CompleteMode::Off, predict: false, next_word: false, typo_suggestions: false, ..Options::default() };
    let mut e = Engine::new(m.learn_detector(), PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL), None, opts);
    e.rev = Some(Reverse::embedded());
    e.load_completers(&mut m);
    e
}

fn sets(cli: &Cli) -> Vec<Set> {
    let m = typethai_core::ModelFiles::load(&cli.model).expect("model");
    let mut en: Vec<(&String, &f64)> = m.english_freq.iter().collect();
    en.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
    let en: Vec<String> = en.into_iter().take(TOP).map(|(w, _)| w.clone()).collect();
    let mut th: Vec<(&String, f64)> = m.thai_words.iter().filter_map(|w| Some((w, *m.wordfreq.get(w)?))).collect();
    th.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(b.0)));
    let th: Vec<String> = th.into_iter().take(TOP).map(|(w, _)| w.clone()).collect();
    let runs: Vec<String> = th[..6000].chunks(2).map(|p| p.concat()).collect();
    let th_lines = chunk(&th, 6); let run_lines = chunk(&runs, 4); let chat = fixture(THAI);
    let en_alpha: Vec<String> = en.iter().filter(|w| w.chars().all(|c| c.is_ascii_alphabetic())).cloned().collect();
    vec![
        Set { name: "English top words (lines of 8)", rev: false, positive: false, lines: chunk(&en, 8) },
        Set { name: "English top 3,000, one per line", rev: false, positive: false, lines: chunk(&en[..3000], 1) },
        Set { name: "Tricky English (eval/auto-english.txt)", rev: false, positive: false, lines: fixture(ENGLISH) },
        Set { name: "Thai top words as keys (lines of 6)", rev: false, positive: true, lines: keys_of(&th_lines) },
        Set { name: "Thai two-word runs as keys", rev: false, positive: true, lines: keys_of(&run_lines) },
        Set { name: "Thai chat lines as keys (eval/auto-thai.txt)", rev: false, positive: true, lines: keys_of(&chat) },
        Set { name: "Thai top words on the Thai layout", rev: true, positive: false, lines: keys_of(&th_lines) },
        Set { name: "Thai two-word runs on the Thai layout", rev: true, positive: false, lines: keys_of(&run_lines) },
        Set { name: "Thai chat lines on the Thai layout", rev: true, positive: false, lines: keys_of(&chat) },
        Set { name: "English top words on the Thai layout", rev: true, positive: true, lines: chunk(&en_alpha, 8) },
    ]
}

/// Type every line of `set` with automatic conversion off and record what the rule would read at each word's end.
fn score(e: &mut Engine, set: &Set) -> Vec<Tok> {
    let mut out = vec![];
    for line in &set.lines {
        e.reset(); e.set_thai_active(set.rev);
        for (i, w) in line.iter().enumerate() {
            for c in w.chars() { e.on_key(Key::Char(c)); }
            out.push(Tok { typed: w.clone(), score: e.auto_score(w) });
            e.on_key(Key::Boundary(if i + 1 == line.len() { Boundary::Enter } else { Boundary::Space }));
            e.set_thai_active(set.rev);
        }
    }
    out
}
fn admits(r: &AutoRule, t: &Tok, rev: bool) -> bool { t.typed.chars().count() >= r.min_keys && !looks_like_address(&t.typed) && r.admits(t.score, rev) }

/// The same lines through the engine with the level on: the conversions it actually makes (the check that the grid's
/// rule is the engine's).
fn live(e: &mut Engine, set: &Set, level: AutoLevel) -> (usize, Vec<String>) {
    e.opts.auto_convert = true; e.opts.auto_level = level; e.auto_safe = true;
    let (mut n, mut ex) = (0, vec![]);
    for line in &set.lines {
        e.reset(); e.set_thai_active(set.rev);
        for (i, w) in line.iter().enumerate() {
            for c in w.chars() { e.on_key(Key::Char(c)); }
            let o = e.on_key(Key::Boundary(if i + 1 == line.len() { Boundary::Enter } else { Boundary::Space }));
            if o.swallow && o.actions.iter().any(|a| matches!(a, Action::Replace { .. })) { n += 1; if ex.len() < 12 { ex.push(w.clone()) } }
            e.set_thai_active(set.rev);
        }
    }
    e.opts.auto_convert = false; e.auto_safe = false;
    (n, ex)
}

pub fn run(cli: &Cli) {
    let mut e = engine(cli);
    let sets = sets(cli);
    let scored: Vec<Vec<Tok>> = sets.iter().map(|s| score(&mut e, s)).collect();
    println!("--eval-auto  model={}", cli.model.display());
    for (s, t) in sets.iter().zip(&scored) { println!("  {:<46} {:>6} words  ({}, {})", s.name, t.len(), if s.rev { "Thai layout" } else { "English layout" }, if s.positive { "should convert" } else { "must not convert" }); }
    let pct = |n: usize, d: usize| 100.0 * n as f64 / d.max(1) as f64;
    // the levels: what the engine does, and examples of what it would get wrong
    println!("\n## Levels (through the engine)\n");
    for level in AutoLevel::ALL {
        let r = level.rule();
        println!("{level:?}: forward p > {}, reverse p > {}, at least {} keys, {}", r.fwd, r.rev, r.min_keys, if r.dict_only { "known words only" } else { "any word" });
        for (s, t) in sets.iter().zip(&scored) {
            let (n, ex) = live(&mut e, s, level);
            let grid = t.iter().filter(|t| admits(&r, t, s.rev)).count();
            let what = if s.positive { "converted" } else { "FALSE" };
            println!("  {:<46} {what} {n:>5} / {:>6} = {:>7.3}%{}{}", s.name, t.len(), pct(n, t.len()),
                if grid != n { format!("  (grid says {grid})") } else { String::new() },
                if !s.positive && n > 0 { format!("  e.g. {}", ex.join(" ")) } else { String::new() });
        }
    }
    // the grid: false conversions per 10,000 words on each negative set, and the share of each positive set caught
    println!("\n## Grid (false conversions per 10,000 words | share converted)\n");
    let neg: Vec<usize> = (0..sets.len()).filter(|&i| !sets[i].positive).collect();
    let pos: Vec<usize> = (0..sets.len()).filter(|&i| sets[i].positive).collect();
    println!("thr    keys dict | {} | {}", neg.iter().map(|&i| format!("N{i}")).collect::<Vec<_>>().join(" "), pos.iter().map(|&i| format!("P{i}")).collect::<Vec<_>>().join(" "));
    for (i, s) in sets.iter().enumerate() { println!("  {}{i} = {}", if s.positive { "P" } else { "N" }, s.name); }
    for thr in [0.9, 0.95, 0.98, 0.99, 0.995, 0.999] { for min_keys in [2, 3, 4] { for dict_only in [true, false] {
        let r = AutoRule { fwd: thr, rev: thr, min_keys, dict_only };
        let cell = |i: usize| { let t = &scored[i]; let n = t.iter().filter(|t| admits(&r, t, sets[i].rev)).count(); (n, t.len()) };
        let negs: Vec<String> = neg.iter().map(|&i| { let (n, d) = cell(i); format!("{:>6.1}", 10_000.0 * n as f64 / d.max(1) as f64) }).collect();
        let poss: Vec<String> = pos.iter().map(|&i| { let (n, d) = cell(i); format!("{:>5.1}%", pct(n, d)) }).collect();
        println!("{thr:<6} {min_keys:>4} {:>4} | {} | {}", if dict_only { "yes" } else { "no" }, negs.join(" "), poss.join(" "));
    } } }
}
