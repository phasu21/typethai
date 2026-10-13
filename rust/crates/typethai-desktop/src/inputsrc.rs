//! Choosing which installed OS input source to switch to (pure; the platform layers enumerate and select).
use crate::engine::InputLang;

/// An enabled keyboard input source as reported by the OS.
#[derive(Clone, Debug, PartialEq)]
pub struct Source { pub id: String, pub name: String, pub ascii: bool, pub selectable: bool }

pub fn is_thai_id(id: &str) -> bool { id.to_lowercase().contains("thai") }

/// Thai: prefer Kedmanee (`com.apple.keylayout.Thai`), then any other Thai source except Pattachote, then Pattachote.
/// Latin: prefer ABC, then US, then any other selectable ASCII-capable non-Thai source.
pub fn pick(sources: &[Source], lang: InputLang) -> Option<&Source> {
    let sel = || sources.iter().filter(|s| s.selectable);
    match lang {
        InputLang::Thai => sel().find(|s| s.id == "com.apple.keylayout.Thai")
            .or_else(|| sel().find(|s| is_thai_id(&s.id) && !s.id.to_lowercase().contains("pattachote")))
            .or_else(|| sel().find(|s| is_thai_id(&s.id))),
        InputLang::Latin => sel().find(|s| s.id == "com.apple.keylayout.ABC")
            .or_else(|| sel().find(|s| s.id == "com.apple.keylayout.US"))
            .or_else(|| sel().find(|s| s.ascii && !is_thai_id(&s.id) && s.id.contains(".keylayout."))),
    }
}
/// Whether the current source already fits `lang` (then no switch is needed).
pub fn fits(current_id: &str, current_ascii: bool, lang: InputLang) -> bool {
    match lang { InputLang::Thai => is_thai_id(current_id), InputLang::Latin => current_ascii && !is_thai_id(current_id) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn s(id: &str, ascii: bool) -> Source { Source { id: id.into(), name: id.into(), ascii, selectable: true } }
    #[test] fn picks_kedmanee_and_abc() {
        let v = vec![s("com.apple.keylayout.US", true), s("com.apple.keylayout.Thai-PattaChote", false), s("com.apple.keylayout.Thai", false), s("com.apple.keylayout.ABC", true)];
        assert_eq!(pick(&v, InputLang::Thai).unwrap().id, "com.apple.keylayout.Thai");
        assert_eq!(pick(&v, InputLang::Latin).unwrap().id, "com.apple.keylayout.ABC");
    }
    #[test] fn fallbacks() {
        let v = vec![s("com.apple.keylayout.Thai-PattaChote", false), s("com.apple.keylayout.British", true), s("com.apple.inputmethod.Kotoeri.RomajiTyping", true)];
        assert_eq!(pick(&v, InputLang::Thai).unwrap().id, "com.apple.keylayout.Thai-PattaChote");
        assert_eq!(pick(&v, InputLang::Latin).unwrap().id, "com.apple.keylayout.British");
        let mut only_en = vec![s("com.apple.keylayout.US", true)]; assert!(pick(&only_en, InputLang::Thai).is_none());
        only_en[0].selectable = false; assert!(pick(&only_en, InputLang::Latin).is_none());
    }
    #[test] fn fit() {
        assert!(fits("com.apple.keylayout.Thai", false, InputLang::Thai)); assert!(!fits("com.apple.keylayout.ABC", true, InputLang::Thai));
        assert!(fits("com.apple.keylayout.ABC", true, InputLang::Latin)); assert!(!fits("com.apple.keylayout.Thai", false, InputLang::Latin));
    }
}
