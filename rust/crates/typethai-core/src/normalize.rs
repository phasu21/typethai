//! Generic Thai orthographic normalisation (mirrors src/normalize.ts).
use crate::orthography::{is_above_below as is_vowel_mark, is_cons, is_tone, KARAN};
fn is_mark(c: char) -> bool { is_vowel_mark(c) || is_tone(c) || c == KARAN }

pub fn normalize_thai(s: &str) -> String {
    // เเ -> แ ; ํ[tone]?า -> [tone]ำ
    let s = s.replace("\u{0E40}\u{0E40}", "\u{0E41}");
    let src: Vec<char> = s.chars().collect();
    let mut t: Vec<char> = Vec::with_capacity(src.len());
    let mut i = 0;
    while i < src.len() {
        if src[i] == '\u{0E4D}' {
            if i + 1 < src.len() && src[i + 1] == '\u{0E32}' { t.push('\u{0E33}'); i += 2; continue; }
            if i + 2 < src.len() && is_tone(src[i + 1]) && src[i + 2] == '\u{0E32}' { t.push(src[i + 1]); t.push('\u{0E33}'); i += 3; continue; }
        }
        t.push(src[i]); i += 1;
    }
    let ch = t;
    let mut out: Vec<char> = Vec::with_capacity(ch.len());
    let mut i = 0;
    while i < ch.len() {
        let c = ch[i];
        if is_mark(c) {
            let mut run = Vec::new();
            while i < ch.len() && is_mark(ch[i]) { run.push(ch[i]); i += 1; }
            let base = out.last().copied();
            if base.is_none() || !is_cons(base.unwrap()) {
                let tn = run.iter().copied().find(|m| is_tone(*m));
                if let (Some(tn), Some(b)) = (tn, base) {
                    if b == '\u{0E30}' || b == '\u{0E32}' {
                        let mut j = out.len() as isize - 2;
                        while j >= 0 && is_mark(out[j as usize]) { j -= 1; }
                        if j >= 0 && is_cons(out[j as usize]) && !out[(j as usize + 1)..(out.len() - 1)].iter().any(|m| is_tone(*m)) {
                            let mut ins = j as usize + 1;
                            while ins < out.len() - 1 && is_vowel_mark(out[ins]) { ins += 1; }
                            out.insert(ins, tn);
                        }
                    }
                }
                continue;
            }
            let v = run.iter().copied().find(|m| is_vowel_mark(*m));
            let tn = run.iter().copied().find(|m| is_tone(*m));
            let k = run.contains(&KARAN);
            if let Some(v) = v { out.push(v) }
            if let Some(tn) = tn { out.push(tn) }
            if k { out.push(KARAN) }
            continue;
        }
        if c == '\u{0E33}' && i + 1 < ch.len() && is_tone(ch[i + 1]) && !out.last().map(|x| is_tone(*x)).unwrap_or(false) {
            out.push(ch[i + 1]); out.push(c); i += 2;
            while i < ch.len() && is_tone(ch[i]) { i += 1; }
            continue;
        }
        out.push(c); i += 1;
    }
    out.into_iter().collect()
}
