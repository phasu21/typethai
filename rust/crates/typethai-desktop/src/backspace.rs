//! What one Backspace removes in the focused app, and the fewest Backspaces that turn the text right before the caret
//! into new text.
use unicode_segmentation::UnicodeSegmentation;

/// What one Backspace removes. Text views and browsers delete one code point, so a Thai vowel or tone mark goes on its
/// own. Terminal UIs (Ink in Claude Code, ratatui apps) delete one extended grapheme cluster: a consonant with its marks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeleteUnit { #[default] CodePoint, Grapheme }

/// Terminals, by macOS bundle id or Windows executable name: apps that delete a grapheme per Backspace.
const GRAPHEME_APPS: &[&str] = &[
    "com.mitchellh.ghostty", "com.apple.Terminal", "com.googlecode.iterm2", "dev.warp.Warp-Stable", "com.github.wez.wezterm",
    "org.alacritty", "net.kovidgoyal.kitty", "co.zeit.hyper", "org.tabby", "com.stablyai.orca",
    "WindowsTerminal.exe", "OpenConsole.exe", "conhost.exe", "wezterm-gui.exe", "alacritty.exe",
];

impl DeleteUnit {
    /// The unit of the app named by `id`, its macOS bundle id or Windows executable name.
    pub fn of_app(id: &str) -> Self {
        if GRAPHEME_APPS.iter().any(|a| a.eq_ignore_ascii_case(id)) { DeleteUnit::Grapheme } else { DeleteUnit::CodePoint }
    }
    /// The Backspaces that delete `s`.
    pub fn count(self, s: &str) -> usize {
        match self { DeleteUnit::CodePoint => s.chars().count(), DeleteUnit::Grapheme => s.graphemes(true).count() }
    }
}

/// Turn `old`, the text right before the caret, into `new`: the Backspaces to send and the text to type after them.
/// The prefix they share stays on screen, cut back to where a grapheme of `old` starts, so the first Backspace never
/// lands on a vowel or tone mark (where one Backspace deletes a whole grapheme it would take the consonant too).
pub fn edit<'a>(old: &str, new: &'a str, unit: DeleteUnit) -> (usize, &'a str) {
    let same = old.char_indices().zip(new.chars()).find(|((_, a), b)| a != b).map_or(old.len().min(new.len()), |((i, _), _)| i);
    let keep = old.grapheme_indices(true).map(|(i, _)| i).chain([old.len()]).take_while(|&i| i <= same).last().unwrap_or(0);
    (unit.count(&old[keep..]), &new[keep..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeleteUnit::{CodePoint, Grapheme};
    #[test] fn a_completion_types_only_its_rest() {
        assert_eq!(edit("ฉันชอบภาษ", "ฉันชอบภาษา", Grapheme), (0, "า"));
        assert_eq!(edit("hel", "help", CodePoint), (0, "p"));
    }
    #[test] fn a_changed_mark_retypes_its_consonant() {
        assert_eq!(edit("กิน ", "กีน ", CodePoint), (4, "กีน "), "ิ is inside ก's grapheme: delete from ก");
        assert_eq!(edit("กิน ", "กีน ", Grapheme), (3, "กีน "));
        assert_eq!(edit("ทำงาน", "ทำงาร", Grapheme), (1, "ร"));
    }
    #[test] fn texts_in_different_scripts_share_nothing() {
        assert_eq!(edit("สวัสดี ", "l;ylfu ", CodePoint), (7, "l;ylfu "));
        assert_eq!(edit("สวัสดี ", "l;ylfu ", Grapheme), (5, "l;ylfu "));
        assert_eq!(edit("ths ", "this ", CodePoint), (2, "is "));
        assert_eq!(edit("abc", "", CodePoint), (3, ""));
    }
    #[test] fn a_thai_mark_joins_its_consonant_only_in_graphemes() {
        assert_eq!((CodePoint.count("ฉันชอบภาษ"), Grapheme.count("ฉันชอบภาษ")), (9, 8));
        assert_eq!((CodePoint.count("ทำ"), Grapheme.count("ทำ")), (2, 1), "sara am is a SpacingMark");
        assert_eq!((CodePoint.count("ok "), Grapheme.count("ok ")), (3, 3));
    }
    #[test] fn terminals_delete_graphemes() {
        assert_eq!(DeleteUnit::of_app("com.mitchellh.ghostty"), Grapheme);
        assert_eq!(DeleteUnit::of_app("windowsterminal.exe"), Grapheme, "Windows names match in any case");
        assert_eq!(DeleteUnit::of_app("com.apple.TextEdit"), CodePoint);
        assert_eq!(DeleteUnit::of_app(""), CodePoint);
    }
}
