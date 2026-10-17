//! Messages and shared state between the key-capture thread (CGEventTap / low-level hooks, owns the Engine) and the
//! gpui UI thread (bubble window + tray).
use crate::engine::{BubbleItem, BubbleKind, InputLang, Reason};
use crate::placement::{Anchor, Rect};
use crate::prefs::{SettingsCmd, SettingsSnap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub enum UiMsg {
    /// Highlight this row (Shift+Up/Down, or Left/Right with the arrow keys in the bubble).
    Select(usize),
    /// The arrow keys moved into / out of the bubble: the highlighted option's keycap says Enter while they are in it.
    Focus(bool),
    /// `layout`: what the bubble asks for; `layout` falls back to rows when a strip does not fit.
    Show { items: Vec<Opt>, layout: BubbleLayout, anchor: Anchor },
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
        UiMsg::Show { items: items.iter().map(|i| Opt { reason: i.reason(), label: i.label.clone() }).collect(), layout, anchor }
    }
}

/// One option as the bubble draws it: its label, and why it is offered (the badge on it).
#[derive(Clone, Debug, PartialEq)]
pub struct Opt { pub label: String, pub reason: Option<Reason> }

/// The flag on a conversion's badge, a bundled square SVG (`flags/`) for the language its text becomes: the US flag for
/// English, as the keys are the US layout's. A typo fix's badge shows a spell-check icon instead.
pub fn flag(lang: InputLang) -> &'static str { match lang { InputLang::Thai => "flags/th.svg", InputLang::Latin => "flags/us.svg" } }

/// A run of options with the same reason: `len` options from `start`. A strip draws each as one group.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run { pub start: usize, pub len: usize, pub reason: Option<Reason> }
pub fn runs(reasons: &[Option<Reason>]) -> Vec<Run> {
    let mut out: Vec<Run> = vec![];
    for (i, r) in reasons.iter().enumerate() {
        match out.last_mut() {
            Some(run) if run.reason == *r => run.len += 1,
            _ => out.push(Run { start: i, len: 1, reason: *r }),
        }
    }
    out
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
/// Corner radius of the rows' panel and of a strip's groups.
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
/// An option's badge: gpui-kit's Medium `Badge` size, round. On a strip button it overhangs the top-right corner,
/// `BADGE_UP` above it and `OPT_GAP` past its right edge, so it ends where the next button starts. In a row, which has no
/// box to hang on, it sits in its own column before the key hint.
pub const BADGE_D: f64 = 16.0;
pub const BADGE_UP: f64 = 6.0;
pub const BADGE_ICON: f64 = 10.0;
/// A strip has no panel: each run of options is its own surface (fill, border, shadow) around its buttons, `GROUP_PAD`
/// inside its border, runs `GROUP_GAP_X` apart. The window keeps `SHADOW_M` of transparent margin around what it draws
/// (the strip's groups or the rows' panel) for their shadow and the badges' overhang: the window has no shadow of its own.
pub const GROUP_GAP_X: f64 = 8.0;
pub const GROUP_PAD: f64 = 3.0;
pub const SHADOW_M: f64 = 10.0;
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

/// Strip metrics: each option is a gpui-kit Medium `Button` (1 px border, 10 px padding, 32 px tall) holding the label,
/// `OPT_GAP` apart. Its key hint hangs on its top-left corner like the badge on its top-right, so a button is at least
/// `OPT_MIN_W` wide: the Tab keycap and the badge never meet.
pub const OPT_BORDER: f64 = 1.0;
pub const OPT_PX: f64 = 10.0;
pub const OPT_H: f64 = 32.0;
pub const OPT_GAP: f64 = 4.0;
pub const OPT_MIN_W: f64 = KEY_TAB_W + BADGE_D;
/// A strip button's key hint: a small solid keycap (accent fill, `KEY_RADIUS` corners) centred on its top edge (`KEY_UP` above it), the label set `LABEL_DROP`
/// lower so the pill clears the tallest Thai marks.
pub const KEY_H: f64 = 14.0;
pub const KEY_UP: f64 = 7.0;
pub const KEY_TAB_W: f64 = 26.0;
pub const KEY_DIGIT_W: f64 = 14.0;
pub const KEY_FONT_PX: f64 = 9.5;
pub const KEY_RADIUS: f64 = 3.0;
/// The key hints' fill when the system accent colour cannot be read: macOS's default blue.
pub const ACCENT_FALLBACK: (f32, f32, f32) = (0.0, 0.478, 1.0);
/// Text on an accent fill: dark on a light accent (yellow, graphite's light grey), white otherwise. sRGB 0..1.
pub fn dark_text_on(rgb: (f32, f32, f32)) -> bool { 0.2126 * rgb.0 + 0.7152 * rgb.1 + 0.0722 * rgb.2 > 0.6 }
pub const LABEL_DROP: f64 = 3.0;
/// A label wider than this stacks the bubble: past about 28 Latin letters an option is a phrase, and phrases side by
/// side are no longer read at a glance.
pub const STRIP_LABEL_MAX: f64 = 240.0;

/// The bubble's geometry in logical px, window-relative. `h` and the rows' `cells` and badges are only the first guess
/// for the vertical layout (rows wrap; the real heights come from gpui's laid-out panel, see `Ui::reconcile`). The strip
/// never wraps, so its guess is exact.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    /// What the bubble is drawn as, after the fallback from a strip that does not fit.
    pub kind: BubbleLayout,
    /// The window: what is drawn and the transparent `margin` around it.
    pub w: f64, pub h: f64,
    pub margin: f64,
    /// What is drawn and takes clicks, with the badges: the rows' panel, or each strip run's box.
    pub surfaces: Vec<Rect>,
    pub runs: Vec<Run>,
    /// Each option's label column: one shared width for the rows (the widest label, clamped), each label's own in a strip.
    pub label_w: Vec<f64>,
    /// Each option's box, the first option highlighted: the row, or the button (see `cells_at`).
    pub cells: Vec<Rect>,
    /// Why each option is offered: its badge.
    pub reasons: Vec<Option<Reason>>,
}
impl Layout {
    /// Where the first option's text starts, from the bubble's left edge. Placement puts it at the caret's x, so the
    /// suggestion reads on from what is being typed.
    pub fn text_x(&self) -> f64 {
        match (self.kind, self.cells.first()) {
            (BubbleLayout::Horizontal, Some(c)) => c.x + OPT_BORDER + OPT_PX,
            _ => self.margin + PAD + BORDER + ROW_PX,
        }
    }
}
/// Lay out options from their measured one-line label widths and their reasons (a badge each). A strip when `wanted` is
/// one, every label is at most `STRIP_LABEL_MAX` and the whole strip fits `max_w` (the monitor's visible width minus
/// margins); otherwise rows.
pub fn layout(label_widths: &[f64], reasons: &[Option<Reason>], max_w: f64, wanted: BubbleLayout) -> Layout {
    debug_assert_eq!(label_widths.len(), reasons.len());
    let max_w = max_w.max(MIN_W);
    if wanted == BubbleLayout::Horizontal && label_widths.iter().all(|w| *w <= STRIP_LABEL_MAX) {
        let s = strip(label_widths, reasons);
        if s.w <= max_w { return s }
    }
    rows(label_widths, reasons, max_w)
}
/// Rows: the label column is the widest label, clamped so the bubble is in [MIN_W, max_w]. Longer labels wrap; nothing
/// is truncated. When any row has a badge, every row keeps a badge column before its key hint.
fn rows(label_widths: &[f64], reasons: &[Option<Reason>], max_w: f64) -> Layout {
    let widest = label_widths.iter().copied().fold(0.0, f64::max).ceil();
    let badge_col = if reasons.iter().any(Option::is_some) { BADGE_D + HINT_GAP } else { 0.0 };
    let (chrome, m) = (CHROME_W + badge_col, SHADOW_M);
    let label_w = widest.clamp(MIN_W - chrome, max_w - chrome - 2.0 * m);
    let (inset, pw) = (m + PAD + BORDER, chrome + label_w);
    let mut y = inset;
    let cells: Vec<Rect> = label_widths.iter().map(|lw| { let c = Rect::new(inset, y, pw - 2.0 * (PAD + BORDER), ROW_H * (lw / label_w).ceil().max(1.0)); y = c.bottom(); c }).collect();
    let ph = y + PAD + BORDER - m;
    Layout { kind: BubbleLayout::Vertical, w: pw + 2.0 * m, h: ph + 2.0 * m, margin: m, surfaces: vec![Rect::new(m, m, pw, ph)], runs: runs(reasons), label_w: vec![label_w; label_widths.len()], cells, reasons: reasons.to_vec() }
}
/// One button per option, left to right, in a box per run. The key hints hang outside the buttons, so moving the
/// highlight moves nothing.
fn strip(label_widths: &[f64], reasons: &[Option<Reason>]) -> Layout {
    let (top, inset) = (SHADOW_M, BORDER + GROUP_PAD);
    let label_w: Vec<f64> = label_widths.iter().map(|w| w.ceil().max(OPT_MIN_W - 2.0 * (OPT_BORDER + OPT_PX))).collect();
    let runs = runs(reasons);
    let (mut x, mut cells, mut surfaces) = (SHADOW_M, vec![], vec![]);
    for (k, run) in runs.iter().enumerate() {
        let left = if k == 0 { x } else { x + GROUP_GAP_X };
        x = left + inset;
        for i in run.start..run.start + run.len {
            if i > run.start { x += OPT_GAP }
            let c = Rect::new(x, top + inset, 2.0 * (OPT_BORDER + OPT_PX) + label_w[i], OPT_H);
            x = c.right(); cells.push(c);
        }
        x += inset;
        surfaces.push(Rect::new(left, top, x - left, 2.0 * inset + OPT_H));
    }
    let h = top + 2.0 * inset + OPT_H + SHADOW_M;
    Layout { kind: BubbleLayout::Horizontal, w: x + SHADOW_M, h, margin: SHADOW_M, surfaces, runs, label_w, cells, reasons: reasons.to_vec() }
}
/// A key hint's width: the Tab keycap on the highlighted option, its number on the others.
pub fn hint_w(highlighted: bool) -> f64 { if highlighted { TAB_CAP_W } else { DIGIT_CAP_W } }
pub fn max_width_for(visible_w: f64) -> f64 { visible_w - 2.0 * SCREEN_MARGIN }

/// On-screen bubble geometry in global units (set by the UI, read by the capture thread for click-to-accept).
/// `cells` = each option's box in logical units relative to the bubble's top-left (a row, or a strip button);
/// `scale` converts logical units to the global units of `frame` (Windows DPI; 1.0 on macOS).
#[derive(Clone, Debug, PartialEq)]
/// `solid` = what is drawn: the surfaces and badges; the window's transparent margin is not.
pub struct Geom { pub frame: Rect, pub cells: Vec<Rect>, pub solid: Vec<Rect>, pub scale: f64 }
impl Geom {
    /// Option index under a global point, if the point is on what the bubble draws (Some(None) = on the bubble but not
    /// on an option: the panel's padding, a group's border, a badge's overhang). None off it, the transparent margin too,
    /// so those clicks reach the app.
    pub fn hit(&self, x: f64, y: f64) -> Option<Option<usize>> {
        if !self.frame.contains(x, y) { return None }
        let (lx, ly) = ((x - self.frame.x) / self.scale, (y - self.frame.y) / self.scale);
        if let Some(i) = self.cells.iter().position(|c| c.contains(lx, ly)) { return Some(Some(i)) }
        self.solid.iter().any(|r| r.contains(lx, ly)).then_some(None)
    }
}
impl Layout {
    /// Each strip button's key hint with option `sel` highlighted: on its top-left corner, the wider Tab one on the
    /// highlighted button. Rows
    /// keep theirs inline (none here).
    pub fn key_hints(&self, sel: usize) -> Vec<Rect> {
        if self.kind == BubbleLayout::Vertical { return vec![] }
        self.cells.iter().enumerate().map(|(i, c)| Rect::new(c.x, c.y - KEY_UP, if i == sel { KEY_TAB_W } else { KEY_DIGIT_W }, KEY_H)).collect()
    }
    /// Each badge: on a button's top-right corner, or in a row's badge column.
    pub fn badges(&self) -> Vec<(Reason, Rect)> { self.badges_of().into_iter().map(|(_, r, at)| (r, at)).collect() }
    /// The same, with the option each badge marks.
    pub fn badges_of(&self) -> Vec<(usize, Reason, Rect)> {
        let at = |c: &Rect| match self.kind {
            BubbleLayout::Horizontal => Rect::new(c.right() + OPT_GAP - BADGE_D, c.y - BADGE_UP, BADGE_D, BADGE_D),
            BubbleLayout::Vertical => Rect::new(c.right() - ROW_PX - HINT_SLOT_W - HINT_GAP - BADGE_D, c.y + (ROW_H - BADGE_D) / 2.0, BADGE_D, BADGE_D),
        };
        self.cells.iter().zip(&self.reasons).enumerate().filter_map(|(i, (c, r))| Some((i, (*r)?, at(c)))).collect()
    }
    /// What takes clicks before gpui has laid the bubble out: the surfaces, the badges and the key hints.
    pub fn solid(&self, sel: usize) -> Vec<Rect> {
        self.surfaces.iter().copied().chain(self.badges().into_iter().map(|(_, r)| r)).chain(self.key_hints(sel)).collect()
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
/// The Settings window's update button: handled by the UI loop like the tray's update item.
pub static REQ_UPDATE_CLICK: AtomicBool = AtomicBool::new(false);
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
    use crate::rewrite::{Preset, ReqId};
    // a row's distance from the window's edge: the shadow margin, then the panel's border and padding
    const INSET: f64 = SHADOW_M + PAD + BORDER;
    fn plain(label_widths: &[f64], max_w: f64, wanted: BubbleLayout) -> Layout { layout(label_widths, &vec![None; label_widths.len()], max_w, wanted) }
    const TH: Option<Reason> = Some(Reason::Layout(InputLang::Thai));
    const EN: Option<Reason> = Some(Reason::Layout(InputLang::Latin));
    const TYPO: Option<Reason> = Some(Reason::Typo);
    fn shown(items: Vec<BubbleItem>, pref: BubbleLayout) -> (Vec<Opt>, BubbleLayout) {
        match UiMsg::show(items, pref, Anchor { caret: None, field: None, mouse: Some((0.0, 0.0)) }) { UiMsg::Show { items, layout, .. } => (items, layout), m => panic!("{m:?}") }
    }
    fn item(label: &str, kind: BubbleKind) -> BubbleItem { BubbleItem { label: label.into(), kind } }
    fn opt(label: &str, reason: Option<Reason>) -> Opt { Opt { label: label.into(), reason } }
    #[test] fn short_options_make_a_strip_sized_to_its_buttons() {
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(l.kind, BubbleLayout::Horizontal);
        assert_eq!(l.label_w, vec![41.0, 61.0, 90.0]);
        // [shadow margin 10][group border+pad 4][border+pad 11][label][pad+border 11], 4 px apart: the key hints hang on
        // the corners
        assert_eq!(l.cells, vec![Rect::new(14.0, 14.0, 63.0, 32.0), Rect::new(81.0, 14.0, 83.0, 32.0), Rect::new(168.0, 14.0, 112.0, 32.0)]);
        assert_eq!(l.surfaces, vec![Rect::new(10.0, 10.0, 274.0, 40.0)]);
        assert_eq!((l.w, l.h, l.margin), (294.0, 60.0, 10.0));
        assert_eq!(l.text_x(), 25.0, "the first label, after the button's border and padding");
        assert_eq!(plain(&[40.0], 1392.0, BubbleLayout::Vertical).text_x(), 27.0, "margin, panel border and padding, row padding");
    }
    #[test] fn a_long_label_or_a_narrow_screen_stacks_the_bubble() {
        let long = plain(&[40.0, 241.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(long.kind, BubbleLayout::Vertical, "a label past STRIP_LABEL_MAX");
        assert_eq!(plain(&[40.0, 240.0], 1392.0, BubbleLayout::Horizontal).kind, BubbleLayout::Horizontal, "at the limit");
        // three 200 px labels: 3 * (22 + 200) + 2 * 4 + 2 * 4 + 2 * 10 = 702 px
        assert_eq!(plain(&[200.0; 3], 702.0, BubbleLayout::Horizontal).w, 702.0);
        let narrow = plain(&[200.0; 3], 701.0, BubbleLayout::Horizontal);
        assert_eq!(narrow.kind, BubbleLayout::Vertical, "the strip is one px wider than the screen allows");
        assert_eq!(narrow.w, CHROME_W + 200.0 + 2.0 * SHADOW_M);
        assert_eq!(plain(&[40.0], 1392.0, BubbleLayout::Vertical).kind, BubbleLayout::Vertical, "the setting");
    }
    #[test] fn rows_widen_to_the_screen_then_wrap() {
        let l = plain(&[20.0, 30.0], 1400.0, BubbleLayout::Vertical);
        assert_eq!((l.w, l.label_w.clone()), (MIN_W + 2.0 * SHADOW_M, vec![MIN_W - CHROME_W; 2]));
        let row_w = MIN_W - 2.0 * (PAD + BORDER);
        assert_eq!(l.cells, vec![Rect::new(INSET, INSET, row_w, ROW_H), Rect::new(INSET, INSET + ROW_H, row_w, ROW_H)]);
        assert_eq!(l.surfaces, vec![Rect::new(SHADOW_M, SHADOW_M, MIN_W, 2.0 * (PAD + BORDER + ROW_H))]);
        assert_eq!(l.h, 2.0 * INSET + 2.0 * ROW_H);
        assert_eq!(plain(&[100.0, 180.0, 312.4], 1400.0, BubbleLayout::Vertical).label_w, vec![313.0; 3]);
        // much wider than the screen: the bubble takes the max width and the first row wraps over 3 lines
        let mw = max_width_for(1440.0); let l = plain(&[2.5 * (mw - CHROME_W), 50.0], mw, BubbleLayout::Horizontal);
        assert_eq!((l.kind, l.w), (BubbleLayout::Vertical, mw));
        assert_eq!(l.cells.iter().map(|c| (c.y, c.h)).collect::<Vec<_>>(), vec![(INSET, 3.0 * ROW_H), (INSET + 3.0 * ROW_H, ROW_H)]);
    }
    #[test] fn a_click_finds_the_row_or_the_button_under_it() {
        let l = plain(&[60.0, 260.0, 30.0], 300.0, BubbleLayout::Vertical);
        let rows = Geom { frame: Rect::new(100.0, 100.0, 300.0, 120.0), scale: 1.0, cells: l.cells.clone(), solid: l.solid(0) };
        assert_eq!(rows.hit(50.0, 50.0), None, "off the bubble");
        assert_eq!(rows.hit(150.0, 100.0 + INSET + 5.0), Some(Some(0)));
        assert_eq!(rows.hit(150.0, 100.0 + INSET + ROW_H + 40.0), Some(Some(1)), "the second line of a wrapped row");
        assert_eq!(rows.hit(150.0, 100.0 + INSET + 3.0 * ROW_H + 1.0), Some(Some(2)));
        assert_eq!(rows.hit(150.0, 100.0 + SHADOW_M + 2.0), Some(None), "the panel's padding");
        assert_eq!(rows.hit(150.0, 101.0), None, "the shadow margin: the click is the app's");
        // strip at 1.5x (Windows DPI): one group at x 10..284, y 10..50 logical; buttons at x 14..77, 81..164, 168..280
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        let strip = Geom { frame: Rect::new(0.0, 0.0, 441.0, 90.0), scale: 1.5, cells: l.cells.clone(), solid: l.solid(0) };
        let at = |x: f64, y: f64| strip.hit(x * 1.5, y * 1.5);
        assert_eq!((at(20.0, 20.0), at(76.0, 20.0), at(100.0, 20.0), at(250.0, 20.0)), (Some(Some(0)), Some(Some(0)), Some(Some(1)), Some(Some(2))));
        assert_eq!(at(79.0, 20.0), Some(None), "the gap between two buttons of a group");
        assert_eq!(at(250.0, 12.0), Some(None), "the group's padding above the buttons");
        assert_eq!(at(88.0, 9.0), Some(None), "a key hint's overhang applies nothing");
        assert_eq!(at(300.0, 5.0), None, "the shadow margin: the click is the app's");
        assert_eq!(at(378.0, 20.0), None, "right of the group");
    }
    #[test] fn rewrite_rows_are_stacked_and_word_options_follow_the_setting() {
        let word = |w: &str| item(w, BubbleKind::Complete { typed: "he".into(), insert: w.into() });
        let fix = item("hello world", BubbleKind::Rewrite(RewriteRow::Result("hello world".into())));
        assert_eq!(shown(vec![word("help"), word("hello")], BubbleLayout::Horizontal), (vec![opt("help", None), opt("hello", None)], BubbleLayout::Horizontal));
        assert_eq!(shown(vec![word("help")], BubbleLayout::Vertical).1, BubbleLayout::Vertical);
        assert_eq!(shown(vec![fix, word("hello")], BubbleLayout::Horizontal).1, BubbleLayout::Vertical);
    }
    #[test] fn each_option_says_why_it_is_offered() {
        let words = vec![
            item("สวัสดี", BubbleKind::Live { typed: "l;ylfu".into(), thai: "สวัสดี".into(), prev: 0 }),
            item("i dont", BubbleKind::Live { typed: "dont".into(), thai: "dont".into(), prev: 1 }),
            item("สวัสดี", BubbleKind::Convert { typed: "l;ylfu".into(), thai: "สวัสดี".into() }),
            item("i dont", BubbleKind::ConvertPrev { k: 1 }),
            item("ร้านอาหาร", BubbleKind::Guess { k: 0, typed: Some("i'ko[kg]kp".into()), targets: vec!["ร้านอาหาร".into()] }),
            item("l;ylfu", BubbleKind::UndoConversion),
            item("hello", BubbleKind::Typo { word: "hello".into() }),
            item("ภาษาไทย", BubbleKind::Complete { typed: "ภาษ".into(), insert: "ภาษาไทย".into() }),
        ];
        assert_eq!(shown(words, BubbleLayout::Horizontal).0, vec![
            opt("สวัสดี", TH), opt("i dont", EN), opt("สวัสดี", TH), opt("i dont", EN), opt("ร้านอาหาร", TH), opt("l;ylfu", EN),
            opt("hello", TYPO), opt("ภาษาไทย", None)]);
        let rw = |label: &str, r: RewriteRow| item(label, BubbleKind::Rewrite(r));
        let rows = vec![
            rw("สวัสดี world", RewriteRow::Fixed(Reason::Layout(InputLang::Thai), "สวัสดี world".into())),
            rw("is this", RewriteRow::Fixed(Reason::Typo, "is this".into())),
            rw("Fix typos", RewriteRow::Preset(Preset::Fix, "is ths".into())),
            rw("Rewriting…", RewriteRow::Busy(ReqId::default())),
            rw("Is this?", RewriteRow::Result("Is this?".into())),
            rw("No suggestions", RewriteRow::Notice),
        ];
        assert_eq!(shown(rows, BubbleLayout::Horizontal).0, vec![
            opt("สวัสดี world", TH), opt("is this", TYPO), opt("Fix typos", None), opt("Rewriting…", None), opt("Is this?", None), opt("No suggestions", None)]);
    }
    #[test] fn options_run_in_groups_by_reason() {
        let r = |start, len, reason| Run { start, len, reason };
        assert_eq!(runs(&[TH, TYPO, TYPO, None, None]), vec![r(0, 1, TH), r(1, 2, TYPO), r(3, 2, None)]);
        assert_eq!(runs(&[TH, EN]), vec![r(0, 1, TH), r(1, 1, EN)]);
        assert_eq!(runs(&[]), vec![]);
    }
    #[test] fn a_strip_hangs_each_badge_on_its_buttons_corner() {
        let l = layout(&[40.2, 61.0, 90.0], &[TH, TYPO, TYPO], 1392.0, BubbleLayout::Horizontal);
        // a box per run, GROUP_GAP_X (8) apart, inside the 10 px shadow margin
        assert_eq!(l.surfaces, vec![Rect::new(10.0, 10.0, 71.0, 40.0), Rect::new(89.0, 10.0, 207.0, 40.0)]);
        assert_eq!(l.cells, vec![Rect::new(14.0, 14.0, 63.0, 32.0), Rect::new(93.0, 14.0, 83.0, 32.0), Rect::new(180.0, 14.0, 112.0, 32.0)]);
        // 6 px above each button and 4 px past it: the second badge ends where the third button starts
        assert_eq!(l.badges(), vec![(Reason::Layout(InputLang::Thai), Rect::new(65.0, 8.0, 16.0, 16.0)),
            (Reason::Typo, Rect::new(164.0, 8.0, 16.0, 16.0)), (Reason::Typo, Rect::new(280.0, 8.0, 16.0, 16.0))]);
        assert_eq!((l.w, l.h), (306.0, 60.0));
        // the key hints on the top-left corners: Tab on the highlighted button, its number on the others; the third
        // button's starts where the second's badge ends
        assert_eq!(l.key_hints(0), vec![Rect::new(14.0, 7.0, 26.0, 14.0), Rect::new(93.0, 7.0, 14.0, 14.0), Rect::new(180.0, 7.0, 14.0, 14.0)]);
        assert_eq!(l.key_hints(1).iter().map(|k| k.w).collect::<Vec<_>>(), vec![14.0, 26.0, 14.0], "the highlight moves only the Tab keycap");
        let window = Rect::new(0.0, 0.0, l.w, l.h);
        assert!(l.badges().iter().map(|(_, b)| *b).chain(l.key_hints(2)).all(|b| window.contains(b.x, b.y) && window.contains(b.right(), b.bottom())), "nothing is clipped");
        assert_eq!(l.text_x(), 25.0);
        let g = Geom { frame: window, scale: 1.0, cells: l.cells.clone(), solid: l.solid(0) };
        assert_eq!(g.hit(70.0, 9.0), Some(None), "the badge's overhang applies nothing");
        assert_eq!(g.hit(20.0, 30.0), Some(Some(0)));
        assert_eq!(g.hit(85.0, 30.0), None, "between the groups: the app's");
        assert_eq!(g.hit(50.0, 5.0), None, "the shadow margin: the app's");
        // three 200 px labels fill 702 px as one group; as three, each group's border, padding and gap take 16 px more
        assert_eq!(layout(&[200.0; 3], &[TH, TYPO, None], 726.0, BubbleLayout::Horizontal).kind, BubbleLayout::Horizontal);
        assert_eq!(layout(&[200.0; 3], &[TH, TYPO, None], 725.0, BubbleLayout::Horizontal).kind, BubbleLayout::Vertical);
    }
    #[test] fn a_short_label_keeps_its_key_hint_clear_of_its_badge() {
        // "ok": the button widens so the Tab keycap and the corner badge never meet
        let l = layout(&[14.0], &[TYPO], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(l.cells[0].w, OPT_MIN_W);
        let (k, b) = (l.key_hints(0)[0], l.badges()[0].1);
        assert!(k.right() < b.x, "Tab keycap {k:?} clear of badge {b:?}");
    }
    #[test] fn accent_text_stays_readable() {
        assert!(!dark_text_on(ACCENT_FALLBACK), "white on blue");
        assert!(dark_text_on((1.0, 0.8, 0.0)), "dark on yellow");
        assert!(!dark_text_on((0.55, 0.55, 0.55)), "white on graphite");
    }
    #[test] fn rows_keep_a_badge_column_when_any_row_has_a_badge() {
        let l = layout(&[100.0, 180.0, 50.0], &[TH, TYPO, None], 1400.0, BubbleLayout::Vertical);
        // CHROME_W 92 + the badge column (16 + 8) + the widest label, in the 10 px shadow margin; each badge centred
        // before the key hint
        assert_eq!((l.w, l.h, l.label_w[0]), (316.0, 124.0, 180.0));
        assert_eq!(l.badges(), vec![(Reason::Layout(InputLang::Thai), Rect::new(215.0, 24.0, 16.0, 16.0)), (Reason::Typo, Rect::new(215.0, 54.0, 16.0, 16.0))]);
        assert!(l.key_hints(0).is_empty(), "rows keep their key hints inline");
        assert_eq!(plain(&[100.0, 180.0, 50.0], 1400.0, BubbleLayout::Vertical).w, 292.0, "no badges, no column");
    }
}
