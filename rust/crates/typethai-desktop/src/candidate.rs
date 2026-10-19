//! The row between a generator and `Engine::show`: the apply instruction, the score its source computed (in that
//! source's own domain), and which generator produced it. `BubbleItem`, the only type the UI and the apply paths see,
//! is unchanged and still has no float. The trigger policy lives here so every producer answers to the same table.
use crate::engine::{BubbleItem, BubbleKind};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub item: BubbleItem,
    /// None = the producer had no probability for this row (a layout flip, a spell-checker string list, a rewrite) or
    /// the source is deterministic. A row with no score is never gated by a confidence floor.
    pub conf: Option<f64>,
    pub src: Src,
}

impl Candidate {
    pub fn bare(item: BubbleItem, src: Src) -> Self { Candidate { item, conf: None, src } }
    pub fn scored(item: BubbleItem, src: Src, conf: f64) -> Self { Candidate { item, conf: Some(conf), src } }
}

/// Which generator produced a row. Distinct from `Reason`, which is the badge: `Live`, `Convert`, `Guess` all say
/// `th`/`en` there, but they must not share a floor or a report row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Src {
    Live,        // personal layout model, forward or reverse, mid-word
    Convert,     // layout conversion at a boundary (`auto_convert`'s Undo is `Undo`)
    ConvertPrev, // reverse phrase flip, after a boundary
    Undo,        // the UndoConversion row; deterministic
    Guess,       // double-Tab / hotkey phrase guess
    Typo,        // corrector suggestions, completer fixes, spell-checker guesses
    Completion,  // prefix completion (completer, spell-checker extensions, carried rows)
    Prediction,  // typethai-predict: the typed-completion row and the model's next-word row
    NextWord,    // corpus bigram pairs and the user's own pairs
    Rewrite,     // selection fixes, presets, results, notices
}

impl Src {
    /// The floor this source answers to; None = deterministic or log-only, never gated.
    pub fn floor(self) -> Option<Floor> {
        match self {
            Src::Completion => Some(Floor::Completion),
            Src::Prediction => Some(Floor::Prediction),
            Src::Live | Src::Convert | Src::ConvertPrev | Src::Undo | Src::Guess | Src::Typo | Src::NextWord | Src::Rewrite => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Src::Live => "live", Src::Convert => "convert", Src::ConvertPrev => "convert_prev",
            Src::Undo => "undo", Src::Guess => "guess", Src::Typo => "typo",
            Src::Completion => "completion", Src::Prediction => "prediction",
            Src::NextWord => "next_word", Src::Rewrite => "rewrite",
        }
    }
}

/// The confidence domains that get a floor. They are not comparable numbers: a completion share and a predictor
/// probability are two different measurements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Floor { Completion, Prediction }

/// The trigger policy. No margin yet: a source's best row alone earns its slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floors {
    pub completion: f64,
    pub prediction: f64,
}

impl Default for Floors {
    fn default() -> Self { Floors { completion: 0.15, prediction: 0.15 } }
}

impl Floors {
    pub fn get(&self, f: Floor) -> f64 {
        match f { Floor::Completion => self.completion, Floor::Prediction => self.prediction }
    }
    /// A row earns its slot unless it is scored and its own source's floor is above it.
    pub fn admits(&self, src: Src, conf: Option<f64>) -> bool {
        match (src.floor(), conf) {
            (Some(f), Some(conf)) => conf >= self.get(f),
            _ => true, // deterministic, or a row the producer could not score
        }
    }
    pub fn keep(&self, c: &Candidate) -> bool { self.admits(c.src, c.conf) }
}

/// Drop what does not deserve a slot. Input and output are in display order; the stable sort happened before this.
pub fn gate(rows: Vec<Candidate>, floors: &Floors) -> Vec<Candidate> {
    rows.into_iter().filter(|c| floors.keep(c)).collect()
}

/// The source a row built the old way belongs to, so unscored offers are still attributed in the report.
/// `Complete` is the ambiguous fallback: the migrated producers tag their rows themselves, and a row that arrives
/// through `show` with this kind comes from a producer that was not migrated (a useful signal, not a mislabel).
pub fn src_of_kind(kind: &BubbleKind) -> Src {
    match kind {
        BubbleKind::Live { .. } => Src::Live,
        BubbleKind::UndoConversion => Src::Undo,
        BubbleKind::Convert { .. } => Src::Convert,
        BubbleKind::Typo { .. } => Src::Typo,
        BubbleKind::ConvertPrev { .. } => Src::ConvertPrev,
        BubbleKind::Guess { .. } => Src::Guess,
        BubbleKind::Complete { .. } => Src::Completion,
        BubbleKind::Rewrite(_) => Src::Rewrite,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> BubbleItem { BubbleItem { label: label.into(), kind: BubbleKind::Complete { typed: String::new(), insert: label.into() } } }
    fn scored(label: &str, src: Src, conf: f64) -> Candidate { Candidate::scored(item(label), src, conf) }

    #[test]
    fn a_weak_completion_is_dropped_and_a_strong_one_kept() {
        let f = Floors::default();
        let rows = vec![scored("weak", Src::Completion, 0.10), scored("strong", Src::Completion, 0.20)];
        assert_eq!(gate(rows, &f), vec![scored("strong", Src::Completion, 0.20)]);
    }
    #[test]
    fn the_floor_is_inclusive() {
        let f = Floors::default();
        assert_eq!(gate(vec![scored("edge", Src::Completion, 0.15)], &f).len(), 1);
    }
    #[test]
    fn unscored_and_deterministic_rows_are_kept() {
        let f = Floors::default();
        let rows = vec![
            Candidate::bare(item("completion with no score"), Src::Completion),
            Candidate::bare(item("convert"), Src::Convert),
            scored("pair", Src::NextWord, 0.01),
            scored("typo", Src::Typo, 0.0),
        ];
        assert_eq!(gate(rows.clone(), &f), rows);
    }
    #[test]
    fn floors_are_per_source() {
        let f = Floors::default();
        let rows = vec![scored("completion", Src::Completion, 0.10), scored("prediction", Src::Prediction, 0.20)];
        assert_eq!(gate(rows, &f), vec![scored("prediction", Src::Prediction, 0.20)]);
    }
    #[test]
    fn the_gate_is_deterministic_and_keeps_display_order() {
        let f = Floors::default();
        let rows = vec![scored("b", Src::Completion, 0.5), scored("a", Src::Completion, 0.6), scored("x", Src::Completion, 0.1)];
        let once = gate(rows.clone(), &f);
        assert_eq!(once, vec![scored("b", Src::Completion, 0.5), scored("a", Src::Completion, 0.6)]);
        assert_eq!(gate(rows, &f), once);
    }
    #[test]
    fn a_kind_is_attributed_to_its_source() {
        assert_eq!(src_of_kind(&BubbleKind::Live { typed: String::new(), thai: String::new(), prev: 0 }), Src::Live);
        assert_eq!(src_of_kind(&BubbleKind::Typo { word: String::new() }), Src::Typo);
        assert_eq!(src_of_kind(&BubbleKind::UndoConversion), Src::Undo);
        assert_eq!(src_of_kind(&BubbleKind::ConvertPrev { k: 0 }), Src::ConvertPrev);
        assert_eq!(src_of_kind(&BubbleKind::Rewrite(crate::engine::RewriteRow::Notice)), Src::Rewrite);
    }
}
