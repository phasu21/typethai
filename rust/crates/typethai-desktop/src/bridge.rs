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
    /// The input-language preview at the caret (`badge::anchor_for`: no mouse fallback): `pin` when focus just landed
    /// in a text field nothing has been typed in yet (it stays), else an input-source change (it goes after a moment).
    Badge { lang: InputLang, anchor: Anchor, pin: bool },
    /// A key was typed, or focus left the text field: a pinned preview goes. (No-op on one that goes by itself.)
    UnpinBadge,
    /// The caret moved while a badge is up (the preview tracks the caret as the app lays it out): re-place the window.
    MoveBadge(Anchor),
    /// A click on the badge (Windows, where it is not click-through): take it down.
    HideBadge,
    /// macOS refused the event tap (`true`: no Accessibility for this build, and the capture thread keeps retrying).
    Permission(bool),
    /// Tab held towards opening the ⌃⌥R rows (None: released, or they opened). `anchor`: where the caret is when no
    /// bubble is up (the indicator goes in the badge window); over the bubble it fills the highlighted option's Tab key.
    Hold { hold: Option<crate::engine::Hold>, anchor: Option<Anchor> },
}

impl UiMsg {
    /// The bubble for the engine's `items`, laid out as the setting `pref` says. Rewrite rows (the AI presets,
    /// "Rewriting…", results and notices) are always stacked: they are sentences, and one rewrite moves from presets to
    /// a result in place, which should not flip the layout under the reader.
    pub fn show(items: Vec<BubbleItem>, pref: BubbleLayout, anchor: Anchor) -> UiMsg {
        let layout = if items.iter().any(|i| matches!(i.kind, BubbleKind::Rewrite(_))) { BubbleLayout::Vertical } else { pref };
        UiMsg::Show { items: items.iter().map(|i| Opt { reason: i.reason(), label: i.label.clone(), rated: i.rated_word().is_some() }).collect(), layout, anchor }
    }
}

/// One option as the bubble draws it: its label, why it is offered (the badge on it), and whether Shift shows thumbs on
/// it to rate it (`Hotkey::Rate`).
#[derive(Clone, Debug, PartialEq)]
pub struct Opt { pub label: String, pub reason: Option<Reason>, pub rated: bool }

/// The flag on a conversion's badge, a bundled square SVG (`flags/`) for the language its text becomes: the US flag for
/// English, as the keys are the US layout's. A typo fix's badge shows a spell-check icon instead.
pub fn flag(lang: InputLang) -> &'static str { match lang { InputLang::Thai => "flags/th.svg", InputLang::Latin => "flags/us.svg" } }
/// The same flag stretched to 3:2 (the square SVG with `preserveAspectRatio="none"`), for a strip button's mark: an
/// image keeps its own aspect ratio whatever size it is given.
pub fn wide_flag(lang: InputLang) -> &'static str { match lang { InputLang::Thai => "flags/th-wide.svg", InputLang::Latin => "flags/us-wide.svg" } }

/// How the bubble lays out its options: one horizontal strip of buttons, or stacked rows. As a setting it is what the
/// user asked for; a bubble whose options do not suit a strip is laid out vertically anyway (see `layout`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BubbleLayout { #[default] Horizontal, Vertical }

/// Bubble layout constants (logical px) shared by the view and the click hit-test.
pub const PAD: f64 = 6.0;
pub const ROW_H: f64 = 30.0;
/// Bubble chrome metrics (logical units). One row = [px ROW_PX][label column (wraps)][gap][Tab keycap][px ROW_PX], at least ROW_H tall.
pub const BORDER: f64 = 1.0;
/// Corner radius of the rows' panel.
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
/// Options with a digit shortcut (Tab+1..3, Ctrl+1..3) and so a digit hint. A fourth (next words beside a layout flip)
/// is picked with the arrows or a click.
pub const DIGIT_ROWS: usize = 3;
/// A row's badge: gpui-kit's Medium `Badge` size, round, in its own column before the key hint.
pub const BADGE_D: f64 = 16.0;
pub const BADGE_ICON: f64 = 10.0;
/// A strip button's reason mark instead: the typo icon `MARK_D` square, or the flag as a small 3:2 rectangle
/// (`MARK_FLAG_W` × `MARK_FLAG_H`) that stays under the label's line, faint (`MARK_ALPHA`) behind the label,
/// `MARK_INSET` inside the button's bottom-right corner.
pub const MARK_D: f64 = 14.0;
pub const MARK_FLAG_W: f64 = 12.0;
pub const MARK_FLAG_H: f64 = 8.0;
pub const MARK_INSET: f64 = 3.0;
pub const MARK_ALPHA: f32 = 0.3;
/// A strip has no panel and no box around its buttons: each button stands on a solid backing with its own shadow,
/// `OPT_GAP` from the next. The window keeps `SHADOW_M` of transparent margin around what it draws (the strip's buttons or
/// the rows' panel) for their shadow and the key hints' overhang: the window has no shadow of its own.
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

/// Strip metrics: each option is a gpui-kit `Button` (1 px border) holding the label, as wide as its slot, `OPT_GAP` from
/// the next, so every gap is the same whichever option is highlighted. The highlighted button fills its slot:
/// `OPT_SEL_H` tall, `OPT_SEL_PX` padding, the label at `SEL_FONT_PX`. The others are `OPT_H` tall on the slot's floor,
/// the label at `FONT_PX` centred, so moving the highlight grows one and shrinks another without moving any edge. Its key
/// hint hangs on its top-left corner and its reason mark sits in its bottom-right one, so a button is at least
/// `OPT_MIN_W` wide: the Tab keycap and the mark never share a column.
pub const OPT_BORDER: f64 = 1.0;
pub const OPT_PX: f64 = 10.0;
pub const OPT_H: f64 = 30.0;
pub const OPT_SEL_PX: f64 = 12.0;
pub const OPT_SEL_H: f64 = 34.0;
pub const SEL_FONT_PX: f64 = 16.0;
/// An option not highlighted is drawn at this opacity, badge and all.
pub const DIM: f32 = 0.55;
pub const OPT_GAP: f64 = 4.0;
pub const OPT_MIN_W: f64 = KEY_DX + KEY_TAB_W + MARK_D.max(MARK_FLAG_W);
/// A strip button's key hint: a small solid keycap (accent fill, `KEY_RADIUS` corners) on its top edge, `KEY_DX` in from
/// its left edge and `KEY_UP` above it, the label set `LABEL_DROP` lower so the pill clears the tallest Thai marks.
pub const KEY_H: f64 = 14.0;
pub const KEY_UP: f64 = 7.0;
pub const KEY_DX: f64 = 6.0;
pub const KEY_TAB_W: f64 = 26.0;
pub const KEY_DIGIT_W: f64 = 14.0;
pub const KEY_FONT_PX: f64 = 9.5;
pub const KEY_RADIUS: f64 = 3.0;
/// The thumbs Shift shows on a rated option (`Opt::rated`), in place of its key hint: up then down, `THUMB_GAP` apart.
/// A strip button has them as pills like its key hint (`KEY_H` tall); a row as keycaps in its key hint slot.
pub const THUMB_W: f64 = 16.0;
pub const THUMB_GAP: f64 = 2.0;
pub const THUMB_ICON: f64 = 10.0;
/// The key hints' fill when the system accent colour cannot be read: macOS's default blue.
pub const ACCENT_FALLBACK: (f32, f32, f32) = (0.0, 0.478, 1.0);
/// Text on an accent fill: dark on a light accent (yellow, graphite's light grey), white otherwise. sRGB 0..1.
pub fn dark_text_on(rgb: (f32, f32, f32)) -> bool { 0.2126 * rgb.0 + 0.7152 * rgb.1 + 0.0722 * rgb.2 > 0.6 }
pub const LABEL_DROP: f64 = 3.0;
/// A label wider than this stacks the bubble: past about 28 Latin letters an option is a phrase, and phrases side by
/// side are no longer read at a glance.
pub const STRIP_LABEL_MAX: f64 = 240.0;
/// The download chip at a strip's end: a small button beside the rail whose click asks the updater for its next step
/// (`update::chip` decides when it shows; the capture side raises the tray item's request). Strips only: rows are read
/// as text, and they get their update from the tray.
pub const UPDATE_CHIP_D: f64 = 28.0;
pub const UPDATE_CHIP_ICON: f64 = 15.0;

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
    /// What is drawn and takes clicks besides the options, with the badges: the rows' panel. A strip draws only its
    /// buttons, inside `cells`.
    pub surfaces: Vec<Rect>,
    /// Each option's label column at `FONT_PX`: one shared width for the rows (the widest label, clamped), each label's
    /// own in a strip (a highlighted one is set larger, see `label_width`).
    pub label_w: Vec<f64>,
    /// Each option's box, which takes its clicks: the row, or the strip slot its button sits in (see `button`).
    pub cells: Vec<Rect>,
    /// Why each option is offered: its badge.
    pub reasons: Vec<Option<Reason>>,
    /// The download chip's box at the strip's end, when the rail carries one (`layout`'s `update`): a click there is
    /// what the tray's update item does right now (`Hit::Update`). None on rows.
    pub update: Option<Rect>,
}
impl Layout {
    /// Where the first option's text starts, from the bubble's left edge. Placement puts it at the caret's x, so the
    /// suggestion reads on from what is being typed. A new bubble highlights its first option.
    pub fn text_x(&self) -> f64 {
        match (self.kind, self.cells.first()) {
            (BubbleLayout::Horizontal, Some(c)) => c.x + OPT_BORDER + OPT_SEL_PX,
            _ => self.margin + PAD + BORDER + ROW_PX,
        }
    }
    /// Option `i`'s label column with option `sel` highlighted: a highlighted strip label is set larger.
    pub fn label_width(&self, i: usize, sel: usize) -> f64 {
        if self.kind == BubbleLayout::Horizontal && i == sel { sel_label_w(self.label_w[i]) } else { self.label_w[i] }
    }
    /// Option `i`'s button with option `sel` highlighted: in a strip the highlighted one fills its slot (`cells[i]`) and
    /// the others are shorter, on the floor of theirs; a row is its cell.
    pub fn button(&self, i: usize, sel: usize) -> Rect {
        let c = self.cells[i];
        if self.kind == BubbleLayout::Vertical || i == sel { return c }
        Rect::new(c.x, c.bottom() - OPT_H, c.w, OPT_H)
    }
}
/// Lay out options from their measured one-line label widths and their reasons (a badge each). A strip when `wanted` is
/// one, every label is at most `STRIP_LABEL_MAX` and the whole strip fits `max_w` (the monitor's visible width minus
/// margins); otherwise rows. `update` rides the download chip onto a strip that fits.
pub fn layout(label_widths: &[f64], reasons: &[Option<Reason>], max_w: f64, wanted: BubbleLayout, update: bool) -> Layout {
    debug_assert_eq!(label_widths.len(), reasons.len());
    let max_w = max_w.max(MIN_W);
    if wanted == BubbleLayout::Horizontal && label_widths.iter().all(|w| *w <= STRIP_LABEL_MAX) {
        let s = strip(label_widths, reasons, update);
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
    Layout { kind: BubbleLayout::Vertical, w: pw + 2.0 * m, h: ph + 2.0 * m, margin: m, surfaces: vec![Rect::new(m, m, pw, ph)], label_w: vec![label_w; label_widths.len()], cells, reasons: reasons.to_vec(), update: None }
}
/// One slot per option, left to right, `OPT_GAP` apart, each wide enough for its option highlighted. The key hints hang
/// outside the buttons, and the slots stay put as the highlight moves. `update`: the download chip takes a slot of its
/// own after the buttons, standing on the rail's floor like they do.
fn strip(label_widths: &[f64], reasons: &[Option<Reason>], update: bool) -> Layout {
    let label_w: Vec<f64> = label_widths.iter().map(|w| w.ceil().max(OPT_MIN_W - 2.0 * (OPT_BORDER + OPT_PX))).collect();
    let mut x = SHADOW_M - OPT_GAP;
    let cells: Vec<Rect> = label_w.iter().map(|lw| { let c = Rect::new(x + OPT_GAP, SHADOW_M, 2.0 * (OPT_BORDER + OPT_SEL_PX) + sel_label_w(*lw), OPT_SEL_H); x = c.right(); c }).collect();
    let update = update.then(|| Rect::new(cells.last().map_or(SHADOW_M, Rect::right) + OPT_GAP, SHADOW_M + OPT_SEL_H - UPDATE_CHIP_D, UPDATE_CHIP_D, UPDATE_CHIP_D));
    let w = update.as_ref().map_or(cells.last().map_or(SHADOW_M, Rect::right), Rect::right) + SHADOW_M;
    Layout { kind: BubbleLayout::Horizontal, w, h: OPT_SEL_H + 2.0 * SHADOW_M, margin: SHADOW_M, surfaces: vec![], label_w, cells, reasons: reasons.to_vec(), update }
}
/// A label column measured at `FONT_PX`, set at `SEL_FONT_PX`: text widths scale with the font size.
fn sel_label_w(w: f64) -> f64 { (w * SEL_FONT_PX / FONT_PX).ceil() }
/// A key hint's width: the Tab keycap on the highlighted option, its number on the others.
pub fn hint_w(highlighted: bool) -> f64 { if highlighted { TAB_CAP_W } else { DIGIT_CAP_W } }
pub fn max_width_for(visible_w: f64) -> f64 { visible_w - 2.0 * SCREEN_MARGIN }

/// On-screen bubble geometry in global units (set by the UI, read by the capture thread for click-to-accept).
/// `cells` = each option's box in logical units relative to the bubble's top-left (a row, or a strip button);
/// `scale` converts logical units to the global units of `frame` (Windows DPI; 1.0 on macOS). `update` = the strip's
/// download chip, when it shows one, in the same logical units.
#[derive(Clone, Debug, PartialEq)]
/// `solid` = what is drawn besides the options: the rows' panel and badges, a strip's key hints; the window's transparent margin is not.
/// `thumbs` = the thumbs Shift shows on the rated options: (option, up, box).
pub struct Geom { pub frame: Rect, pub cells: Vec<Rect>, pub solid: Vec<Rect>, pub thumbs: Vec<(usize, bool, Rect)>, pub update: Option<Rect>, pub scale: f64 }
/// What a click on the bubble lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// An option: it is applied.
    Pick(usize),
    /// A thumb on an option, up or down, while Shift is held.
    Rate(usize, bool),
    /// The download chip at a strip's end: the updater's next step. No engine key (`key` is None): the capture side
    /// raises the request the tray item's click follows (`REQ_UPDATE_CLICK`), the UI loop acts on it.
    Update,
    /// What else the bubble draws (the panel's padding, a badge's or key hint's overhang): nothing happens.
    Bubble,
}
impl Hit {
    /// The engine key a click here sends, if any.
    pub fn key(self) -> Option<crate::engine::Key> {
        use crate::engine::{Hotkey, Key};
        match self { Hit::Pick(i) => Some(Key::Hotkey(Hotkey::Accept(i))), Hit::Rate(i, up) => Some(Key::Hotkey(Hotkey::Rate(i, up))), Hit::Update | Hit::Bubble => None }
    }
}
impl Geom {
    /// What a global point is on, if it is on what the bubble draws. None off it, the transparent margin and the gaps
    /// between strip buttons too, so those clicks reach the app. The thumbs take clicks only while `shift` is held, as
    /// that is when they show; they sit over a button's edge, so they win over it.
    pub fn hit(&self, x: f64, y: f64, shift: bool) -> Option<Hit> {
        if !self.frame.contains(x, y) { return None }
        let (lx, ly) = ((x - self.frame.x) / self.scale, (y - self.frame.y) / self.scale);
        if let Some(&(i, up, _)) = self.thumbs.iter().find(|(_, _, r)| shift && r.contains(lx, ly)) { return Some(Hit::Rate(i, up)) }
        if let Some(i) = self.cells.iter().position(|c| c.contains(lx, ly)) { return Some(Hit::Pick(i)) }
        if self.update.is_some_and(|r| r.contains(lx, ly)) { return Some(Hit::Update) }
        self.solid.iter().any(|r| r.contains(lx, ly)).then_some(Hit::Bubble)
    }
}
impl Layout {
    /// Each strip button's key hint with option `sel` highlighted, with the option it belongs to: on its top edge near
    /// the left corner, the wider Tab one on the highlighted button. A button past `DIGIT_ROWS` has no digit, so only
    /// its Tab when highlighted. Rows keep theirs inline (none here).
    pub fn key_hints(&self, sel: usize) -> Vec<(usize, Rect)> {
        if self.kind == BubbleLayout::Vertical { return vec![] }
        (0..self.cells.len()).filter(|&i| i == sel || i < DIGIT_ROWS)
            .map(|i| { let b = self.button(i, sel); (i, Rect::new(b.x + KEY_DX, b.y - KEY_UP, if i == sel { KEY_TAB_W } else { KEY_DIGIT_W }, KEY_H)) }).collect()
    }
    /// The thumbs on each rated option with option `sel` highlighted: (option, up, box). A strip button has them where
    /// its key hint is; a row at the right of its key hint slot, centred on the row. `cells`: the options as laid out
    /// (a row's height follows its wrapped label; a strip button is `button`).
    pub fn thumbs(&self, cells: &[Rect], sel: usize, rated: &[bool]) -> Vec<(usize, bool, Rect)> {
        let pair = |x: f64, y: f64, w: f64, h: f64| [(true, Rect::new(x, y, w, h)), (false, Rect::new(x + w + THUMB_GAP, y, w, h))];
        (0..cells.len().min(rated.len())).filter(|&i| rated[i]).flat_map(|i| {
            let at = match self.kind {
                BubbleLayout::Horizontal => { let b = self.button(i, sel); pair(b.x + KEY_DX, b.y - KEY_UP, THUMB_W, KEY_H) }
                BubbleLayout::Vertical => { let c = cells[i]; pair(c.right() - ROW_PX - 2.0 * DIGIT_CAP_W - THUMB_GAP, c.y + (c.h - KEYCAP_H) / 2.0, DIGIT_CAP_W, KEYCAP_H) }
            };
            at.map(|(up, r)| (i, up, r))
        }).collect()
    }
    /// Each row's badge, centred in the badge column. A strip has none: its buttons carry their reason inside.
    pub fn badges(&self) -> Vec<(Reason, Rect)> {
        if self.kind == BubbleLayout::Horizontal { return vec![] }
        let at = |c: &Rect| Rect::new(c.right() - ROW_PX - HINT_SLOT_W - HINT_GAP - BADGE_D, c.y + (ROW_H - BADGE_D) / 2.0, BADGE_D, BADGE_D);
        self.cells.iter().zip(&self.reasons).filter_map(|(c, r)| Some(((*r)?, at(c)))).collect()
    }
    /// What takes clicks before gpui has laid the bubble out: the surfaces, the badges and the key hints.
    pub fn solid(&self, sel: usize) -> Vec<Rect> {
        self.surfaces.iter().copied().chain(self.badges().into_iter().map(|(_, r)| r)).chain(self.key_hints(sel).into_iter().map(|(_, r)| r)).collect()
    }
}
pub static SHIFT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The app other than typeThai itself that keys or focus went to last, and whether the user turned typeThai off there:
/// the tray's app item and the Settings page are about this app. Written by the capture side (`App::set_front_app`),
/// read by the UI loop/tray.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontApp { pub id: String, pub name: String, pub off: bool }
pub static FRONT_APP: Mutex<Option<FrontApp>> = Mutex::new(None);
pub fn front_app() -> Option<FrontApp> { FRONT_APP.lock().unwrap().clone() }
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
/// macOS: the main thread saw the source change to this language; the tap thread looks up the caret and asks for the badge.
pub static REQ_BADGE: Mutex<Option<crate::engine::InputLang>> = Mutex::new(None);
/// Where the badge is on screen while it is up (global units), for the Windows mouse hook.
pub static BADGE_AT: Mutex<Option<Rect>> = Mutex::new(None);
/// Where the download chip's hover tip is up, read by the Windows mouse hook as `BADGE_AT` is: a click on it is
/// swallowed (the tip is only a label; on macOS the tip panel ignores the mouse on its own).
pub static TIP_AT: Mutex<Option<Rect>> = Mutex::new(None);
/// `--settings`: open the Settings window once the UI is up.
pub static REQ_OPEN_SETTINGS: AtomicBool = AtomicBool::new(false);
/// `--settings-page N`: which page that window starts on (0 Word suggestions, 3 AI rewrite, …).
pub static SETTINGS_PAGE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The Settings window's update button: handled by the UI loop like the tray's update item. The rail's download chip
/// clicks land here too — the capture side raises it, the UI loop does what the tray item does right now.
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
    fn plain(label_widths: &[f64], max_w: f64, wanted: BubbleLayout) -> Layout { layout(label_widths, &vec![None; label_widths.len()], max_w, wanted, false) }
    const TH: Option<Reason> = Some(Reason::Layout(InputLang::Thai));
    const EN: Option<Reason> = Some(Reason::Layout(InputLang::Latin));
    const TYPO: Option<Reason> = Some(Reason::Typo);
    fn shown(items: Vec<BubbleItem>, pref: BubbleLayout) -> (Vec<Opt>, BubbleLayout) {
        match UiMsg::show(items, pref, Anchor { caret: None, field: None, mouse: Some((0.0, 0.0)) }) { UiMsg::Show { items, layout, .. } => (items, layout), m => panic!("{m:?}") }
    }
    fn item(label: &str, kind: BubbleKind) -> BubbleItem { BubbleItem { label: label.into(), kind } }
    fn opt(label: &str, reason: Option<Reason>) -> Opt { Opt { label: label.into(), reason, rated: false } }
    #[test] fn short_options_make_a_strip_sized_to_its_buttons() {
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(l.kind, BubbleLayout::Horizontal);
        assert_eq!(l.label_w, vec![41.0, 61.0, 90.0]);
        // [shadow margin 10] then a slot per option, 4 px apart, sized for the option highlighted: [border+pad 13][label
        // at 16/15 the width][pad+border 13]. No box around them; the key hints hang on the corners.
        assert_eq!(l.cells, vec![Rect::new(10.0, 10.0, 70.0, 34.0), Rect::new(84.0, 10.0, 92.0, 34.0), Rect::new(180.0, 10.0, 122.0, 34.0)]);
        assert_eq!(l.surfaces, vec![]);
        assert_eq!((l.w, l.h, l.margin), (312.0, 54.0, 10.0));
        assert_eq!(l.text_x(), 23.0, "the first label, after the highlighted button's border and padding");
        assert_eq!(plain(&[40.0], 1392.0, BubbleLayout::Vertical).text_x(), 27.0, "margin, panel border and padding, row padding");
    }
    #[test] fn a_long_label_or_a_narrow_screen_stacks_the_bubble() {
        let long = plain(&[40.0, 241.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(long.kind, BubbleLayout::Vertical, "a label past STRIP_LABEL_MAX");
        assert_eq!(plain(&[40.0, 240.0], 1392.0, BubbleLayout::Horizontal).kind, BubbleLayout::Horizontal, "at the limit");
        // three 200 px labels: 3 * (26 + 214) + 2 * 4 + 2 * 10 = 748 px
        assert_eq!(plain(&[200.0; 3], 748.0, BubbleLayout::Horizontal).w, 748.0);
        let narrow = plain(&[200.0; 3], 747.0, BubbleLayout::Horizontal);
        assert_eq!(narrow.kind, BubbleLayout::Vertical, "the strip is one px wider than the screen allows");
        assert_eq!(narrow.w, CHROME_W + 200.0 + 2.0 * SHADOW_M);
        assert_eq!(plain(&[40.0], 1392.0, BubbleLayout::Vertical).kind, BubbleLayout::Vertical, "the setting");
    }
    #[test] fn the_highlighted_button_fills_its_slot_and_the_gaps_stay_equal() {
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        assert_eq!((l.button(1, 1), l.label_width(1, 1)), (l.cells[1], 66.0));
        assert_eq!((l.button(1, 0), l.label_width(1, 0)), (Rect::new(84.0, 14.0, 92.0, 30.0), 61.0), "as wide, shorter, on the slot's floor");
        for sel in 0..3 {
            let b: Vec<Rect> = (0..3).map(|i| l.button(i, sel)).collect();
            assert_eq!(b.windows(2).map(|p| p[1].x - p[0].right()).collect::<Vec<_>>(), vec![OPT_GAP; 2], "every gap is the same with option {sel} highlighted");
            assert!(b.iter().all(|b| b.bottom() == l.cells[0].bottom()), "on one floor");
        }
        let rows = plain(&[40.0], 1392.0, BubbleLayout::Vertical);
        assert_eq!((rows.button(0, 1), rows.label_width(0, 0)), (rows.cells[0], rows.label_w[0]), "a row is its cell");
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
        let rows = Geom { frame: Rect::new(100.0, 100.0, 300.0, 120.0), scale: 1.0, cells: l.cells.clone(), solid: l.solid(0), thumbs: vec![], update: None };
        let hit = |x: f64, y: f64| rows.hit(x, y, false);
        assert_eq!(hit(50.0, 50.0), None, "off the bubble");
        assert_eq!(hit(150.0, 100.0 + INSET + 5.0), Some(Hit::Pick(0)));
        assert_eq!(hit(150.0, 100.0 + INSET + ROW_H + 40.0), Some(Hit::Pick(1)), "the second line of a wrapped row");
        assert_eq!(hit(150.0, 100.0 + INSET + 3.0 * ROW_H + 1.0), Some(Hit::Pick(2)));
        assert_eq!(hit(150.0, 100.0 + SHADOW_M + 2.0), Some(Hit::Bubble), "the panel's padding");
        assert_eq!(hit(150.0, 101.0), None, "the shadow margin: the click is the app's");
        // strip at 1.5x (Windows DPI): slots at x 10..80, 84..176, 180..302, y 10..44 logical, and no box around them
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        let strip = Geom { frame: Rect::new(0.0, 0.0, 468.0, 81.0), scale: 1.5, cells: l.cells.clone(), solid: l.solid(0), thumbs: vec![], update: None };
        let at = |x: f64, y: f64| strip.hit(x * 1.5, y * 1.5, false);
        assert_eq!((at(15.0, 20.0), at(79.0, 20.0), at(100.0, 20.0), at(250.0, 20.0)), (Some(Hit::Pick(0)), Some(Hit::Pick(0)), Some(Hit::Pick(1)), Some(Hit::Pick(2))));
        assert_eq!(at(85.0, 12.0), Some(Hit::Pick(1)), "a slot takes the click above its smaller button");
        assert_eq!(at(82.0, 20.0), None, "the gap between two slots: the app's");
        assert_eq!(at(95.0, 8.0), Some(Hit::Bubble), "a key hint's overhang applies nothing");
        assert_eq!(at(300.0, 5.0), None, "the shadow margin: the click is the app's");
        assert_eq!(at(306.0, 20.0), None, "right of the strip");
    }
    #[test] fn rewrite_rows_are_stacked_and_word_options_follow_the_setting() {
        let word = |w: &str| item(w, BubbleKind::Complete { typed: "he".into(), insert: w.into() });
        let fix = item("hello world", BubbleKind::Rewrite(RewriteRow::Result("hello world".into())));
        assert_eq!(shown(vec![word("help"), word("hello")], BubbleLayout::Horizontal), (vec![opt("help", None), opt("hello", None)], BubbleLayout::Horizontal));
        assert_eq!(shown(vec![word("help")], BubbleLayout::Vertical).1, BubbleLayout::Vertical);
        assert_eq!(shown(vec![fix, word("hello")], BubbleLayout::Horizontal).1, BubbleLayout::Vertical);
    }
    #[test] fn thumbs_take_clicks_only_while_shift_is_held() {
        let l = plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal);
        // on the first button only, where its key hint is: up then down
        let t = l.thumbs(&l.cells, 0, &[true, false, false]);
        assert_eq!(t, vec![(0, true, Rect::new(16.0, 3.0, 16.0, 14.0)), (0, false, Rect::new(34.0, 3.0, 16.0, 14.0))]);
        let g = Geom { frame: Rect::new(0.0, 0.0, l.w, l.h), scale: 1.0, cells: l.cells.clone(), solid: l.solid(0), thumbs: t, update: None };
        assert_eq!((g.hit(20.0, 12.0, true), g.hit(40.0, 12.0, true)), (Some(Hit::Rate(0, true)), Some(Hit::Rate(0, false))));
        assert_eq!((g.hit(20.0, 12.0, false), g.hit(40.0, 12.0, false)), (Some(Hit::Pick(0)), Some(Hit::Pick(0))), "no Shift: the button's");
        // rows: right of the key hint slot, centred on the row
        let r = plain(&[60.0, 80.0], 1392.0, BubbleLayout::Vertical);
        let c = r.cells[1];
        assert_eq!(r.thumbs(&r.cells, 0, &[false, true]), vec![
            (1, true, Rect::new(c.right() - ROW_PX - 38.0, c.y + (c.h - KEYCAP_H) / 2.0, 18.0, 18.0)),
            (1, false, Rect::new(c.right() - ROW_PX - 18.0, c.y + (c.h - KEYCAP_H) / 2.0, 18.0, 18.0))]);
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
        // only the conversion of a finished word gets thumbs
        assert_eq!(shown(words, BubbleLayout::Horizontal).0, vec![
            opt("สวัสดี", TH), opt("i dont", EN), Opt { rated: true, ..opt("สวัสดี", TH) }, opt("i dont", EN), opt("ร้านอาหาร", TH), opt("l;ylfu", EN),
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
    #[test] fn a_strip_keeps_its_reasons_inside_the_buttons() {
        let l = layout(&[40.2, 61.0, 90.0], &[TH, TYPO, TYPO], 1392.0, BubbleLayout::Horizontal, false);
        // options with different reasons keep the same OPT_GAP (4) as any others, inside the 10 px shadow margin
        assert_eq!(l.cells, plain(&[40.2, 61.0, 90.0], 1392.0, BubbleLayout::Horizontal).cells);
        assert_eq!(l.cells, vec![Rect::new(10.0, 10.0, 70.0, 34.0), Rect::new(84.0, 10.0, 92.0, 34.0), Rect::new(180.0, 10.0, 122.0, 34.0)]);
        assert_eq!(l.badges(), vec![], "nothing hangs outside a button");
        assert_eq!((l.w, l.h), (312.0, 54.0));
        // the key hints 6 px in from the top-left corners: Tab on the highlighted button, its number on the others
        assert_eq!(l.key_hints(0), vec![(0, Rect::new(16.0, 3.0, 26.0, 14.0)), (1, Rect::new(90.0, 7.0, 14.0, 14.0)), (2, Rect::new(186.0, 7.0, 14.0, 14.0))]);
        assert_eq!(l.key_hints(1)[..2], [(0, Rect::new(16.0, 7.0, 14.0, 14.0)), (1, Rect::new(90.0, 3.0, 26.0, 14.0))], "the keycaps follow the buttons");
        let window = Rect::new(0.0, 0.0, l.w, l.h);
        for sel in 0..3 { assert!(l.key_hints(sel).iter().all(|(_, b)| window.contains(b.x, b.y) && window.contains(b.right(), b.bottom())), "nothing is clipped") }
        assert_eq!(l.text_x(), 23.0);
        let g = Geom { frame: window, scale: 1.0, cells: l.cells.clone(), solid: l.solid(0), thumbs: vec![], update: None };
        let hit = |x: f64, y: f64| g.hit(x, y, false);
        assert_eq!(hit(20.0, 5.0), Some(Hit::Bubble), "a key hint's overhang applies nothing");
        assert_eq!(hit(74.0, 5.0), None, "where a badge used to overhang: the app's");
        assert_eq!(hit(76.0, 40.0), Some(Hit::Pick(0)), "the mark's corner is the button's");
        assert_eq!(hit(82.0, 30.0), None, "between two buttons: the app's");
    }
    #[test] fn a_fourth_option_has_no_digit_hint() {
        // นา | lets | done | cool: only options 1-3 have a digit; the fourth shows its Tab once highlighted
        let l = layout(&[20.0, 30.0, 35.0, 30.0], &[TH, None, None, None], 1392.0, BubbleLayout::Horizontal, false);
        assert_eq!(l.key_hints(0).iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2]);
        let on_fourth = l.key_hints(3);
        assert_eq!(on_fourth.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [0, 1, 2, 3]);
        assert_eq!(on_fourth[3].1.w, KEY_TAB_W, "the highlighted fourth gets the Tab keycap");
    }
    #[test] fn a_short_label_keeps_its_key_hint_clear_of_its_mark() {
        // "ok": the button widens so the Tab keycap on its top-left corner and the mark in its bottom-right one never
        // share a column, highlighted or not
        let l = layout(&[14.0], &[TYPO], 1392.0, BubbleLayout::Horizontal, false);
        for sel in [0, 1] {
            let (k, b) = (l.key_hints(sel)[0].1, l.button(0, sel));
            assert!(k.x + KEY_TAB_W <= b.right() - OPT_BORDER - MARK_INSET - MARK_D.max(MARK_FLAG_W), "Tab keycap {k:?} clear of the widest mark in {b:?}");
        }
    }
    #[test] fn accent_text_stays_readable() {
        assert!(!dark_text_on(ACCENT_FALLBACK), "white on blue");
        assert!(dark_text_on((1.0, 0.8, 0.0)), "dark on yellow");
        assert!(!dark_text_on((0.55, 0.55, 0.55)), "white on graphite");
    }
    #[test] fn rows_keep_a_badge_column_when_any_row_has_a_badge() {
        let l = layout(&[100.0, 180.0, 50.0], &[TH, TYPO, None], 1400.0, BubbleLayout::Vertical, false);
        // CHROME_W 92 + the badge column (16 + 8) + the widest label, in the 10 px shadow margin; each badge centred
        // before the key hint
        assert_eq!((l.w, l.h, l.label_w[0]), (316.0, 124.0, 180.0));
        assert_eq!(l.badges(), vec![(Reason::Layout(InputLang::Thai), Rect::new(215.0, 24.0, 16.0, 16.0)), (Reason::Typo, Rect::new(215.0, 54.0, 16.0, 16.0))]);
        assert!(l.key_hints(0).is_empty(), "rows keep their key hints inline");
        assert_eq!(plain(&[100.0, 180.0, 50.0], 1400.0, BubbleLayout::Vertical).w, 292.0, "no badges, no column");
    }
    #[test] fn the_download_chip_rides_the_strip_but_not_the_rows() {
        // one gap after the last button, a slot of its own, centred on the rail; the buttons stay put
        let l = layout(&[40.2, 61.0], &vec![None; 2], 600.0, BubbleLayout::Horizontal, true);
        let chip = l.update.expect("the strip carries the chip");
        assert_eq!(l.cells, plain(&[40.2, 61.0], 600.0, BubbleLayout::Horizontal).cells);
        assert_eq!(chip, Rect::new(l.cells[1].right() + OPT_GAP, SHADOW_M + OPT_SEL_H - UPDATE_CHIP_D, UPDATE_CHIP_D, UPDATE_CHIP_D));
        assert_eq!(l.w, chip.right() + SHADOW_M);
        // the chip takes the click, and a click there is no engine key: the capture side raises the tray item's request
        let g = Geom { frame: Rect::new(0.0, 0.0, l.w, l.h), scale: 1.0, cells: l.cells.clone(), solid: l.solid(0), thumbs: vec![], update: l.update };
        assert_eq!(g.hit(chip.x + 1.0, chip.y + 1.0, false), Some(Hit::Update));
        assert_eq!(Hit::Update.key(), None, "the chip is not an option: the engine hears nothing");
        // a strip that fits bare does not fit with the chip: rows, and rows carry no chip
        let bare = plain(&[61.0; 4], 1392.0, BubbleLayout::Horizontal);
        assert_eq!(bare.kind, BubbleLayout::Horizontal);
        let l = layout(&[61.0; 4], &vec![None; 4], bare.w, BubbleLayout::Horizontal, true);
        assert_eq!((l.kind, l.update), (BubbleLayout::Vertical, None));
        assert_eq!(layout(&[61.0], &vec![None; 1], 600.0, BubbleLayout::Vertical, true).update, None, "wanted rows, no chip");
    }
}
