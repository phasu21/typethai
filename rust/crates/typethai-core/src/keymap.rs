//! Full EN (US QWERTY) <-> Thai Kedmanee keymap, unshifted + shifted (mirrors src/keymap.ts).
use std::collections::HashMap;
use std::sync::OnceLock;

pub const EN_UNSHIFTED: &str = "`1234567890-=qwertyuiop[]\\asdfghjkl;'zxcvbnm,./";
pub const TH_UNSHIFTED: &str = "_ๅ/-ภถุึคตจขชๆไำพะัีรนยบลฃฟหกดเ้่าสวงผปแอิืทมใฝ";
pub const EN_SHIFTED: &str = "~!@#$%^&*()_+QWERTYUIOP{}|ASDFGHJKL:\"ZXCVBNM<>?";
pub const TH_SHIFTED: &str = "%+๑๒๓๔ู฿๕๖๗๘๙๐\"ฎฑธํ๊ณฯญฐ,ฅฤฆฏโฌ็๋ษศซ.()ฉฮฺ์?ฒฬฦ";

pub fn is_thai(c: char) -> bool { ('\u{0E00}'..='\u{0E7F}').contains(&c) }

fn maps() -> &'static (HashMap<char, char>, HashMap<char, char>) {
    static M: OnceLock<(HashMap<char, char>, HashMap<char, char>)> = OnceLock::new();
    M.get_or_init(|| {
        let e: Vec<char> = EN_UNSHIFTED.chars().chain(EN_SHIFTED.chars()).collect();
        let t: Vec<char> = TH_UNSHIFTED.chars().chain(TH_SHIFTED.chars()).collect();
        assert_eq!(e.len(), t.len(), "keymap length mismatch");
        let mut en_th = HashMap::new();
        let mut th_en = HashMap::new();
        for (a, b) in e.iter().zip(t.iter()) {
            en_th.insert(*a, *b);
            if is_thai(*b) { th_en.entry(*b).or_insert(*a); } // first wins == JS Map.set order? JS overwrites; see note
        }
        // JS: `for (const [e,t] of EN_TO_TH) if thai(t) m.set(t,e)` -> last writer wins. Thai outputs are unique, so equal.
        (en_th, th_en)
    })
}
pub fn en_to_th_map() -> &'static HashMap<char, char> { &maps().0 }
pub fn th_to_en_map() -> &'static HashMap<char, char> { &maps().1 }

pub fn en_to_th(s: &str) -> String { s.chars().map(|c| *en_to_th_map().get(&c).unwrap_or(&c)).collect() }

/// None if some char cannot be typed on Kedmanee.
pub fn th_to_en(s: &str) -> Option<String> {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        if let Some(e) = th_to_en_map().get(&c) { o.push(*e) } else if c == ' ' { o.push(' ') } else { return None }
    }
    Some(o)
}

/// Where a character's key is: row and column of the US-QWERTY grid, and whether Shift is held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPos { pub r: i32, pub c: i32, pub shift: bool }
const ROWS: [&str; 4] = ["`1234567890-=", "qwertyuiop[]\\", "asdfghjkl;'", "zxcvbnm,./"];
fn positions(on: impl Fn(usize, char, &mut HashMap<char, KeyPos>, KeyPos)) -> HashMap<char, KeyPos> {
    let mut m = HashMap::new();
    for (i, k) in EN_UNSHIFTED.chars().enumerate() {
        let r = ROWS.iter().position(|row| row.contains(k)).unwrap();
        let c = ROWS[r].chars().position(|x| x == k).unwrap();
        on(i, k, &mut m, KeyPos { r: r as i32, c: c as i32, shift: false });
    }
    m
}
/// The Kedmanee key of each character the Thai layout types.
pub fn thai_pos() -> &'static HashMap<char, KeyPos> {
    static M: OnceLock<HashMap<char, KeyPos>> = OnceLock::new();
    M.get_or_init(|| {
        let (th, ths): (Vec<char>, Vec<char>) = (TH_UNSHIFTED.chars().collect(), TH_SHIFTED.chars().collect());
        positions(|i, _, m, p| { m.entry(th[i]).or_insert(p); m.entry(ths[i]).or_insert(KeyPos { shift: true, ..p }); })
    })
}
/// The US-QWERTY key of each character the US layout types.
pub fn en_pos() -> &'static HashMap<char, KeyPos> {
    static M: OnceLock<HashMap<char, KeyPos>> = OnceLock::new();
    M.get_or_init(|| {
        let ens: Vec<char> = EN_SHIFTED.chars().collect();
        positions(|i, k, m, p| { m.entry(k).or_insert(p); if let Some(sk) = ens.get(i) { m.entry(*sk).or_insert(KeyPos { shift: true, ..p }); } })
    })
}
/// Keys that touch on the staggered grid: beside, or above/below and half a key over.
pub fn adjacent(a: KeyPos, b: KeyPos) -> bool {
    if a.r == b.r { return (a.c - b.c).abs() == 1 }
    if b.r == a.r - 1 { return b.c == a.c || b.c == a.c + 1 }
    if b.r == a.r + 1 { return b.c == a.c || b.c == a.c - 1 }
    false
}
/// How far a finger is from typing one character when it means another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The same key with Shift the other way (Thai `ข` / `ฃ`).
    SameKey,
    /// A neighbouring key on the same Shift level.
    Near,
    Far,
}
/// The keys of `a` and `b` on the layout that types both, Kedmanee first (as the corrector weighs them).
fn pair(a: char, b: char) -> Option<(KeyPos, KeyPos)> {
    [thai_pos(), en_pos()].into_iter().find_map(|m| Some((*m.get(&a)?, *m.get(&b)?)))
}
pub fn reach(a: char, b: char) -> Reach {
    match pair(a, b) {
        Some((p, q)) if (p.r, p.c) == (q.r, q.c) => Reach::SameKey,
        Some((p, q)) if p.shift == q.shift && adjacent(p, q) => Reach::Near,
        _ => Reach::Far,
    }
}
/// The letters of `c`'s script on the keys next to it, on its Shift level, in char order.
pub fn neighbours(c: char) -> Vec<char> {
    let letter = |x: char| if is_thai(c) { is_thai(x) } else { x.is_ascii_lowercase() };
    let mut v: Vec<char> = (if is_thai(c) { thai_pos() } else { en_pos() }).iter()
        .filter(|(x, _)| letter(**x)).filter(|(x, _)| reach(c, **x) == Reach::Near).map(|(x, _)| *x).collect();
    v.sort_unstable(); v
}
