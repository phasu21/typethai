//! Spike eval of English real-word fixes (typethai_core::confusion), in the style of tinyfix's bench.
//! usage: tt-confusion-eval <model.ttm> [--beta B] [--margins m1,m2,...] [--recount a.txt,b.txt] [--report M]
//!                          <held-out text file>...
//! Held-out lines are split in two by line number: even lines are "dev" (pick the margin there), odd lines "test".
//!   clean:    every word of the held-out text as written; a row offered on a confusion-set word is a false flag.
//!   injected: in each line with a word that some set lists as a target, one such word (chosen by a hash) is replaced
//!             by a typed word whose set lists it; a hit is a top-1 row with the original word.
//! Two modes: "left" scores with the previous word only (the row offered when the word is finished), "left+1" also with
//! the next word (the row offered one word later). Neighbours are words of the same run (no punctuation between).
//! --recount: also count word pairs in these (training) files for pairs with an apostrophe word (you're, it's...),
//!            which the shipped section 13 drops; kept at count >= 2 and added to the shipped pairs.
//! --no-guard: compare words without stored followers too (ablation of the evidence guard).
//! --shipped: the shipped margins (confusion::MARGIN and MARGINS, with --beta as given), on dev and test.
//! --tune F: per-set margins picked on dev for left+1 (the engine's mode): the margin of each typed word's set, from
//!           --base (default confusion::MARGIN) up, that best trades hits for false flags, at the trade-off that keeps dev
//!           at F false flags per 1,000 words or fewer (only sets whose typed word is seen --min-seen times on dev get
//!           their own margin); prints them as a `MARGINS` table and scores them on test.
//! --report L:R: per-set breakdown on test (false flags, hits) at margin L for left and R for left+1, macro recall,
//!            and the cost of one decision.
use std::collections::{BTreeMap, HashMap};
use typethai_core::{completer::Bigrams, confusion::{Confusions, RealWords, Scorer}, ModelFiles};

#[derive(Clone)]
struct Tok { w: String, cap: bool, start: bool }
/// Runs of words: apostrophes stay inside words (’ becomes '), whitespace separates words, anything else ends the run.
fn runs(line: &str) -> Vec<Vec<Tok>> {
    let (mut out, mut run, mut cur) = (Vec::new(), Vec::<Tok>::new(), String::new());
    let mut start = true;
    let flush = |cur: &mut String, run: &mut Vec<Tok>, start: &mut bool| {
        let w = cur.trim_matches('\'').to_string(); cur.clear();
        if w.is_empty() { return }
        run.push(Tok { cap: w.starts_with(|c: char| c.is_ascii_uppercase()), w: w.to_ascii_lowercase(), start: *start });
        *start = false;
    };
    for c in line.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '\'' => cur.push(c),
            '\u{2019}' => cur.push('\''),
            c if c.is_whitespace() => flush(&mut cur, &mut run, &mut start),
            c => {
                flush(&mut cur, &mut run, &mut start);
                if !run.is_empty() { out.push(std::mem::take(&mut run)) }
                if matches!(c, '.' | '!' | '?' | '"' | '\u{201C}' | ':' | '(') { start = true }
            }
        }
    }
    flush(&mut cur, &mut run, &mut start);
    if !run.is_empty() { out.push(run) }
    out
}
fn hash(s: &str) -> u64 { s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)) }

/// One decision: the gain of the best alternative over the typed word, and whether that alternative is the right word.
#[derive(Clone)]
struct Case { set: String, gain_l: f64, ok_l: bool, gain_r: f64, ok_r: bool, fixable: bool, initial: bool }
/// Per typed word on dev: the gains on clean text, and the injected cases (gain, right word).
type PerSet = (Vec<f64>, Vec<(f64, bool)>);
#[derive(Default)]
struct Split { words: u64, conf_words: u64, clean: Vec<Case>, inj: Vec<Case> }

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let opt = |f: &str| a.iter().position(|x| x == f).map(|i| a[i + 1].clone());
    let beta: f64 = opt("--beta").map_or(200.0, |s| s.parse().unwrap());
    let margins: Vec<f64> = opt("--margins").map_or(vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0], |s| s.split(',').map(|x| x.parse().unwrap()).collect());
    let guard = !a.iter().any(|x| x == "--no-guard");
    let report: Option<Vec<f64>> = opt("--report").map(|s| s.split(':').map(|x| x.parse().unwrap()).collect());
    let mut files = Vec::new();
    let mut i = 2;
    while i < a.len() { if a[i] == "--no-guard" || a[i] == "--shipped" { i += 1 } else if a[i].starts_with("--") { i += 2 } else { files.push(a[i].clone()); i += 1 } }
    let m = ModelFiles::load(std::path::Path::new(&a[1])).expect("model");
    let conf = Confusions::tinyfix();

    let shipped = &m.en_bigrams;
    let merged;
    let mut freq = m.english_freq.clone();
    let bg: &Bigrams = if let Some(rc) = opt("--recount") {
        let mut c: HashMap<(String, String), f64> = HashMap::new();
        let mut uni: HashMap<String, f64> = HashMap::new();
        for f in rc.split(',') {
            for line in std::fs::read_to_string(f).expect(f).lines() {
                for r in runs(line) {
                    for t in &r { *uni.entry(t.w.clone()).or_default() += 1.0 }
                    for p in r.windows(2) {
                        if p[0].w.contains('\'') || p[1].w.contains('\'') { *c.entry((p[0].w.clone(), p[1].w.clone())).or_default() += 1.0 }
                    }
                }
            }
        }
        c.retain(|_, v| *v >= 2.0);
        // apostrophe words are missing from the frequency list: add their counts here, scaled to the list's total
        // over the words both have
        let (mut a_sum, mut b_sum) = (0.0, 0.0);
        for (w, n) in &uni { if let Some(f) = freq.get(w) { a_sum += f; b_sum += n } }
        let k = a_sum / b_sum;
        let mut added = 0;
        for (w, n) in &uni { if w.contains('\'') && *n >= 2.0 && !freq.contains_key(w) { freq.insert(w.clone(), n * k); added += 1 } }
        eprintln!("recount: {} apostrophe pairs, {added} apostrophe words added (scale {k:.1})", c.len());
        merged = Bigrams::new(shipped.iter().map(|(p, n, f)| (p.to_string(), n.to_string(), f)).chain(c.into_iter().map(|((p, n), f)| (p, n, f))));
        &merged
    } else { shipped };
    let sc = Scorer::new(&freq, beta);
    eprintln!("pairs {} (shipped {}), freq words {}, sets {}", bg.len(), shipped.len(), m.english_freq.len(), conf.len());
    let missing: Vec<&str> = conf.words().flat_map(|(t, al)| std::iter::once(t).chain(al.iter().map(String::as_str)))
        .filter(|w| bg.mass(w) == 0.0).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    eprintln!("set words with no stored followers ({}): {}", missing.len(), missing.join(" "));
    let nofreq: Vec<&str> = conf.words().flat_map(|(t, al)| std::iter::once(t).chain(al.iter().map(String::as_str)))
        .filter(|w| bg.mass(w) > 0.0 && !freq.contains_key(*w)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    eprintln!("set words with followers but no frequency ({}): {}", nofreq.len(), nofreq.join(" "));

    // target word -> typed words whose set lists it
    let mut rev: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (t, al) in conf.words() { for x in al { rev.entry(x.as_str()).or_default().push(t) } }
    for v in rev.values_mut() { v.sort() }

    let mut sp = [Split::default(), Split::default()];
    let mut n = 0usize;
    for f in &files {
        for line in std::fs::read_to_string(f).expect(f).lines() {
            let s = &mut sp[n % 2]; n += 1;
            let rs = runs(line);
            // a typed word without a row counts as gain -inf (never offered)
            let decide = |r: &[Tok], i: usize, typed: &str, want: Option<&str>| -> Case {
                let prev = (i > 0).then(|| r[i - 1].w.as_str());
                let next = r.get(i + 1).map(|t| t.w.as_str());
                let g = |nx: Option<&str>| conf.best_with(&sc, bg, prev, typed, nx, guard).map_or((f64::NEG_INFINITY, false), |(b, s)| (s, want == Some(b)));
                let (gl, ol) = g(None); let (gr, or) = g(next);
                let fixable = want.is_some_and(|w| bg.mass(w) > 0.0 && bg.mass(typed) > 0.0);
                Case { set: format!("{typed}>{}", want.unwrap_or("-")), gain_l: gl, ok_l: ol, gain_r: gr, ok_r: or, fixable, initial: i == 0 }
            };
            let mut slots = Vec::new();
            for (ri, r) in rs.iter().enumerate() {
                for (i, t) in r.iter().enumerate() {
                    s.words += 1;
                    let name = t.cap && !t.start;
                    if name { continue }
                    if !conf.alternatives(&t.w).is_empty() { s.conf_words += 1; s.clean.push(decide(r, i, &t.w, None)) }
                    if rev.contains_key(t.w.as_str()) { slots.push((ri, i)) }
                }
            }
            if slots.is_empty() { continue }
            let h = hash(line);
            let (ri, i) = slots[(h % slots.len() as u64) as usize];
            let want = rs[ri][i].w.clone();
            let typos = &rev[want.as_str()];
            let typed = typos[((h >> 32) % typos.len() as u64) as usize];
            let mut r = rs[ri].clone(); r[i].w = typed.to_string();
            s.inj.push(decide(&r, i, typed, Some(&want)));
        }
    }
    for (name, s) in ["dev", "test"].iter().zip(&sp) {
        println!("== {name}: {} words, {} confusion-set words, {} injected", s.words, s.conf_words, s.inj.len());
        // rec: top-1 hits / all injected; fix: hits / injected cases where both words have pairs; prec: hits / rows
        // offered on injected words; all: hits / (those rows + clean false flags scaled to the injected word count)
        println!("{:>6} | {:>8} {:>8} {:>6} {:>6} {:>6} {:>6} | {:>8} {:>8} {:>6} {:>6} {:>6} {:>6}", "margin",
            "L ff/1k", "ff/set1k", "rec", "fix", "prec", "all", "R ff/1k", "ff/set1k", "rec", "fix", "prec", "all");
        let fixable = s.inj.iter().filter(|c| c.fixable).count() as f64;
        for &mg in &margins {
            let ff = |r: bool| s.clean.iter().filter(|c| if r { c.gain_r } else { c.gain_l } >= mg).count() as f64;
            let row = |r: bool| {
                let o: Vec<&Case> = s.inj.iter().filter(|c| if r { c.gain_r } else { c.gain_l } >= mg).collect();
                let h = o.iter().filter(|c| if r { c.ok_r } else { c.ok_l }).count() as f64;
                // a line has one injected word among its words: false flags at the clean rate over the other words
                let other = (s.words as f64 / (n as f64 / 2.0)).max(1.0) * s.inj.len() as f64;
                let ff_inj = ff(r) / s.words as f64 * other;
                format!("{:>8.3} {:>8.2} {:>5.1}% {:>5.1}% {:>5.1}% {:>5.1}%", 1000.0 * ff(r) / s.words as f64, 1000.0 * ff(r) / s.conf_words as f64,
                    100.0 * h / s.inj.len() as f64, 100.0 * h / fixable.max(1.0), 100.0 * h / (o.len() as f64).max(1.0), 100.0 * h / (o.len() as f64 + ff_inj).max(1.0))
            };
            println!("{mg:>6.1} | {} | {}", row(false), row(true));
        }
    }
    let typed_of = |c: &Case| c.set.split('>').next().unwrap().to_string();
    // false flags / 1K words, recall, fixable recall, macro recall, precision under per-typed-word margins (left+1)
    let score = |s: &Split, mg: &dyn Fn(&str) -> f64| -> String {
        let ff = s.clean.iter().filter(|c| c.gain_r >= mg(&typed_of(c))).count() as f64;
        let o: Vec<&Case> = s.inj.iter().filter(|c| c.gain_r >= mg(&typed_of(c))).collect();
        let h = o.iter().filter(|c| c.ok_r).count() as f64;
        let fixable = s.inj.iter().filter(|c| c.fixable).count() as f64;
        let mut st: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for c in &s.inj { let e = st.entry(c.set.as_str()).or_default(); e.0 += 1; if c.ok_r && c.gain_r >= mg(&typed_of(c)) { e.1 += 1 } }
        let big: Vec<_> = st.values().filter(|v| v.0 >= 20).collect();
        let mac = big.iter().map(|v| v.1 as f64 / v.0 as f64).sum::<f64>() / big.len().max(1) as f64;
        format!("ff/1k {:.3} ({} flags)  recall {:.1}%  fixable {:.1}%  macro {:.1}%  precision {:.1}%", 1000.0 * ff / s.words as f64, ff,
            100.0 * h / s.inj.len() as f64, 100.0 * h / fixable.max(1.0), 100.0 * mac, 100.0 * h / (o.len() as f64).max(1.0))
    };
    if a.iter().any(|x| x == "--shipped") {
        let rw = RealWords::new(&m.english_freq);
        for (name, s) in ["dev", "test"].iter().zip(&sp) { println!("== shipped margins, left+1, {name}: {}", score(s, &|t| rw.margin_for(t))) }
        // contractions: injected cases whose typed or wanted word has an apostrophe, and false flags on typed ones
        let s = &sp[1];
        let on = |c: &Case| c.gain_r >= rw.margin_for(&typed_of(c));
        let ap: Vec<&Case> = s.inj.iter().filter(|c| c.set.contains('\'')).collect();
        let hits = ap.iter().filter(|c| c.ok_r && on(c)).count();
        let ff = s.clean.iter().filter(|c| c.set.contains('\'') && on(c)).count();
        let mut st: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for c in &ap { let e = st.entry(c.set.as_str()).or_default(); e.0 += 1; if c.ok_r && on(c) { e.1 += 1 } }
        println!("== shipped margins, left+1, test, contractions: recall {}/{} ({:.1}%), false flags on typed contractions {ff} ({:.3}/1k words); {}", hits, ap.len(),
            100.0 * hits as f64 / ap.len().max(1) as f64, 1000.0 * ff as f64 / s.words as f64,
            st.iter().filter(|v| v.1.0 >= 20).map(|(k, v)| format!("{k} {}/{}", v.1, v.0)).collect::<Vec<_>>().join(", "));
        let mut fl: BTreeMap<String, u64> = BTreeMap::new();
        for c in s.clean.iter().filter(|c| on(c)) { *fl.entry(typed_of(c)).or_default() += 1 }
        let mut fl: Vec<_> = fl.into_iter().collect(); fl.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("== shipped margins, left+1, test, false flags by typed word: {}", fl.iter().take(15).map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "));
    }
    if let Some(budget) = opt("--tune").map(|x| x.parse::<f64>().unwrap()) {
        let base: f64 = opt("--base").map_or(typethai_core::confusion::MARGIN, |x| x.parse().unwrap());
        let dev = &sp[0];
        let min_seen: usize = opt("--min-seen").map_or(0, |x| x.parse().unwrap());
        let grid: Vec<f64> = (0..).map(|k| base + 0.5 * k as f64).take_while(|m| *m <= 20.0).chain([f64::INFINITY]).collect();
        // per typed word: clean gains, and injected (gain, right)
        let mut per: BTreeMap<String, PerSet> = BTreeMap::new();
        for c in &dev.clean { per.entry(typed_of(c)).or_default().0.push(c.gain_r) }
        for c in &dev.inj { per.entry(typed_of(c)).or_default().1.push((c.gain_r, c.ok_r)) }
        let pick = |lambda: f64| -> BTreeMap<String, f64> {
            per.iter().map(|(t, (g, inj))| {
                if g.len() < min_seen { return (t.clone(), base) }
                let val = |m: f64| { let ff = g.iter().filter(|x| **x >= m).count() as f64;
                    let (h, w) = inj.iter().filter(|x| x.0 >= m).fold((0.0, 0.0), |(h, w), x| if x.1 { (h + 1.0, w) } else { (h, w + 1.0) });
                    h - lambda * (ff + w) };
                let best = grid.iter().copied().fold((base, f64::NEG_INFINITY), |b, m| { let v = val(m); if v > b.1 + 1e-9 { (m, v) } else { b } });
                (t.clone(), best.0)
            }).collect()
        };
        let dev_ff = |mg: &BTreeMap<String, f64>| dev.clean.iter().filter(|c| c.gain_r >= *mg.get(&typed_of(c)).unwrap_or(&base)).count() as f64 * 1000.0 / dev.words as f64;
        let (mut lo, mut hi) = (0.0f64, 1000.0f64);
        for _ in 0..60 { let mid = (lo + hi) / 2.0; if dev_ff(&pick(mid)) <= budget { hi = mid } else { lo = mid } }
        let mg = pick(hi);
        let over: Vec<(&String, &f64)> = mg.iter().filter(|(_, m)| **m > base).collect();
        println!("== tune: base {base}, lambda {hi:.3}, {} sets above base", over.len());
        println!("pub const MARGINS: &[(&str, f64)] = &[{}];", over.iter().map(|(t, m)| format!("(\"{t}\", {})", if m.is_finite() { format!("{m:.1}") } else { "f64::INFINITY".into() })).collect::<Vec<_>>().join(", "));
        let f = |t: &str| *mg.get(t).unwrap_or(&base);
        for (name, s) in ["dev", "test"].iter().zip(&sp) { println!("== tuned, left+1, {name}: {}", score(s, &f)); println!("== base {base} only, left+1, {name}: {}", score(s, &|_| base)) }
    }
    if let Some(ms) = report {
        // breakdown on test; margins as "left:left+1" or one value for both
        let s = &sp[1];
        const CLASSIC: &[&str] = &["their>there", "there>their", "they're>their", "they're>there", "their>they're", "there>they're",
            "your>you're", "you're>your", "its>it's", "it's>its", "then>than", "than>then", "to>too", "too>to", "two>to", "to>two",
            "were>where", "where>were", "were>we're", "we're>were", "form>from", "from>form", "lose>loose", "loose>lose",
            "affect>effect", "effect>affect", "weather>whether", "whether>weather", "who's>whose", "whose>who's", "of>off", "off>of",
            "know>no", "no>know", "now>know", "hear>here", "here>hear", "quiet>quite", "quite>quiet", "passed>past", "past>passed"];
        for (r, label) in [(false, "left"), (true, "left+1")] {
            let mg = if r { *ms.last().unwrap() } else { ms[0] };
            let gain = |c: &Case| if r { c.gain_r } else { c.gain_l };
            let ok = |c: &Case| if r { c.ok_r } else { c.ok_l };
            let mut ff: BTreeMap<String, u64> = BTreeMap::new();
            let mut seen: BTreeMap<String, u64> = BTreeMap::new();
            for c in &s.clean { *seen.entry(c.set.trim_end_matches(">-").to_string()).or_default() += 1 }
            for c in s.clean.iter().filter(|c| gain(c) >= mg) { *ff.entry(c.set.trim_end_matches(">-").to_string()).or_default() += 1 }
            let init = s.clean.iter().filter(|c| gain(c) >= mg && c.initial).count();
            let total = s.clean.iter().filter(|c| gain(c) >= mg).count();
            println!("== {label} @ {mg} (test): {init} of {total} false flags are on the first word of a run (no previous word)");
            let mut v: Vec<_> = ff.into_iter().collect(); v.sort_by_key(|x| std::cmp::Reverse(x.1));
            println!("== {label} @ {mg} (test): false flags by typed word (flags/occurrences): {}", v.iter().take(20).map(|(k, n)| format!("{k} {n}/{}", seen[k])).collect::<Vec<_>>().join(", "));
            let mut st: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
            for c in &s.inj { let e = st.entry(c.set.clone()).or_default(); e.0 += 1; if gain(c) >= mg { e.2 += 1; if ok(c) { e.1 += 1 } } }
            let big: Vec<_> = st.iter().filter(|(_, v)| v.0 >= 20).collect();
            let macro_rec = big.iter().map(|(_, v)| v.1 as f64 / v.0 as f64).sum::<f64>() / big.len() as f64;
            println!("== {label} @ {mg} (test): macro recall over {} typed>want pairs with n>=20: {:.1}%", big.len(), 100.0 * macro_rec);
            println!("== {label} @ {mg} (test): classic pairs hits/n: {}", CLASSIC.iter().filter_map(|k| st.get(*k).map(|v| format!("{k} {}/{}", v.1, v.0))).collect::<Vec<_>>().join(", "));
        }
        // cost of one decision with the right word, over the clean confusion-set words
        let mut ctx: Vec<(Option<String>, String, Option<String>)> = Vec::new();
        for f in &files { for line in std::fs::read_to_string(f).unwrap().lines().take(20000) { for r in runs(line) { for (i, w) in r.iter().enumerate() {
            if !conf.alternatives(&w.w).is_empty() { ctx.push(((i > 0).then(|| r[i - 1].w.clone()), w.w.clone(), r.get(i + 1).map(|t| t.w.clone()))) }
        } } } }
        let k = ctx.len();
        let t = std::time::Instant::now();
        for (p, w, nx) in &ctx { std::hint::black_box(conf.best(&sc, bg, p.as_deref(), w, nx.as_deref())); }
        println!("== cost: {:.1} us per checked word ({k} words)", t.elapsed().as_secs_f64() * 1e6 / k as f64);
    }
}
