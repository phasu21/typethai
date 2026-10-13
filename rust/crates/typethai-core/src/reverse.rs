//! Reverse direction (v11): English typed while the Thai Kedmanee input source is active.
//! The platform still reports physical keys as their US-QWERTY chars, so a word buffer `keys` (e.g. `dont`) has two
//! readings: the Thai text the user actually got on screen (`en_to_th(keys)` = `กนืะ`) and the intended English (`keys`).
//! A two-stage logistic model (same shape as the v6 forward classifier: token features, then token + context features)
//! scores "English clearly wins". Weights are trained by `tt-train-rev` and embedded (src/reverse-model.json).
use crate::{classifier::{context_features, lr_predict, FlipModel}, detector::{core_of, Detector}, keymap::{en_to_th, is_thai, EN_SHIFTED, EN_UNSHIFTED, TH_SHIFTED, TH_UNSHIFTED}, orthography::check_thai};
use std::collections::HashMap;
use std::sync::OnceLock;

/// Every char the Kedmanee layout can produce -> the US key that produces it (first wins), incl. non-Thai outputs
/// such as `.`, `(`, `"`, `-`. Used to turn real Thai text into the keys that typed it (training negatives, tests).
fn th_layout_inverse() -> &'static HashMap<char, char> {
    static M: OnceLock<HashMap<char, char>> = OnceLock::new();
    M.get_or_init(|| {
        let mut m = HashMap::new();
        for (e, t) in EN_UNSHIFTED.chars().chain(EN_SHIFTED.chars()).zip(TH_UNSHIFTED.chars().chain(TH_SHIFTED.chars())) { m.entry(t).or_insert(e); }
        m
    })
}
/// Keys that type `s` on Kedmanee; None if some char can't be typed there (Latin letters, ASCII digits, …).
pub fn th_text_to_keys(s: &str) -> Option<String> { s.chars().map(|c| if c == ' ' { Some(' ') } else { th_layout_inverse().get(&c).copied() }).collect() }

/// Very common short English / chat words (typed on the wrong layout they are too short for the n-grams alone).
pub const SHORT_EN: &[&str] = &["i","a","ok","okay","im","u","ur","is","it","in","on","to","of","no","so","oh","my","me","we","he","be","do","go","up","an","at","as","or","if","by","hi","yo","ya","lol","idk","btw","thx","omg","wtf","pls","plz","yes","yea","yeah","yep","nah","the","and","you","are","not","but","can","don't","dont","im","i'm","its","it's","ty","np","gg","brb","k","kk","hey","bye","wow","nice"];

/// Stricter reverse-only orthography check on top of `check_thai` (which stays parity-identical to the TS engine):
/// sara a (ะ) or sara aa (า) right after an above/below vowel is not Thai, except เ-ือะ / เ-ีย / เ-ือ patterns
/// (here: ะ/า after ื or ี with a leading เ earlier in the word). Catches `กนืะ` ("dont"), which check_thai accepts.
pub fn strict_invalid(th: &str) -> bool {
    let c: Vec<char> = th.chars().collect();
    for i in 1..c.len() {
        if matches!(c[i], 'ะ' | 'า') && matches!(c[i - 1], 'ั' | 'ิ' | 'ี' | 'ึ' | 'ื' | 'ุ' | 'ู' | '็' | '่' | '้' | '๊' | '๋') {
            let pre = &c[..i - 1]; let lead = pre.iter().rposition(|x| *x == 'เ').is_some_and(|j| i - 1 - j <= 3);
            let tone_ok = matches!(c[i - 1], '่' | '้' | '๊' | '๋'); // tone marks before ะ/า are normal (ก่า, ค้ะ)
            if tone_ok { continue }
            if matches!(c[i - 1], 'ื' | 'ี' | 'ั') && lead { continue }
            return true;
        }
    }
    false
}

#[derive(Clone, Debug)]
pub struct RevDecision { pub keys: String, pub thai: String, pub valid_th: bool, pub strict_bad: bool, pub ll_en: f64, pub ll_th: f64, pub en_hit: bool, pub th_cov: f64, pub th_word: bool, pub convertible: bool }

fn b(x: bool) -> f64 { if x { 1.0 } else { 0.0 } }
pub fn score(det: &Detector, keys: &str) -> RevDecision {
    let thai = en_to_th(keys);
    // convertible: every key is a printable US key and the Thai reading actually contains Thai
    let convertible = !keys.is_empty() && keys.chars().all(|c| ('\x21'..='\x7e').contains(&c)) && thai.chars().any(is_thai);
    let core = core_of(keys); let core = if core.is_empty() { keys.to_string() } else { core };
    let lower = core.to_lowercase();
    let th_core = en_to_th(&core);
    RevDecision {
        keys: keys.into(), valid_th: check_thai(&thai).is_ok(), strict_bad: strict_invalid(&thai),
        ll_en: det.english.mean_log_prob(&lower), ll_th: det.thai.mean_log_prob(&thai),
        en_hit: det.en_words.contains(lower.as_str()) || SHORT_EN.contains(&lower.as_str()),
        th_cov: det.th_dict.coverage(&th_core), th_word: det.th_dict.has(&th_core), thai, convertible,
    }
}
pub fn features(d: &RevDecision) -> Vec<f64> {
    let k = d.keys.as_str(); let n = k.chars().count().max(1) as f64;
    let core = core_of(k); let core = if core.is_empty() { k.to_string() } else { core };
    let letters = k.chars().filter(|c| c.is_ascii_alphabetic()).count() as f64;
    let upper = k.chars().filter(|c| c.is_ascii_uppercase()).count() as f64;
    let digits = k.chars().filter(|c| c.is_ascii_digit()).count() as f64;
    let alpha_lower = !core.is_empty() && core.chars().all(|c| c.is_ascii_lowercase());
    let contraction = { let p: Vec<&str> = core.split('\'').collect(); p.len() == 2 && p.iter().all(|x| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphabetic())) };
    let cap = { let mut c = core.chars(); c.next().is_some_and(|f| f.is_ascii_uppercase()) && c.clone().count() > 0 && c.all(|x| x.is_ascii_lowercase()) };
    let short = SHORT_EN.contains(&core.to_lowercase().as_str());
    let m = d.ll_en - d.ll_th;
    vec![
        b(d.convertible), b(d.valid_th), d.ll_en, d.ll_th, m, b(d.en_hit), d.th_cov, b(d.th_word),
        n.min(20.0) / 20.0, 1.0 / n, b(n <= 2.0), b(n <= 4.0),
        letters / n, upper / n, digits / n, (n - letters - digits) / n,
        b(alpha_lower), b(contraction), b(short), if d.valid_th { m } else { 0.0 }, b(d.en_hit && !d.th_word), b(cap), b(d.strict_bad), b(!d.valid_th || d.strict_bad),
    ]
}
pub struct Reverse { pub model: FlipModel }
impl Reverse {
    pub fn embedded() -> Reverse { Reverse { model: serde_json::from_str(include_str!("reverse-model.json")).expect("reverse-model.json") } }
    pub fn decide_line(&self, det: &Detector, keys: &[&str]) -> Vec<RevDecision> { keys.iter().map(|k| score(det, k)).collect() }
    /// P(English intended) per token, given the whole line (stage 2 uses the neighbours' stage-1 probabilities).
    pub fn line_probs(&self, ds: &[RevDecision]) -> Vec<f64> {
        let p1: Vec<f64> = ds.iter().map(|d| lr_predict(&self.model.stage1, &features(d))).collect();
        ds.iter().enumerate().map(|(i, d)| {
            if !d.convertible { return 0.0 }
            let mut x = features(d); x.extend(context_features(&p1, i)); lr_predict(&self.model.stage2, &x)
        }).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn inverse_keys() {
        assert_eq!(th_text_to_keys("ร กนืะ าืนไ").as_deref(), Some("i dont know"));
        assert_eq!(th_text_to_keys("กนืงะ").as_deref(), Some("don't"));
        assert_eq!(en_to_th("don't"), "กนืงะ");
        assert_eq!(th_text_to_keys("สวัสดี").as_deref(), Some("l;ylfu"));
        assert!(th_text_to_keys("abc").is_none());
        assert!(check_thai("กนืะ").is_ok()); // the shared (TS-parity) checker accepts it...
        assert!(strict_invalid("กนืะ")); // ...the reverse-only strict check does not
        for ok in ["เสือ", "เมื่อ", "เกือะ", "ก่า", "ค้ะ", "กะ", "สวัสดี", "เรียน", "เขียน", "ทั้ง"] { assert!(!strict_invalid(ok), "{ok}"); }
    }
}
