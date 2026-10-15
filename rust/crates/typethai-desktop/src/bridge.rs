//! Messages and shared state between the key-capture thread (CGEventTap / low-level hooks, owns the Engine) and the
//! gpui UI thread (bubble window + tray).
use crate::engine::{BubbleItem, BubbleKind};
use crate::placement::{Anchor, Rect};
use crate::prefs::{SettingsCmd, SettingsSnap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub enum UiMsg {
    /// Highlight this row (Shift+Up/Down).
    Select(usize),
    /// `layout`: what the bubble asks for; `layout` falls back to rows when a strip does not fit.
    Show { items: Vec<String>, layout: BubbleLayout, anchor: Anchor },
    /// The caret moved while the bubble is shown: re-place it (same size; skipped if the rect doesn't change).
    Move(Anchor),
    Hide,
    Paused(bool),
    /// Capture thread saved state and stopped; UI should quit.
    Stopped,
    /// Current don't-translate list and learned words, for the Settings window.
    Settings(SettingsSnap),
}

impl UiMsg {
    /// The bubble for the engine's `items`, laid out as the setting `pref` says. Rewrite rows (the AI presets,
    /// "Rewriting…", results and notices) are always stacked: they are sentences, and one rewrite moves from presets to
    /// a result in place, which should not flip the layout under the reader.
    pub fn show(items: Vec<BubbleItem>, pref: BubbleLayout, anchor: Anchor) -> UiMsg {
        let layout = if items.iter().any(|i| matches!(i.kind, BubbleKind::Rewrite(_))) { BubbleLayout::Vertical } else { pref };
        UiMsg::Show { items: items.into_iter().map(|i| i.label).collect(), layout, anchor }
    }
}

/// How the bubble lays out its options: one horizontal strip of buttons, or stacked rows. As a setting it is what the
/// user asked for; a bubble whose options do not suit a strip is laid out vertically anyway (see `layout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BubbleLayout { #[default] Horizontal, Vertical }

/// Bubble layout constants (logical px) shared by the view and the click hit-test.
pub const PAD: f64 = 6.0;
pub const ROW_H: f64 = 30.0;
/// Bubble chrome metrics (logical units). One row = [px ROW_PX][label column (wraps)][gap][Tab keycap][px ROW_PX], at least ROW_H tall.
pub const BORDER: f64 = 1.0;
/// Panel corner radius; also the native layer clip radius on macOS.
pub const RADIUS: f64 = 8.0;
pub const ROW_PX: f64 = 10.0;
pub const FONT_PX: f64 = 15.0;
/// Space between the suggestion and its key hint (`Tab`, or `Tab` `2`).
pub const HINT_GAP: f64 = 8.0;
/// Key hint keycaps: fixed sizes so the hint fits its slot whatever the platform font.
pub const TAB_CAP_W: f64 = 30.0;
pub const DIGIT_CAP_W: f64 = 18.0;
pub const KEYCAP_GAP: f64 = 2.0;
pub const KEYCAP_H: f64 = 18.0;
pub const KEYCAP_FONT_PX: f64 = 11.0;
/// Width reserved for the widest key hint: `Tab` and a digit.
pub const HINT_SLOT_W: f64 = TAB_CAP_W + KEYCAP_GAP + DIGIT_CAP_W;
/// Added to every measured single-line width: Thai upper/lower marks and tone marks on the last cluster can paint a
/// little past the advance width, and layout rounds to device pixels; the slack keeps them inside the column.
pub const MEASURE_SLACK: f64 = 3.0;
pub const MIN_W: f64 = 200.0;
/// Margin kept from the monitor's visible edges when computing the maximum bubble width.
pub const SCREEN_MARGIN: f64 = 24.0;
/// Width used by everything except the label column (padding, border, and the Tab keycap slot).
pub const CHROME_W: f64 = 2.0 * (PAD + BORDER) + 2.0 * ROW_PX + HINT_GAP + HINT_SLOT_W;

/// Strip metrics: each option is a gpui-kit Medium `Button` (1 px border, 10 px padding, 32 px tall) holding the key hint
/// and the label, `OPT_GAP` apart.
pub const OPT_BORDER: f64 = 1.0;
pub const OPT_PX: f64 = 10.0;
pub const OPT_H: f64 = 32.0;
pub const OPT_GAP: f64 = 4.0;
/// A label wider than this stacks the bubble: past about 28 Latin letters an option is a phrase, and phrases side by
/// side are no longer read at a glance.
pub const STRIP_LABEL_MAX: f64 = 240.0;

/// The bubble's geometry in logical px, window-relative. `h` and the rows' `cells` are only the first guess for the
/// vertical layout (rows wrap; the real heights come from gpui's laid-out panel, see `Ui::reconcile`). The strip never
/// wraps, so its guess is exact.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// What the bubble is drawn as, after the fallback from a strip that does not fit.
    pub kind: BubbleLayout,
    pub w: f64, pub h: f64,
    /// Each option's label column: one shared width for the rows (the widest label, clamped), each label's own in a strip.
    pub label_w: Vec<f64>,
    /// Each option's box: the row, or the button.
    pub cells: Vec<Rect>,
}
impl Layout {
    /// Where the first option's text starts, from the bubble's left edge. Placement puts it at the caret's x, so the
    /// suggestion reads on from what is being typed.
    pub fn text_x(&self) -> f64 {
        match (self.kind, self.cells.first()) {
            (BubbleLayout::Horizontal, Some(c)) => c.x + OPT_BORDER + OPT_PX + hint_w(true) + HINT_GAP,
            _ => PAD + BORDER + ROW_PX,
        }
    }
}
/// Lay out options from their measured one-line label widths. A strip when `wanted` is one, every label is at most
/// `STRIP_LABEL_MAX` and the whole strip fits `max_w` (the monitor's visible width minus margins); otherwise rows.
pub fn layout(label_widths: &[f64], max_w: f64, wanted: BubbleLayout) -> Layout {
    let max_w = max_w.max(MIN_W);
    if wanted == BubbleLayout::Horizontal && label_widths.iter().all(|w| *w <= STRIP_LABEL_MAX) {
        let s = strip(label_widths);
        if s.w <= max_w { return s }
    }
    rows(label_widths, max_w)
}
/// Rows: the label column is the widest label, clamped so the bubble is in [MIN_W, max_w]. Longer labels wrap; nothing
/// is truncated.
fn rows(label_widths: &[f64], max_w: f64) -> Layout {
    let widest = label_widths.iter().copied().fold(0.0, f64::max).ceil();
    let label_w = widest.clamp(MIN_W - CHROME_W, max_w - CHROME_W);
    let (inset, w) = (PAD + BORDER, CHROME_W + label_w);
    let mut y = inset;
    let cells = label_widths.iter().map(|lw| { let c = Rect::new(inset, y, w - 2.0 * inset, ROW_H * (lw / label_w).ceil().max(1.0)); y = c.bottom(); c }).collect();
    Layout { kind: BubbleLayout::Vertical, w, h: y + inset, label_w: vec![label_w; label_widths.len()], cells }
}
/// One button per option, left to right. The first is highlighted when the bubble appears; moving the highlight swaps
/// which button carries the wider Tab keycap, so the strip's width never changes.
fn strip(label_widths: &[f64]) -> Layout {
    let inset = PAD + BORDER;
    let label_w: Vec<f64> = label_widths.iter().map(|w| w.ceil()).collect();
    let mut x = inset;
    let cells: Vec<Rect> = label_w.iter().enumerate().map(|(i, lw)| {
        let c = Rect::new(x, inset, 2.0 * (OPT_BORDER + OPT_PX) + hint_w(i == 0) + HINT_GAP + lw, OPT_H);
        x = c.right() + OPT_GAP; c
    }).collect();
    let w = cells.last().map_or(2.0 * inset, |c| c.right() + inset);
    Layout { kind: BubbleLayout::Horizontal, w, h: 2.0 * inset + OPT_H, label_w, cells }
}
/// A strip option's key hint: the Tab keycap on the highlighted option, its number on the others.
pub fn hint_w(highlighted: bool) -> f64 { if highlighted { TAB_CAP_W } else { DIGIT_CAP_W } }
pub fn max_width_for(visible_w: f64) -> f64 { visible_w - 2.0 * SCREEN_MARGIN }

/// On-screen bubble geometry in global units (set by the UI, read by the capture thread for click-to-accept).
/// `cells` = each option's box in logical units relative to the bubble's top-left (a row, or a strip button);
/// `scale` converts logical units to the global units of `frame` (Windows DPI; 1.0 on macOS).
#[derive(Clone, Debug, PartialEq)]
pub struct Geom { pub frame: Rect, pub cells: Vec<Rect>, pub scale: f64 }
impl Geom {
    /// Option index under a global point, if the point is on the bubble (Some(None) = on the bubble but not on an
    /// option: the panel's padding, or a gap between buttons).
    pub fn hit(&self, x: f64, y: f64) -> Option<Option<usize>> {
        if !self.frame.contains(x, y) { return None }
        let (lx, ly) = ((x - self.frame.x) / self.scale, (y - self.frame.y) / self.scale);
        Some(self.cells.iter().position(|c| c.contains(lx, ly)))
    }
}
pub static GEOM: Mutex<Option<Geom>> = Mutex::new(None);
pub fn geom() -> Option<Geom> { GEOM.lock().unwrap().clone() }
pub fn set_geom(g: Option<Geom>) { *GEOM.lock().unwrap() = g; }

/// Tray -> capture thread requests (polled by a timer on the capture thread).
pub static REQ_TOGGLE_PAUSE: AtomicBool = AtomicBool::new(false);
pub static REQ_QUIT: AtomicBool = AtomicBool::new(false);
/// macOS input source, owned by the main thread: on macOS 26 HIToolbox traps when TIS is called off the main queue, and
/// the event tap runs on its own thread. The tap reads the last answer and leaves switches here for the UI loop.
pub static INPUT_THAI: AtomicBool = AtomicBool::new(false);
pub static REQ_SWITCH_INPUT: Mutex<Option<crate::engine::InputLang>> = Mutex::new(None);
/// `--settings`: open the Settings window once the UI is up.
pub static REQ_OPEN_SETTINGS: AtomicBool = AtomicBool::new(false);
/// Settings window wants a fresh snapshot. Commands, if any, are applied first.
pub static REQ_SETTINGS: AtomicBool = AtomicBool::new(false);
static SETTINGS_CMDS: Mutex<Vec<SettingsCmd>> = Mutex::new(Vec::new());
pub fn take(flag: &AtomicBool) -> bool { flag.swap(false, Ordering::SeqCst) }
pub fn push_settings(cmd: SettingsCmd) {
    SETTINGS_CMDS.lock().unwrap().push(cmd);
    REQ_SETTINGS.store(true, Ordering::SeqCst);
}
pub fn request_settings() { REQ_SETTINGS.store(true, Ordering::SeqCst); }
pub fn take_settings() -> Vec<SettingsCmd> { std::mem::take(&mut SETTINGS_CMDS.lock().unwrap()) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::RewriteRow;
    const INSET: f64 = PAD + BORDER;
    #[test] fn short_options_make_a_strip_sized_to_its_buttons() {
        let l = layout(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(l.kind, BubbleLayout::Horizontal);
        assert_eq!(l.label_w, vec![41.0, 61.0, 90.0]);
        // [border+pad][Tab 30][gap 8][label][pad+border]; the others carry an 18 px number keycap
        assert_eq!(l.cells, vec![Rect::new(7.0, 7.0, 101.0, 32.0), Rect::new(112.0, 7.0, 109.0, 32.0), Rect::new(225.0, 7.0, 138.0, 32.0)]);
        assert_eq!((l.w, l.h), (370.0, 46.0));
        assert_eq!(l.text_x(), 56.0, "the first label, after the Tab keycap");
        assert_eq!(layout(&[40.0], 1392.0, BubbleLayout::Vertical).text_x(), 17.0);
    }
    #[test] fn a_long_label_or_a_narrow_screen_stacks_the_bubble() {
        let long = layout(&[40.0, 241.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(long.kind, BubbleLayout::Vertical, "a label past STRIP_LABEL_MAX");
        assert_eq!(layout(&[40.0, 240.0], 1392.0, BubbleLayout::Horizontal).kind, BubbleLayout::Horizontal, "at the limit");
        // three 200 px labels: 3 * (22 + 8 + 200) + 30 + 2 * 18 + 2 * 4 + 14 = 778 px
        assert_eq!(layout(&[200.0; 3], 778.0, BubbleLayout::Horizontal).w, 778.0);
        let narrow = layout(&[200.0; 3], 777.0, BubbleLayout::Horizontal);
        assert_eq!(narrow.kind, BubbleLayout::Vertical, "the strip is one px wider than the screen allows");
        assert_eq!(narrow.w, CHROME_W + 200.0);
        assert_eq!(layout(&[40.0], 1392.0, BubbleLayout::Vertical).kind, BubbleLayout::Vertical, "the setting");
    }
    #[test] fn rows_widen_to_the_screen_then_wrap() {
        let l = layout(&[20.0, 30.0], 1400.0, BubbleLayout::Vertical);
        assert_eq!((l.w, l.label_w.clone()), (MIN_W, vec![MIN_W - CHROME_W; 2]));
        assert_eq!(l.cells, vec![Rect::new(INSET, INSET, MIN_W - 2.0 * INSET, ROW_H), Rect::new(INSET, INSET + ROW_H, MIN_W - 2.0 * INSET, ROW_H)]);
        assert_eq!(l.h, 2.0 * INSET + 2.0 * ROW_H);
        assert_eq!(layout(&[100.0, 180.0, 312.4], 1400.0, BubbleLayout::Vertical).label_w, vec![313.0; 3]);
        // much wider than the screen: the bubble takes the max width and the first row wraps over 3 lines
        let mw = max_width_for(1440.0); let l = layout(&[2.5 * (mw - CHROME_W), 50.0], mw, BubbleLayout::Horizontal);
        assert_eq!((l.kind, l.w), (BubbleLayout::Vertical, mw));
        assert_eq!(l.cells.iter().map(|c| (c.y, c.h)).collect::<Vec<_>>(), vec![(INSET, 3.0 * ROW_H), (INSET + 3.0 * ROW_H, ROW_H)]);
    }
    #[test] fn a_click_finds_the_row_or_the_button_under_it() {
        let rows = Geom { frame: Rect::new(100.0, 100.0, 300.0, 120.0), scale: 1.0, cells: layout(&[60.0, 260.0, 30.0], 300.0, BubbleLayout::Vertical).cells };
        assert_eq!(rows.hit(50.0, 50.0), None, "off the bubble");
        assert_eq!(rows.hit(150.0, 100.0 + INSET + 5.0), Some(Some(0)));
        assert_eq!(rows.hit(150.0, 100.0 + INSET + ROW_H + 40.0), Some(Some(1)), "the second line of a wrapped row");
        assert_eq!(rows.hit(150.0, 100.0 + INSET + 3.0 * ROW_H + 1.0), Some(Some(2)));
        assert_eq!(rows.hit(150.0, 101.0), Some(None), "the panel's padding");
        // strip at 1.5x (Windows DPI): buttons at x 7..108, 112..221, 225..363 logical
        let strip = Geom { frame: Rect::new(0.0, 0.0, 555.0, 69.0), scale: 1.5, cells: layout(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal).cells };
        assert_eq!(strip.hit(10.0 * 1.5, 20.0 * 1.5), Some(Some(0)));
        assert_eq!(strip.hit(107.0 * 1.5, 20.0 * 1.5), Some(Some(0)));
        assert_eq!(strip.hit(110.0 * 1.5, 20.0 * 1.5), Some(None), "the gap between two buttons");
        assert_eq!(strip.hit(150.0 * 1.5, 20.0 * 1.5), Some(Some(1)));
        assert_eq!(strip.hit(300.0 * 1.5, 20.0 * 1.5), Some(Some(2)));
        assert_eq!(strip.hit(300.0 * 1.5, 3.0 * 1.5), Some(None), "above the buttons");
        assert_eq!(strip.hit(366.0 * 1.5, 20.0 * 1.5), Some(None), "right of the last button");
    }
    #[test] fn rewrite_rows_are_stacked_and_word_options_follow_the_setting() {
        let anchor = Anchor { caret: None, field: None, mouse: Some((0.0, 0.0)) };
        let word = |w: &str| BubbleItem { label: w.into(), kind: BubbleKind::Complete { typed: "he".into(), insert: w.into() } };
        let fix = BubbleItem { label: "hello world".into(), kind: BubbleKind::Rewrite(RewriteRow::Result("hello world".into())) };
        let shown = |items: Vec<BubbleItem>, pref| match UiMsg::show(items, pref, anchor) { UiMsg::Show { items, layout, .. } => (items, layout), m => panic!("{m:?}") };
        assert_eq!(shown(vec![word("help"), word("hello")], BubbleLayout::Horizontal), (vec!["help".to_string(), "hello".into()], BubbleLayout::Horizontal));
        assert_eq!(shown(vec![word("help")], BubbleLayout::Vertical).1, BubbleLayout::Vertical);
        assert_eq!(shown(vec![fix, word("hello")], BubbleLayout::Horizontal).1, BubbleLayout::Vertical);
    }
}
