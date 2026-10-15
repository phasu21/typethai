//! The fewest Backspaces that turn the text right before the caret into new text.
use unicode_segmentation::UnicodeSegmentation;

/// Turn `old`, the text right before the caret, into `new`: the Backspaces to send and the text to type after them.
/// The prefix they share stays on screen, cut back to where a grapheme of `old` starts, so the first Backspace never
/// lands on a vowel or tone mark (where one Backspace deletes a whole grapheme it would take the consonant too).
pub fn edit<'a>(old: &str, new: &'a str) -> (usize, &'a str) {
    let same = old.char_indices().zip(new.chars()).find(|((_, a), b)| a != b).map_or(old.len().min(new.len()), |((i, _), _)| i);
    let keep = old.grapheme_indices(true).map(|(i, _)| i).chain([old.len()]).take_while(|&i| i <= same).last().unwrap_or(0);
    (old[keep..].chars().count(), &new[keep..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn a_completion_types_only_its_rest() {
        assert_eq!(edit("ฉันชอบภาษ", "ฉันชอบภาษา"), (0, "า"));
        assert_eq!(edit("hel", "help"), (0, "p"));
    }
    #[test] fn a_changed_mark_retypes_its_consonant() {
        assert_eq!(edit("กิน ", "กีน "), (4, "กีน "), "ิ is inside ก's grapheme: delete from ก");
        assert_eq!(edit("ทำงาน", "ทำงาร"), (1, "ร"));
    }
    #[test] fn texts_in_different_scripts_share_nothing() {
        assert_eq!(edit("l;ylfu ", "สวัสดี "), (7, "สวัสดี "));
        assert_eq!(edit("ths ", "this "), (2, "is "));
        assert_eq!(edit("abc", ""), (3, ""));
    }
}
