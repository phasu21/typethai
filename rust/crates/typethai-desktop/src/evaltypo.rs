//! `--eval-typo DIR [--slip-rate R] [--seed N] [--per-file N]`: does a slipped key still reach the word meant?
//! Every `DIR/*.txt` (held-out text, never counted into the model) is split into word runs; from each line, in a
//! seeded shuffle, one word of 4+ chars that has a word before it is a target, up to `--per-file` per file and
//! language. Each target gets one slip, drawn as `MIX` says, and is scored with the keys read as typed (before) and
//! through slips (after, `Options::slips`, `--slip-rate` overriding its rate), with the word before as context and a
//! fresh word memory:
//! - mid-word: the first N keys typed, N = 3, 4, 5, when the slip is among them and the word goes on after them.
//!   Hit: the word is in the top 3 completions (Pause mode, ungated). Clean rows: the same for the word typed right.
//!   Thai is typed glued to the word before, as Thai runs are, and completed from its tail like the engine does.
//! - finished: the whole slipped word, as the rows after Space offer it. English: the spell checker's first 3 guesses
//!   (before) against `en_items`; Thai: the corrector rows (before) against `typo_items`. Also how often it gets rows
//!   at all (a slip that makes another real word is not flagged), and `Completer::correct` alone, ungated.
//! - typed right, by words the completer knows and words it lacks: how often the Typing-mode gate would show a wrong
//!   word on top, and how often (after) a row is a word that does not extend the keys.
use crate::app::{App, Cli};
use crate::engine::{Engine, InputLang};
use std::collections::BTreeMap;
use typethai_core::{completer::thai_tail, corpus::{word_runs, Lang}, keymap::neighbours, slip::Slips};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind { Near, Drop, Double, Extra, Swap }
/// The slips injected and their shares: a neighbouring key instead of the meant one, a key left out, a key typed twice,
/// a neighbour typed after a key, two keys swapped.
const MIX: [(Kind, f64); 5] = [(Kind::Near, 0.40), (Kind::Drop, 0.25), (Kind::Double, 0.10), (Kind::Extra, 0.10), (Kind::Swap, 0.15)];
const CUTS: [usize; 3] = [3, 4, 5];

/// splitmix64
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize { (self.next() % n as u64) as usize }
    fn unit(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
}
/// `w` with one slip: the keys typed, the index of the first key that differs, and the kind.
fn inject(w: &[char], rng: &mut Rng) -> Option<(Vec<char>, usize, Kind)> {
    for _ in 0..20 {
        let mut x = rng.unit();
        let kind = MIX.iter().find(|(_, p)| { x -= p; x < 0.0 }).map_or(Kind::Swap, |m| m.0);
        let i = rng.below(w.len());
        let mut t = w.to_vec();
        let nb = neighbours(w[i]);
        let site = match kind {
            Kind::Near if !nb.is_empty() => { t[i] = nb[rng.below(nb.len())]; i }
            Kind::Drop => { t.remove(i); i }
            Kind::Double => { t.insert(i + 1, w[i]); i + 1 }
            Kind::Extra if !nb.is_empty() => { t.insert(i + 1, nb[rng.below(nb.len())]); i + 1 }
            Kind::Swap if i + 1 < w.len() && w[i] != w[i + 1] => { t.swap(i, i + 1); i }
            _ => continue,
        };
        if t != w { return Some((t, site, kind)) }
    }
    None
}

#[derive(Default, Clone, Copy)] struct Hits { n: u64, before: u64, after: u64, extra: u64 }
impl Hits {
    fn add(&mut self, before: bool, after: bool) { self.n += 1; self.before += before as u64; self.after += after as u64 }
    fn pct(a: u64, n: u64) -> String { format!("{:5.1}%", 100.0 * a as f64 / n.max(1) as f64) }
}
#[derive(Default)] struct Tally {
    targets: u64, known: u64,
    clean: BTreeMap<usize, Hits>, typo: BTreeMap<usize, Hits>, finished: Hits,
    /// finished words that get any rows (the spell checker flags them / the corrector does not know them)
    flagged: Hits,
    /// clean prefixes, by known / unknown word: the Typing gate shows a wrong word on top (before, after), and a row
    /// does not extend the keys (after, in `extra`)
    noise: [Hits; 2],
    by_kind: BTreeMap<Kind, (Hits, Hits)>,
}

/// The top 3 completions of `typed` after `prev`, as the engine reads them (Thai: from the tail of the run), whether
/// one does not extend the keys, and the word the Typing gate would show on top.
fn completions(e: &Engine, prev: &str, typed: &str, lang: Lang) -> (Vec<String>, bool, Option<String>) {
    let (head, done) = match lang {
        Lang::En => {
            let Some(comp) = &e.en_comp else { return (vec![], false, None) };
            (String::new(), comp.complete(typed, 3, e.context(Some(prev), InputLang::Latin)))
        }
        Lang::Th => {
            let Some(comp) = &e.thai_comp else { return (vec![], false, None) };
            let run = format!("{prev}{typed}");
            let d = &e.det.th_dict;
            let Some(at) = thai_tail(&run, d, comp, 1, e.opts.slips.as_ref()).filter(|&at| run.chars().count() - at >= 2) else { return (vec![], false, None) };
            let (head, tail) = run.split_at(run.char_indices().nth(at).map_or(run.len(), |(b, _)| b));
            let p = match d.segment(head).last() { Some(w) => d.has(w).then(|| w.to_string()), None => Some(prev.to_string()) };
            (head.to_string(), comp.complete(tail, 3, e.context(p.as_deref(), InputLang::Thai)))
        }
    };
    let keys = match lang { Lang::En => typed.to_ascii_lowercase(), Lang::Th => format!("{prev}{typed}") };
    let rows: Vec<String> = done.words.into_iter().map(|w| format!("{head}{w}")).collect();
    let off = |r: &String| !r.to_ascii_lowercase().starts_with(&keys);
    let gate = rows.first().filter(|_| done.confidence >= e.opts.complete_conf).cloned();
    (rows.clone(), rows.iter().any(off), gate)
}
fn finished(e: &Engine, prev: &str, typed: &str, lang: Lang) -> Vec<String> {
    match lang {
        Lang::En => e.en_items(&format!("{prev} "), typed, Some(prev)).into_iter().map(|b| b.label).collect(),
        Lang::Th => e.typo_items(typed, Some(prev)).into_iter().map(|b| b.label).collect(),
    }
}
fn finished_before(e: &Engine, prev: &str, typed: &str, lang: Lang) -> Vec<String> {
    match lang {
        Lang::En => e.speller.as_ref().and_then(|s| s.guesses(&format!("{prev} "), typed)).unwrap_or_default().into_iter().filter(|g| g != typed).take(3).collect(),
        Lang::Th => finished(e, prev, typed, lang),
    }
}

pub fn run(cli: &Cli, dir: &str) {
    let mut app = App::load(cli).expect("model");
    let e = &mut app.engine;
    let slips = Slips { rate: cli.slip_rate.unwrap_or(Slips::default().rate) };
    let mut files: Vec<_> = std::fs::read_dir(dir).expect("dir").filter_map(|f| f.ok().map(|f| f.path())).filter(|p| p.extension().is_some_and(|x| x == "txt")).collect();
    files.sort();
    println!("slip rate {} | seed {} | per file and language {} | speller {}", slips.rate, cli.seed, cli.per_file, if e.speller.is_some() { "system" } else { "none" });
    println!("injected: {}", MIX.iter().map(|(k, p)| format!("{k:?} {:.0}%", p * 100.0)).collect::<Vec<_>>().join(", "));
    let mut all: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut rng = Rng(cli.seed);
    for f in &files {
        let text = std::fs::read_to_string(f).expect("file");
        let mut lines: Vec<&str> = text.lines().collect();
        for i in (1..lines.len()).rev() { lines.swap(i, rng.below(i + 1)) }
        let mut taken: BTreeMap<&str, usize> = BTreeMap::new();
        for line in lines {
            let mut pairs: Vec<(Lang, String, String)> = word_runs(line, &e.det.th_dict).into_iter()
                .flat_map(|(lang, ws)| ws.windows(2).filter(|p| p[1].chars().count() >= 4).map(|p| (lang, p[0].clone(), p[1].clone())).collect::<Vec<_>>()).collect();
            pairs.retain(|(lang, ..)| taken.get(name(*lang)).copied().unwrap_or(0) < cli.per_file);
            if pairs.is_empty() { continue }
            let (lang, prev, w) = pairs.swap_remove(rng.below(pairs.len()));
            *taken.entry(name(lang)).or_default() += 1;
            let wc: Vec<char> = w.chars().collect();
            let Some((tc, site, kind)) = inject(&wc, &mut rng) else { continue };
            let t: String = tc.iter().collect();
            let tally = all.entry(name(lang)).or_default();
            tally.targets += 1;
            let known = match lang { Lang::En => e.en_comp.as_ref().is_some_and(|c| c.knows(&w)), Lang::Th => e.thai_comp.as_ref().is_some_and(|c| c.knows(&w)) };
            tally.known += known as u64;
            let meant = format!("{}{w}", if lang == Lang::Th { prev.as_str() } else { "" });
            let hit = |rows: &[String]| rows.iter().any(|r| r.eq_ignore_ascii_case(if lang == Lang::Th { &meant } else { &w }));
            for n in CUTS {
                // hit before, hit after, a row does not extend the keys (after), the gate shows a wrong word (before, after)
                let both = |e: &mut Engine, typed: &str| -> (bool, bool, bool, (bool, bool)) {
                    e.opts.slips = None; let (b, _, gb) = completions(e, &prev, typed, lang);
                    e.opts.slips = Some(slips); let (a, off, ga) = completions(e, &prev, typed, lang);
                    let wrong = |g: Option<String>| g.is_some_and(|g| !hit(&[g]));
                    (hit(&b), hit(&a), off, (wrong(gb), wrong(ga)))
                };
                if wc.len() > n {
                    let typed: String = wc[..n].iter().collect();
                    let (b, a, noisy, gate) = both(e, &typed);
                    tally.clean.entry(n).or_default().add(b, a);
                    let x = &mut tally.noise[usize::from(!known)];
                    x.add(gate.0, gate.1); x.extra += noisy as u64;
                }
                if site < n && tc.len() > n && wc.len() > n {
                    let typed: String = tc[..n].iter().collect();
                    let (b, a, ..) = both(e, &typed);
                    tally.typo.entry(n).or_default().add(b, a);
                    if n == 4 { tally.by_kind.entry(kind).or_default().0.add(b, a) }
                }
            }
            let fixed = |rows: &[String]| rows.iter().any(|r| r.eq_ignore_ascii_case(&w));
            e.opts.slips = None; let rb = finished_before(e, &prev, &t, lang);
            e.opts.slips = Some(slips); let ra = finished(e, &prev, &t, lang);
            let (b, a) = (fixed(&rb), fixed(&ra));
            tally.finished.add(b, a);
            tally.flagged.add(!rb.is_empty(), !ra.is_empty());
            let model_only = match lang {
                Lang::En => e.en_comp.as_ref().map_or(vec![], |c| c.correct(&t, 3, e.context(Some(&prev), InputLang::Latin), &[])),
                Lang::Th => e.thai_comp.as_ref().map_or(vec![], |c| c.correct(&t, 3, e.context(Some(&prev), InputLang::Thai), &[])),
            };
            tally.finished.extra += model_only.iter().any(|r| r.eq_ignore_ascii_case(&w)) as u64;
            tally.by_kind.entry(kind).or_default().1.add(b, a);
        }
        eprintln!("{}: {:?}", f.display(), taken);
    }
    let p = Hits::pct;
    for (lang, t) in &all {
        println!("\n== {lang}: {} targets, {} known to the completer", t.targets, p(t.known, t.targets));
        println!("{:<30} {:>6} {:>8} {:>8}", "top-3 hit", "n", "before", "after");
        for (n, h) in &t.clean { println!("{:<30} {:>6} {:>8} {:>8}", format!("typed right, {n} keys"), h.n, p(h.before, h.n), p(h.after, h.n)) }
        for (n, h) in &t.typo { println!("{:<30} {:>6} {:>8} {:>8}", format!("one slip, {n} keys"), h.n, p(h.before, h.n), p(h.after, h.n)) }
        let f = &t.finished;
        println!("{:<30} {:>6} {:>8} {:>8}   (model alone {})", "one slip, finished word", f.n, p(f.before, f.n), p(f.after, f.n), p(f.extra, f.n));
        println!("{:<30} {:>6} {:>8} {:>8}", "  finished word gets rows", t.flagged.n, p(t.flagged.before, t.flagged.n), p(t.flagged.after, t.flagged.n));
        for (k, (mid, fin)) in &t.by_kind { println!("  {:<28} {:>6} {:>8} {:>8}   finished {:>6} {:>8} {:>8}", format!("{k:?}, 4 keys"), mid.n, p(mid.before, mid.n), p(mid.after, mid.n), fin.n, p(fin.before, fin.n), p(fin.after, fin.n)) }
        for (i, x) in t.noise.iter().enumerate() {
            println!("{:<30} {:>6} {:>8} {:>8}   (a row not extending the keys {})", format!("typed right, {}: gate shows a wrong word", ["known", "unknown"][i]), x.n, p(x.before, x.n), p(x.after, x.n), p(x.extra, x.n));
        }
    }
}
fn name(l: Lang) -> &'static str { match l { Lang::En => "en", Lang::Th => "th" } }
