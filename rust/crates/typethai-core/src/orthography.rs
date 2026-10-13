//! Thai mark-placement validity rules (mirrors src/orthography.ts).
use crate::keymap::is_thai;
pub fn is_cons(c: char) -> bool { ('\u{0E01}'..='\u{0E2E}').contains(&c) }
pub fn is_above_below(c: char) -> bool { c == '\u{0E31}' || ('\u{0E34}'..='\u{0E3A}').contains(&c) || c == '\u{0E47}' || c == '\u{0E4D}' }
pub fn is_tone(c: char) -> bool { ('\u{0E48}'..='\u{0E4B}').contains(&c) }
pub const KARAN: char = '\u{0E4C}';
fn is_leading(c: char) -> bool { ('\u{0E40}'..='\u{0E44}').contains(&c) }
fn is_follow(c: char) -> bool { matches!(c, '\u{0E30}' | '\u{0E32}' | '\u{0E33}' | '\u{0E45}') }

/// Ok(()) if plausible Thai; Err(reason) otherwise. Reasons are informational (not parity-checked verbatim).
pub fn check_thai(s: &str) -> Result<(), String> {
    let ch: Vec<char> = s.chars().collect();
    let opt = |i: isize| if i >= 0 && (i as usize) < ch.len() { Some(ch[i as usize]) } else { None };
    for i in 0..ch.len() {
        let c = ch[i];
        let p = opt(i as isize - 1);
        let pp = opt(i as isize - 2);
        let pc = |f: fn(char) -> bool, x: Option<char>| x.map(f).unwrap_or(false);
        if is_above_below(c) {
            if !pc(is_cons, p) { return Err(format!("vowel mark {c} after non-consonant at {i}")); }
        } else if is_tone(c) || c == KARAN {
            let ok = pc(is_cons, p) || (pc(is_above_below, p) && pc(is_cons, pp));
            if !ok { return Err(format!("tone/mark {c} misplaced at {i}")); }
        } else if is_leading(c) {
            if !pc(is_cons, opt(i as isize + 1)) { return Err(format!("leading vowel {c} not followed by consonant")); }
        } else if is_follow(c) {
            if i == 0 && c != '\u{0E45}' { return Err(format!("{c} at start")); }
            if i > 0 {
                let p = p.unwrap();
                if !(is_cons(p) || is_tone(p) || is_above_below(p) || p == '\u{0E32}' || p == '\u{0E30}' || p == KARAN) { return Err(format!("{c} after {p}")); }
            }
        }
    }
    if !ch.iter().any(|c| is_thai(*c)) { return Err("no thai".into()); }
    Ok(())
}
