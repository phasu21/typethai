//! Count adjacent word pairs in plain text, or re-prune a pair file, for the TTM bigram sections.
//! usage: tt-bigrams count <model> <th.tsv> <en.tsv> [opts] <text file|->...
//!        tt-bigrams prune <model> <th|en> <in.tsv> <out.tsv> [opts]
//! opts: --min-count N (default 1)  --top K (followers kept per previous word, default all)
//!       --vocab (next word must be in that language's completion vocabulary, previous word a known word:
//!                Thai dictionary / English completion words or English word list)
//! Thai is segmented over the model's dictionary. Output lines are `prev<TAB>next<TAB>count`.
use std::{collections::HashSet, io::BufRead};
use typethai_core::{corpus::{from_tsv, prune, to_tsv, Counts, Lang, Pair, Prune}, dict::Dict, ModelFiles};

struct Vocab { prev: HashSet<String>, next: HashSet<String> }
fn vocab(m: &ModelFiles, lang: Lang) -> Vocab {
    match lang {
        Lang::Th => Vocab { prev: m.thai_words.iter().cloned().collect(), next: m.thai_words.iter().filter(|w| m.wordfreq.get(*w).is_some_and(|f| *f > 0.0)).cloned().collect() },
        Lang::En => {
            let next: HashSet<String> = m.english_freq.iter().filter(|e| *e.1 > 0.0).map(|e| e.0.to_ascii_lowercase()).collect();
            Vocab { prev: next.iter().cloned().chain(m.english_words.iter().map(|w| w.to_ascii_lowercase())).collect(), next }
        }
    }
}
fn opt<T: std::str::FromStr>(a: &[String], f: &str, d: T) -> T { a.iter().position(|x| x == f).map_or(d, |i| a[i + 1].parse().ok().expect(f)) }
fn write(m: &ModelFiles, a: &[String], lang: Lang, pairs: Vec<Pair>, out: &str) {
    let v = a.iter().any(|x| x == "--vocab").then(|| vocab(m, lang));
    let keep = |p: &str, n: &str| v.as_ref().is_none_or(|v| v.prev.contains(p) && v.next.contains(n));
    let raw = pairs.len();
    let kept = prune(pairs, &Prune { min_count: opt(a, "--min-count", 1), top: opt(a, "--top", usize::MAX), keep: &keep });
    std::fs::write(out, to_tsv(&kept)).expect(out);
    eprintln!("{out}: {} of {raw} pairs, {} occurrences", kept.len(), kept.iter().map(|p| p.2).sum::<u64>());
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let m = ModelFiles::load(std::path::Path::new(&a[2])).expect("model");
    match a[1].as_str() {
        "count" => {
            let dict = Dict::new(m.thai_words.iter().cloned());
            let mut c = Counts::default();
            let (mut files, mut i) = (Vec::new(), 5);
            while i < a.len() {
                match a[i].as_str() { "--min-count" | "--top" => i += 1, "--vocab" => {} f => files.push(f) }
                i += 1;
            }
            for f in files {
                let r: Box<dyn BufRead> = if f == "-" { Box::new(std::io::stdin().lock()) } else { Box::new(std::io::BufReader::new(std::fs::File::open(f).expect(f))) };
                for l in r.lines() { c.add_line(&l.expect(f), &dict); }
                eprintln!("{f}: counted");
            }
            write(&m, &a, Lang::Th, c.th.into_pairs(), &a[3]);
            write(&m, &a, Lang::En, c.en.into_pairs(), &a[4]);
        }
        "prune" => {
            let lang = match a[3].as_str() { "th" => Lang::Th, "en" => Lang::En, l => panic!("language {l}: expected th or en") };
            write(&m, &a, lang, from_tsv(&std::fs::read_to_string(&a[4]).expect("in.tsv")).expect("in.tsv"), &a[5]);
        }
        c => panic!("unknown command {c}: expected count or prune"),
    }
}
