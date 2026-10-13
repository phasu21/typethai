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
