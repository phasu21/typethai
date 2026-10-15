use typethai_core::{corpus::{from_tsv, prune, sort, to_tsv, word_runs, Counts, Lang, Pair, Prune}, dict::Dict};

fn dict() -> Dict { Dict::new(["ฉัน", "ชอบ", "กิน", "ข้าว", "ผัด", "ไป", "เที่ยว", "ไปเที่ยว", "ตา", "ตาก", "ลม", "กลม"].map(String::from)) }
fn pairs(v: &[(&str, &str, u64)]) -> Vec<Pair> { v.iter().map(|&(a, b, c)| (a.into(), b.into(), c)).collect() }

#[test] fn segment_prefers_long_words_and_groups_unknown_chars() {
    let d = dict();
    assert_eq!(d.segment("ฉันชอบไปเที่ยว"), ["ฉัน", "ชอบ", "ไปเที่ยว"]);
    assert_eq!(d.segment("ตากลม"), ["ตาก", "ลม"]);
    assert_eq!(d.segment("ฉันกขชอบ"), ["ฉัน", "กข", "ชอบ"]);
    assert_eq!(d.segment("ขฉัน"), ["ข", "ฉัน"]);
}
#[test] fn runs_break_at_punctuation_digits_script_switch_and_unknown_words() {
    let runs = word_runs("ฉันชอบกินข้าวผัด, I like Fried rice 3 times ไป เที่ยว don't stop ฉันชอบกขกินข้าว", &dict());
    let got: Vec<(Lang, Vec<&str>)> = runs.iter().map(|(l, w)| (*l, w.iter().map(String::as_str).collect())).collect();
    assert_eq!(got, [
        (Lang::Th, vec!["ฉัน", "ชอบ", "กิน", "ข้าว", "ผัด"]),
        (Lang::En, vec!["i", "like", "fried", "rice"]),
        (Lang::Th, vec!["ไป", "เที่ยว"]),
        (Lang::Th, vec!["ฉัน", "ชอบ"]),
        (Lang::Th, vec!["กิน", "ข้าว"]),
    ]);
}
#[test] fn counts_adjacent_pairs_within_runs_only() {
    let d = dict(); let mut c = Counts::default();
    for l in ["ฉันชอบกินข้าว", "ฉันชอบ. ชอบกิน", "I like it\nit works", "like it"] { c.add_line(l, &d); }
    let mut th = c.th.into_pairs(); sort(&mut th);
    assert_eq!(th, pairs(&[("กิน", "ข้าว", 1), ("ฉัน", "ชอบ", 2), ("ชอบ", "กิน", 2)]));
    let mut en = c.en.into_pairs(); sort(&mut en);
    assert_eq!(en, pairs(&[("i", "like", 1), ("it", "works", 1), ("like", "it", 2)]));
}
#[test] fn prune_applies_vocab_then_min_count_then_top_followers() {
    let all = pairs(&[("i", "am", 9), ("i", "was", 5), ("i", "want", 7), ("i", "xq", 50), ("to", "be", 2), ("to", "go", 1)]);
    let keep = |_: &str, n: &str| n != "xq";
    assert_eq!(prune(all.clone(), &Prune { min_count: 2, top: 2, keep: &keep }), pairs(&[("i", "am", 9), ("i", "want", 7), ("to", "be", 2)]));
    assert_eq!(prune(all, &Prune { min_count: 1, top: 9, keep: &|_, _| true }).len(), 6);
}
#[test] fn tsv_roundtrips_and_reports_bad_lines() {
    let p = pairs(&[("ฉัน", "ชอบ", 3), ("i", "am", 12)]);
    assert_eq!(to_tsv(&p), "ฉัน\tชอบ\t3\ni\tam\t12\n");
    assert_eq!(from_tsv(&to_tsv(&p)).unwrap(), p);
    assert_eq!(from_tsv("a\tb\t1\na\tb\n"), Err("line 2: expected prev<TAB>next<TAB>count".to_string()));
}
