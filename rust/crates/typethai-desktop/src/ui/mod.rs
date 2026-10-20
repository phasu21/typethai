//! gpui-kit suggestion bubble + tray, running on the main thread's gpui event loop.
//!
//! The bubble is one persistent gpui window of kind `PopUp` (macOS: a non-activating NSPanel at popup level that joins
//! all Spaces; Windows: a topmost tool window to which we add WS_EX_NOACTIVATE), created with `focus: false, show: false`.
//! It is positioned/shown/hidden natively (see `native_*`) at the rect computed by `placement::place`, so placement uses
//! one global coordinate space on both OSes. Clicks never reach it: the capture thread hit-tests mouse-downs against the
//! shared `bridge::GEOM` and swallows them, so the app being typed in keeps focus.
//!
//! The input-language badge is a second such window, shown for a moment over the caret when the input source changes
//! (`badge`). On macOS it ignores the mouse; on Windows the mouse hook swallows a click on it and takes it down.
use crate::badge::{self, Cmd};
use crate::engine::{Hold, InputLang};
use crate::bridge::{self, BubbleLayout, Geom, Layout, Opt, UiMsg, FONT_PX, PAD, ROW_H, ROW_PX};
use crate::engine::Reason;
use crate::update;
use crate::placement::{self, Anchor, Monitor, Opts, Placement, Rect};
use gpui_kit::component::{button::Button, theme::Theme, ActiveTheme, Icon, Selectable as _, Sizable as _};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

mod settings;
pub use settings::{demo_local, LocalDemo};
#[cfg(target_os = "macos")] pub mod native_mac;
#[cfg(target_os = "macos")] pub use native_mac as native;
#[cfg(windows)] pub mod native_win;
#[cfg(windows)] pub use native_win as native;

pub struct BubbleView {
    pub items: Vec<Opt>, pub layout: Layout,
    /// Highlighted row (Tab applies it); 0 on every new bubble.
    pub selected: usize,
    /// The arrow keys are in the bubble: Enter applies the highlighted option too, and its keycap says so.
    pub focused: bool,
    /// Tab held over the bubble: the accent sweeps across the highlighted option's Tab key as it fills.
    pub hold: Option<Hold>,
    /// The panel's bounds as actually laid out by gpui in the last frame (logical px, window-relative).
    pub panel: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Each option's laid-out bounds in the last frame (a row, or a strip button) — for the click hit-test.
    pub row_bounds: std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// Rendered (laid-out) text box of each label, for the `--screenshot` self-check (measured vs rendered).
    pub text_bounds: std::rc::Rc<std::cell::RefCell<Vec<Option<Bounds<Pixels>>>>>,
    /// `--probe-caret`: draw a solid red marker instead of the rows.
    pub marker: bool,
    /// The download chip rides at the strip's end while the updater is not sure everything is current (`update::chip`).
    pub update: bool,
    /// The chip's hover tooltip, opened with the bubble (its text is fresh at every show; only set at creation).
    tip: Option<TooltipWin>,
}

/// The bundled typeThai Sans; the system Thai fonts only if it failed to load.
pub(crate) fn thai_font() -> Font {
    let mut f = font("typeThai Sans");
    f.fallbacks = Some(FontFallbacks::from_fonts(["Thonburi", "Sukhumvit Set", "Ayuthaya", "Leelawadee UI", "Leelawadee", "Tahoma"].map(String::from).to_vec()));
    f
}

impl Render for BubbleView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The key tap owns the shift state (`bridge::SHIFT_HELD`); the run loop repaints on a change of it (20 ms), so
        // no window modifier listener is needed here. A listener registered per render also feeds gpui key/modifier
        // dispatch for a non-activating popup, whose dispatch has no active node: `key_dispatch::active_node_id()`
        // unwraps None and the app aborts (seen at launch, in `did_finish_launching`).
        if self.marker { return div().size_full().bg(rgb(0xff2d2d)).into_any_element() }
        let t = cx.theme(); let l = &self.layout; let selected = self.selected;
        // the strip's pill is too small for "Enter"
        let (apply_key, apply_pill) = if self.focused { ("Enter", "↵") } else { ("Tab", "Tab") };
        let hold = self.hold.map(|h| h.progress(Instant::now()));
        let shift = bridge::SHIFT_HELD.load(std::sync::atomic::Ordering::SeqCst) || window.modifiers().shift;
        let dark = t.is_dark();
        let hover = t.list_hover;
        // selected row is a little stronger in dark mode
        let sel_bg = if dark { t.foreground.opacity(0.22) } else { t.accent };
        // fixed-size keycaps: the platform fonts differ in width, and the hint slot is reserved in the layout
        let (cap_fg, cap_bg) = (t.muted_foreground, t.tokens.muted);
        let cap = move |text: String, w: f64| div().w(px(w as f32)).h(px(bridge::KEYCAP_H as f32)).flex_none().flex().items_center().justify_center()
            .rounded(px(4.)).text_color(cap_fg).text_size(px(bridge::KEYCAP_FONT_PX as f32)).child(text);
        // a thumb (`Layout::thumbs`): a grey keycap like the digits with Lucide's outline thumb; on hover the thumb is
        // drawn filled (`icons/`, the same shape with a fill), in the colour of what it does: the accent for up, the
        // danger red for down. Both icons take the keycap's text colour; the group hover shows one or the other
        let a = native::accent().unwrap_or(bridge::ACCENT_FALLBACK);
        let key_bg = Hsla::from(Rgba { r: a.0, g: a.1, b: a.2, a: 1.0 });
        let key_fg = if bridge::dark_text_on(a) { Hsla::from(rgb(0x1a1a1a)) } else { Hsla::from(rgb(0xffffff)) };
        let danger_fg = t.danger;
        let thumb_key = move |i: usize, up: bool, w: f64, h: f64, radius: f64| {
            let id = 2 * i + usize::from(up);
            let group = SharedString::from(format!("thumb-{id}"));
            let (outline, filled) = if up { (gpui_kit::assets::IconName::ThumbsUp, "icons/thumbs-up-fill.svg") } else { (gpui_kit::assets::IconName::ThumbsDown, "icons/thumbs-down-fill.svg") };
            let size = px(bridge::THUMB_ICON as f32);
            div().id(("thumb", id)).group(group.clone())
                .w(px(w as f32)).h(px(h as f32)).flex_none().relative().flex().items_center().justify_center().rounded(px(radius as f32))
                .bg(cap_bg).text_color(cap_fg)
                .hover(move |s| s.text_color(if up { key_bg } else { danger_fg }))
                .child(div().flex().group_hover(group.clone(), |s| s.opacity(0.)).child(Icon::new(outline).with_size(size)))
                .child(div().absolute().size_full().flex().items_center().justify_center().opacity(0.).group_hover(group, |s| s.opacity(1.))
                    .child(Icon::empty().path(filled).with_size(size)))
        };
        // why an option is offered: the flag of the language a conversion's text becomes, or the typo icon in the danger
        // red. A row shows it round like gpui-kit's Badge on the row's fill, ringed in the border colour (`Badge::icon`
        // tints its icon one colour, so the flag needs this); a strip button carries it faint in its bottom-right corner,
        // behind the label.
        let (ring, danger) = (t.input, t.danger);
        let (row_bg, row_sel): (Background, Background) = (t.popover.into(), sel_bg.into());
        let reason_icon = move |r: Reason, icon_px: f64| match r {
            Reason::Layout(lang) => img(bridge::flag(lang)).size_full().rounded_full().into_any_element(),
            Reason::Typo => Icon::new(gpui_kit::assets::IconName::SpellCheck).with_size(px(icon_px as f32)).text_color(danger).into_any_element(),
        };
        let badge = move |r: Reason, fill: Background| div().size(px(bridge::BADGE_D as f32)).flex_none().rounded_full().border_1().border_color(ring)
            .bg(fill).flex().items_center().justify_center().child(reason_icon(r, bridge::BADGE_ICON));
        // the flag at 2:1 (`wide_flag`), in a box that clips so nothing past it shows
        let mark = move |r: Reason| {
            let (w, h) = match r { Reason::Layout(_) => (bridge::MARK_FLAG_W, bridge::MARK_FLAG_H), Reason::Typo => (bridge::MARK_D, bridge::MARK_D) };
            div().absolute().right(px(bridge::MARK_INSET as f32)).bottom(px(bridge::MARK_INSET as f32)).w(px(w as f32)).h(px(h as f32))
                .overflow_hidden().rounded(px(2.)).opacity(bridge::MARK_ALPHA)
                .child(match r {
                    Reason::Layout(lang) => img(bridge::wide_flag(lang)).w(px(w as f32)).h(px(h as f32)).flex_none().into_any_element(),
                    Reason::Typo => reason_icon(r, bridge::MARK_D),
                })
        };
        let slot = self.panel.clone(); let rows_slot = self.row_bounds.clone();
        { let mut tb = self.text_bounds.borrow_mut(); tb.clear(); tb.resize(self.items.len(), None); }
        // a fixed label column; the inner text box shrinks to the rendered text so the self-check can measure it
        // a label laid out on one line never wraps: the line wrapper counts Thai marks (ั ิ ์) as if they took room, so
        // a word that fits shaped (ประวัติศาสตร์) would break before its last cluster
        let column = |i: usize, o: &Opt| { let tb = self.text_bounds.clone(); let w = px(l.label_width(i, selected) as f32);
            let one_line = l.kind == BubbleLayout::Horizontal || l.cells.get(i).is_some_and(|c| c.h <= ROW_H);
            let font = if l.kind == BubbleLayout::Horizontal && i == selected { bridge::SEL_FONT_PX } else { FONT_PX };
            div().w(w).flex_none().flex().when(l.kind == BubbleLayout::Horizontal, |d| d.justify_center()).text_size(px(font as f32))
                .on_children_prepainted(move |b, _, _| { if let Some(s) = tb.borrow_mut().get_mut(i) { *s = b.first().copied(); } })
                .child(if one_line { div().whitespace_nowrap() } else { div().max_w(w).whitespace_normal() }.child(o.label.clone())) };
        // The window is borderless and transparent: the rows' panel with its badges, or the strip's buttons, and the key
        // hints are all that is drawn. Their laid-out bounds are recorded every frame and `Ui::reconcile` resizes /
        // re-places the window and refreshes the click cells.
        let (surface, edge) = (t.popover, t.border);
        let root = div().size_full().relative().flex().items_start().justify_start()
            .font(thai_font()).text_size(px(FONT_PX as f32)).text_color(t.popover_foreground)
            .on_children_prepainted(move |b, _, _| slot.set(b.first().copied()));
        match l.kind {
            // No panel: each button stands on a solid backing with a shadow of its own, so it reads over any app. One
            // gpui-kit Button per option, as wide as its fixed slot: the highlighted one fills it, larger, the others
            // shorter, dimmed and on its floor, each with its reason mark inside. Its key hint is a pill on its top-left
            // corner: Tab (Enter while the arrow keys are in the bubble) on the highlighted option, the option's number on
            // the others.
            BubbleLayout::Horizontal => {
                let radius = t.radius;
                let buttons = div().flex_none().flex().flex_row().items_end().gap(px(bridge::OPT_GAP as f32))
                    .on_children_prepainted(move |b, _, _| *rows_slot.borrow_mut() = b)
                    .children(self.items.iter().enumerate().map(|(i, o)| {
                        let (slot, b, on) = (l.cells[i], l.button(i, selected), i == selected);
                        let label = column(i, o).mt(px(bridge::LABEL_DROP as f32));
                        // the button's own padding would clip the mark: the padding is on this box, sized to the inside of
                        // the button's border (`size_full` lets the label's line box stretch it past the bottom edge)
                        let content = div().w(px((b.w - 2.0 * bridge::OPT_BORDER) as f32)).h(px((b.h - 2.0 * bridge::OPT_BORDER) as f32)).flex_none()
                            .relative().flex().items_center().justify_center()
                            .px(px((if on { bridge::OPT_SEL_PX } else { bridge::OPT_PX }) as f32))
                            .when_some(o.reason, |d, r| d.child(mark(r))).child(label);
                        div().w(px(slot.w as f32)).h(px(slot.h as f32)).flex_none().flex().items_end()
                            .child(div().flex_none().rounded(radius).bg(surface).shadow_md()
                                .child(Button::new(("opt", i)).selected(on).w(px(b.w as f32)).h(px(b.h as f32)).px(px(0.))
                                    .when(!on, |b| b.opacity(bridge::DIM)).child(content)))
                    }));
                // the key hints come after all the buttons, so a neighbour never paints over one; solid in the user's
                // accent colour (`key_bg`), so the key reads over the button's border and over any app
                // the others grey (the theme's muted keycap colours over the surface, still solid). Shift held: a rated
                // option shows its thumbs there instead
                let rated: Vec<bool> = self.items.iter().map(|o| o.rated).collect();
                let keys = l.key_hints(selected).into_iter().filter(|&(i, _)| !(shift && rated[i])).map(|(i, at)| { let on = i == selected;
                    let key = div().absolute().left(px(at.x as f32)).top(px(at.y as f32)).w(px(at.w as f32)).h(px(at.h as f32)).rounded(px(bridge::KEY_RADIUS as f32)).bg(surface);
                    match hold.filter(|_| on) {
                        // Tab held: grey, the accent sweeping in as the hold fills
                        Some(p) => key.child(sweep(at.w, at.h, bridge::KEY_RADIUS, apply_pill, bridge::KEY_FONT_PX, (*cap_bg, cap_fg), (key_bg, key_fg), p)),
                        None => key.child(div().size_full().flex().items_center().justify_center().rounded(px(bridge::KEY_RADIUS as f32))
                            .bg(if on { key_bg } else { *cap_bg }).text_color(if on { key_fg } else { cap_fg }).text_size(px(bridge::KEY_FONT_PX as f32))
                            .child(if on { apply_pill.to_string() } else { (i + 1).to_string() })),
                    } });
                let thumbs = if shift { l.thumbs(&l.cells, selected, &rated) } else { vec![] };
                let thumbs = thumbs.into_iter().map(|(i, up, at)| div().absolute().left(px(at.x as f32)).top(px(at.y as f32))
                    .rounded(px(bridge::KEY_RADIUS as f32)).bg(surface).child(thumb_key(i, up, at.w, at.h, bridge::KEY_RADIUS)));
                // the download chip at the rail's end, on its own small backing like a keycap, its icon in the user's
                // accent: no hint, no label, nothing stateful — a click is the tray's update item (see `Hit::Update`),
                // and hovering it names the release below the chip (the tip panel ignores the mouse)
                let tip = self.tip.clone();
                let chip = l.update.map(|c| div().id("chip").absolute().left(px(c.x as f32)).top(px(c.y as f32))
                    .w(px(bridge::UPDATE_CHIP_D as f32)).h(px(bridge::UPDATE_CHIP_D as f32)).flex_none()
                    .rounded(px(6.)).bg(surface).shadow_md().flex().items_center().justify_center()
                    .on_hover(move |hovered, _, cx| {
                        let Some(tip) = &tip else { return };
                        if let (true, Some(g)) = (*hovered, bridge::geom()) {
                            let chip = Rect::new(g.frame.x + c.x * g.scale, g.frame.y + c.y * g.scale, c.w * g.scale, c.h * g.scale);
                            tip.show(chip, cx);
                        } else { tip.hide(cx) }
                    })
                    .child(Icon::empty().path("icons/download.svg").with_size(px(bridge::UPDATE_CHIP_ICON as f32)).text_color(key_bg)));
                root.pl(px(l.margin as f32)).pt(px(l.margin as f32)).child(buttons).when_some(chip, |d, c| d.child(c))
                    .children(keys).children(thumbs).into_any_element()
            }
            BubbleLayout::Vertical => {
                let panel = div().flex_none().p(px(PAD as f32)).flex().w(px((l.w - 2.0 * l.margin) as f32)).flex_col()
                    .on_children_prepainted(move |b, _, _| *rows_slot.borrow_mut() = b)
                    .bg(surface).border_1().border_color(edge).rounded(px(bridge::RADIUS as f32)).shadow_md()
                    .children(self.items.iter().enumerate().map(|(i, o)| {
                        div().id(i).min_h(px(ROW_H as f32)).flex().flex_row().items_center().justify_between().px(px(ROW_PX as f32)).py(px(5.)).rounded(px(5.))
                            .when(i == selected, |d| d.bg(sel_bg))
                            .when(i != selected, |d| d.hover(move |s| s.bg(hover)))
                            // full text, wrapped within the label column (no ellipsis). The keycap slot stays out of this column.
                            .child(column(i, o))
                            // a row has no corner to hang a badge on: a column before the keys, when any row has one
                            .when(l.reasons.iter().any(Option::is_some), |d| d.child(div().w(px(bridge::BADGE_D as f32)).flex_none().when_some(o.reason, |d, r| d.child(badge(r, if i == selected { row_sel } else { row_bg })))))
                            // the keys that apply this row: Tab for the highlighted one, Tab + its number for the others
                            // (a row past `DIGIT_ROWS` has no number: nothing but its Tab when highlighted). Shift held: a
                            // rated row shows its thumbs there instead (`Layout::thumbs`)
                            .child(div().w(px(bridge::HINT_SLOT_W as f32)).flex_none().flex().flex_row().justify_end().gap(px(bridge::KEYCAP_GAP as f32))
                                .when(shift && o.rated, |d| d.gap(px(bridge::THUMB_GAP as f32))
                                    .children([true, false].map(|up| thumb_key(i, up, bridge::DIGIT_CAP_W, bridge::KEYCAP_H, 4.0))))
                                .when(!(shift && o.rated) && (i == selected || i < bridge::DIGIT_ROWS), |d| d.child(match hold.filter(|_| i == selected) {
                                    Some(p) => { let fill = (key_bg, key_fg);
                                        sweep(bridge::TAB_CAP_W, bridge::KEYCAP_H, 4.0, apply_key, bridge::KEYCAP_FONT_PX, (*cap_bg, cap_fg), fill, p).flex_none() }
                                    None => cap(if i == selected { apply_key } else { "Tab" }.into(), bridge::TAB_CAP_W).bg(cap_bg),
                                }))
                                .when(!(shift && o.rated) && i != selected && i < bridge::DIGIT_ROWS, |d| d.child(cap((i + 1).to_string(), bridge::DIGIT_CAP_W).bg(cap_bg))))
                    }));
                root.p(px(l.margin as f32)).child(panel).into_any_element()
            }
        }
    }
}

/// A keycap that fills with the accent from the left as a held Tab nears opening the ⌃⌥R rows (`p` 0..1). The label
/// is drawn twice, in each part's colour, so it reads wherever the edge is.
fn sweep(w: f64, h: f64, radius: f64, label: &'static str, font_px: f64, (bg, fg): (Hsla, Hsla), (fill, fill_fg): (Hsla, Hsla), p: f32) -> Div {
    let face = move |c: Hsla| div().absolute().left_0().top_0().w(px(w as f32)).h(px(h as f32)).flex().items_center().justify_center().text_color(c).text_size(px(font_px as f32)).child(label);
    div().relative().w(px(w as f32)).h(px(h as f32)).rounded(px(radius as f32)).bg(bg).overflow_hidden().child(face(fg))
        .child(div().absolute().left_0().top_0().h_full().w(px((w * p.clamp(0.0, 1.0) as f64) as f32)).rounded_l(px(radius as f32)).overflow_hidden().bg(fill).child(face(fill_fg)))
}
/// Measure each label with the real text system (same font + size as the view, incl. Thai fallbacks).
fn measure(window: &Window, items: &[Opt]) -> Vec<f64> {
    let f = thai_font();
    items.iter().map(|o| {
        let run = TextRun { len: o.label.len(), font: f.clone(), color: black(), background_color: None, underline: None, strikethrough: None };
        window.text_system().shape_line(o.label.clone().into(), px(FONT_PX as f32), &[run], None).width().as_f32() as f64
            + bridge::MEASURE_SLACK
    }).collect()
}
// the typo badge's icon, the Settings updates and apps pages' and the hold indicator's, which the component icons lack
gpui_kit::assets::icon_assets!(BadgeIcons, [SpellCheck, Download, Sparkles, AppWindow]);
/// The component icons, the typo badge's icon and the flags on conversion badges (flag-icons, MIT: `flags/LICENSE`).
struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let ours: Option<&'static [u8]> = match path {
            "flags/th.svg" => Some(include_bytes!("../../flags/th.svg")), "flags/us.svg" => Some(include_bytes!("../../flags/us.svg")),
            "flags/th-wide.svg" => Some(include_bytes!("../../flags/th-wide.svg")), "flags/us-wide.svg" => Some(include_bytes!("../../flags/us-wide.svg")),
            "icons/thumbs-up-fill.svg" => Some(include_bytes!("../../icons/thumbs-up-fill.svg")), "icons/thumbs-down-fill.svg" => Some(include_bytes!("../../icons/thumbs-down-fill.svg")),
            _ => None };
        if let Some(b) = ours { return Ok(Some(std::borrow::Cow::Borrowed(b))) }
        match BadgeIcons.load(path)? { Some(b) => Ok(Some(b)), None => assets::Assets.load(path) }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = assets::Assets.list(path)?; all.extend(BadgeIcons.list(path)?); all.sort(); all.dedup(); Ok(all)
    }
}

/// `cells`: the option boxes last published to `bridge::GEOM`.
struct Ui { last: Option<Rect>, native: native::Native, handle: AnyWindowHandle, view: Entity<BubbleView>, visible: bool, anchor: Option<Anchor>, size: (f64, f64), cells: Vec<Rect>, shift: bool }
impl Ui {
    /// Measure labels -> layout (a strip, or rows when the strip does not fit) -> place -> show. The exact row heights
    /// (and widths, if fonts differ) come from the real layout via `reconcile` on the next frames. Returns the initial placement.
    fn show(&mut self, items: Vec<Opt>, wanted: BubbleLayout, anchor: Anchor, cx: &mut App) -> Option<Placement> {
        // A read that found no caret must not throw the bubble to the field's edge or the mouse: the caret it was
        // last seen at is still the text being typed at.
        let anchor = placement::keep_caret(self.anchor.as_ref(), anchor);
        let view = self.view.clone(); let (visible, real) = (self.visible, self.size);
        let update_chip = self.view.read(cx).update;
        let res = self.handle.update(cx, |_, window, cx| {
            // up to the visible width of the monitor the bubble will be on (minus margins): rows wrap, a strip gives way to rows
            let mons = native::monitors();
            let max_w = anchor.caret.map(|c| (c.x, c.y)).or(anchor.mouse).and_then(|(x, y)| placement::monitor_for(&mons, x, y)).map(|i| bridge::max_width_for(mons[i].visible.w)).unwrap_or(600.0);
            let l = bridge::layout(&measure(window, &items), &items.iter().map(|o| o.reason).collect::<Vec<_>>(), max_w, wanted, update_chip);
            // rows only know their height once laid out: the same layout as the bubble on screen (typing on, rows
            // refreshed in place) keeps its real size, so the window is not resized to the guess and back
            let size = if visible && l == view.read(cx).layout { real } else { (l.w, l.h) };
            view.update(cx, |v, cx| { v.items = items; v.layout = l; v.selected = 0; v.focused = false; v.panel.set(None); v.row_bounds.borrow_mut().clear(); cx.notify(); });
            Some(size)
        }).ok().flatten()?;
        self.anchor = Some(anchor); self.size = (0.0, 0.0);
        let p = self.place(res, cx);
        if p.is_some() { self.visible = true; }
        p
    }
    /// Each option's box: as gpui laid it out in the last frame, or the layout's guess until it has.
    fn laid_cells(&self, cx: &App) -> Vec<Rect> {
        let v = self.view.read(cx); let laid = v.row_bounds.borrow();
        if laid.len() != v.layout.cells.len() { return v.layout.cells.clone() }
        laid.iter().map(|b| Rect::new(b.origin.x.as_f32() as f64, b.origin.y.as_f32() as f64, b.size.width.as_f32() as f64, b.size.height.as_f32() as f64)).collect()
    }
    /// What takes clicks besides the options: the rows' panel as laid out, the badges and the key hints.
    fn laid_solid(&self, cx: &App) -> Vec<Rect> {
        let v = self.view.read(cx); let mut solid = v.layout.solid(v.selected);
        if let (BubbleLayout::Vertical, Some(b)) = (v.layout.kind, v.panel.get()) {
            solid[0] = Rect::new(b.origin.x.as_f32() as f64, b.origin.y.as_f32() as f64, b.size.width.as_f32() as f64, b.size.height.as_f32() as f64);
        }
        solid
    }
    /// The thumbs on the rated options (`Layout::thumbs`), on the options as laid out.
    fn laid_thumbs(&self, cx: &App) -> Vec<(usize, bool, Rect)> {
        let v = self.view.read(cx); let rated: Vec<bool> = v.items.iter().map(|o| o.rated).collect();
        v.layout.thumbs(&self.cells, v.selected, &rated)
    }
    fn place(&mut self, size: (f64, f64), cx: &mut App) -> Option<Placement> {
        let anchor = self.anchor?;
        let mons = native::monitors();
        let l = &self.view.read(cx).layout;
        // the strip sits over the caret line, rows under it; either takes the other side when it does not fit
        let lower_fraction = if l.kind == BubbleLayout::Horizontal { 0.0 } else { Opts::default().lower_fraction };
        let opts = Opts { caret_dx: -l.text_x(), inset: l.margin, lower_fraction, ..Opts::default() };
        let p = placement::place(&anchor, size, &mons, &opts)?;
        // run after the current gpui update returns, so gpui isn't borrowed and receives the resulting resize
        let n = self.native.clone(); let r = p.rect;
        // unchanged rect (e.g. a caret-follow that didn't move): don't touch the window -> no flicker; moved: the tip
        // would hang off its old spot, so it goes until the pointer enters the chip again
        if self.last != Some(r) || !self.visible { self.last = Some(r); cx.foreground_executor().spawn(async move { native::show(&n, r) }).detach(); if let Some(tip) = self.view.read(cx).tip.clone() { tip.hide(cx) } }
        self.cells = self.laid_cells(cx);
        bridge::set_geom(Some(Geom { frame: p.rect, scale: mons[p.monitor].size_scale, cells: self.cells.clone(), solid: self.laid_solid(cx), thumbs: self.laid_thumbs(cx), update: self.view.read(cx).layout.update }));
        self.size = size;
        Some(p)
    }
    /// Called every UI tick: if gpui's laid-out panel size differs from the size the window was placed with, re-run
    /// placement with the real size (so the flip/clamp uses it) and resize the window to it. Options that moved inside
    /// the same size (the strip's Tab keycap following the highlight) only refresh the click cells.
    fn reconcile(&mut self, cx: &mut App) -> Option<Placement> {
        if !self.visible { return None }
        let b = self.view.read(cx).panel.get()?;
        // the window holds what is drawn and the transparent margin around it (the strip's download chip rides after
        // the buttons, so the drawn part reaches one `OPT_GAP + UPDATE_CHIP_D` further right)
        let v = self.view.read(cx);
        let chip = if v.layout.update.is_some() { bridge::OPT_GAP + bridge::UPDATE_CHIP_D } else { 0.0 };
        let m = v.layout.margin;
        let real = ((b.origin.x + b.size.width).as_f32().ceil() as f64 + chip + m, (b.origin.y + b.size.height).as_f32().ceil() as f64 + m);
        if (real.0 - self.size.0).abs() < 0.5 && (real.1 - self.size.1).abs() < 0.5 && self.laid_cells(cx) == self.cells { return None }
        self.place(real, cx)
    }
    /// Caret moved while shown: same size, new anchor.
    fn reanchor(&mut self, anchor: Anchor, cx: &mut App) -> Option<Placement> {
        if !self.visible || self.size.0 <= 0.0 { return None }
        // A re-read that found no caret must not move the bubble: the fallbacks (the field's edge, the mouse) are
        // nowhere near the text, so a read that timed out would throw the bubble off the caret.
        let anchor = placement::keep_caret(self.anchor.as_ref(), anchor);
        self.anchor = Some(anchor); let sz = self.size; self.place(sz, cx)
    }
    fn hide(&mut self, cx: &mut App) {
        bridge::set_geom(None); self.anchor = None; self.last = None;
        if let Some(tip) = self.view.read(cx).tip.clone() { tip.hide(cx) }
        if self.visible { let n = self.native.clone(); cx.foreground_executor().spawn(async move { native::hide(&n) }).detach(); self.visible = false; }
    }
}

/// A hidden, transparent PopUp that never takes focus, positioned and shown natively (`native::show`): the bubble, the badge.
fn open_popup<V: Render>(w: f32, h: f32, cx: &mut App, build: impl FnOnce(&mut App) -> Entity<V>) -> (AnyWindowHandle, Entity<V>, native::Native) {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(w), px(h))))),
        titlebar: None, focus: false, show: false, kind: WindowKind::PopUp,
        is_movable: false, is_resizable: false, is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let (handle, view) = open_window(opts, cx, |_, cx| build(cx)).expect("popup window");
    // gpui-kit's Root fills the window with the theme background; only what the view draws may show
    handle.update(cx, |root, _, cx| if let Ok(root) = root.downcast::<gpui_kit::base::Root>() {
        root.update(cx, |r, _| r.style().background = Some(transparent_black().into()))
    }).expect("popup window");
    let n = handle.update(cx, |_, window, _| {
        let n = native::native(window).expect("native popup window");
        native::prepare(&n);
        // follow the system light/dark appearance
        window.observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx)).detach();
        n
    }).expect("popup window");
    (handle, view, n)
}
fn open_bubble(cx: &mut App) -> Ui {
    let tip = TooltipWin::open(cx);
    let (handle, view, n) = open_popup(300., 80., cx, |cx| cx.new(|_| BubbleView { items: vec![], selected: 0, focused: false, hold: None, layout: bridge::layout(&[], &[], 600.0, BubbleLayout::Vertical, false), panel: Default::default(), row_bounds: Default::default(), text_bounds: Default::default(), marker: false, update: false, tip: Some(tip) }));
    Ui { last: None, native: n, handle, view, visible: false, anchor: None, size: (0.0, 0.0), cells: vec![], shift: false }
}

/// The input-language badge: a solid pill with a shadow, the language's round flag (as on a conversion's badge)
/// and its name, sized to the label. Or, while Tab is held with no bubble up, the hold indicator: a sparkle, "Rewrite"
/// and a thin bar filling in the accent colour.
pub struct BadgeView { lang: InputLang, label_w: f64, hold: Option<Hold> }
impl Render for BadgeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme(); let f = |v: f64| px(v as f32);
        let pill_w = badge::size(self.label_w).0 - 2.0 * badge::MARGIN;
        let pill = div().flex_none().relative().w(f(pill_w)).h(f(badge::PILL_H)).px(f(badge::PILL_PX)).flex().flex_row().items_center().gap(f(badge::GAP))
            .bg(t.popover).border_1().border_color(t.border).rounded_full().shadow_md()
            .font(thai_font()).font_weight(FontWeight::MEDIUM).text_size(f(badge::LABEL_PX)).text_color(t.popover_foreground);
        let pill = match self.hold {
            Some(h) => {
                let a = native::accent().unwrap_or(bridge::ACCENT_FALLBACK); let accent = Hsla::from(Rgba { r: a.0, g: a.1, b: a.2, a: 1.0 });
                let bar_w = pill_w - 2.0 * (bridge::BORDER + badge::PILL_PX);
                pill.child(div().size(f(badge::FLAG)).flex_none().flex().items_center().justify_center()
                        .child(Icon::new(gpui_kit::assets::IconName::Sparkles).with_size(f(badge::FLAG - 2.0)).text_color(accent)))
                    .child(div().w(f(self.label_w)).flex_none().child(badge::HOLD_LABEL))
                    .child(div().absolute().left(f(badge::PILL_PX)).bottom(f(badge::BAR_BOTTOM)).w(f(bar_w)).h(f(badge::BAR_H)).rounded_full().bg(t.foreground.opacity(0.12))
                        .child(div().h_full().w(f(bar_w * h.progress(Instant::now()) as f64)).rounded_full().bg(accent)))
            }
            None => pill.child(div().size(f(badge::FLAG)).flex_none().rounded_full().border_1().border_color(t.input).child(img(bridge::flag(self.lang)).size_full().rounded_full()))
                .child(div().w(f(self.label_w)).flex_none().child(badge::label(self.lang))),
        };
        div().size_full().flex().items_start().justify_start().p(f(badge::MARGIN)).child(pill)
    }
}
/// Width of a badge label as drawn.
fn label_w(window: &Window, text: &'static str) -> f64 {
    let mut f = thai_font(); f.weight = FontWeight::MEDIUM;
    let run = TextRun { len: text.len(), font: f, color: black(), background_color: None, underline: None, strikethrough: None };
    window.text_system().shape_line(text.into(), px(badge::LABEL_PX as f32), &[run], None).width().as_f32().ceil() as f64 + 1.0
}
struct BadgeWin { handle: AnyWindowHandle, view: Entity<BadgeView>, native: native::Native, shown: badge::Shown, at: Option<Placement>, last: Option<Anchor> }
impl BadgeWin {
    fn open(cx: &mut App) -> BadgeWin {
        let (handle, view, native) = open_popup(80., 46., cx, |cx| cx.new(|_| BadgeView { lang: InputLang::Latin, label_w: 0.0, hold: None }));
        native::click_through(&native);
        BadgeWin { handle, view, native, shown: badge::Shown::default(), at: None, last: None }
    }
    fn changed(&mut self, lang: InputLang, anchor: Anchor, cx: &mut App) { let c = self.shown.changed(lang, Instant::now()); self.run(c, Some(anchor), cx) }
    /// Focus landed in a text field nothing has been typed in yet: show and stay.
    fn pinned(&mut self, lang: InputLang, anchor: Anchor, cx: &mut App) { let c = self.shown.pinned(lang); self.run(c, Some(anchor), cx) }
    fn unpin(&mut self, cx: &mut App) { let c = self.shown.unpin(); self.run(c, None, cx) }
    fn bubble(&mut self, visible: bool, cx: &mut App) { let c = self.shown.bubble(visible); self.run(c, None, cx) }
    fn clicked(&mut self, cx: &mut App) { let c = self.shown.clicked(); self.run(c, None, cx) }
    /// The caret moved under a badge that is up: follow it. Nothing is done while no badge is on screen (the offer may
    /// have been refused, or a keystroke took it down since), and a spot that has not changed is left alone.
    fn move_to(&mut self, anchor: Anchor, cx: &mut App) {
        let Some(cur) = self.at else { return };
        // A re-read that found no caret must not move the badge off the caret it is on.
        let anchor = placement::keep_caret(self.last.as_ref(), anchor);
        self.last = Some(anchor);
        let w = self.view.read(cx).label_w;
        let Some(p) = badge::place(&anchor, w, &native::monitors()) else { return };
        if p.rect == cur.rect { return }
        let n = self.native.clone();
        cx.foreground_executor().spawn(async move { native::show(&n, p.rect) }).detach();
        *bridge::BADGE_AT.lock().unwrap() = Some(p.rect); self.at = Some(p);
    }
    fn tick(&mut self, cx: &mut App) {
        let c = self.shown.tick(Instant::now()); self.run(c, None, cx);
        // the hold indicator fills on the UI's own ticks
        if self.view.read(cx).hold.is_some() && self.at.is_some() { self.view.update(cx, |_, cx| cx.notify()) }
    }
    /// Tab held with no bubble up (None: let go).
    fn hold(&mut self, hold: Option<Hold>, anchor: Option<Anchor>, cx: &mut App) {
        if hold.is_some() { self.view.update(cx, |v, _| v.hold = hold); }
        let c = self.shown.hold(hold.is_some()); self.run(c, anchor, cx)
    }
    fn run(&mut self, cmd: Option<Cmd>, anchor: Option<Anchor>, cx: &mut App) {
        let n = self.native.clone();
        if let Some(a) = anchor { self.last = Some(a); }
        let at = match cmd {
            Some(c @ (Cmd::Show(_) | Cmd::Hold)) => {
                let text = match c { Cmd::Show(lang) => badge::label(lang), _ => badge::HOLD_LABEL };
                let w = self.handle.update(cx, |_, window, _| label_w(window, text)).unwrap_or(24.0);
                self.view.update(cx, |v, cx| { if let Cmd::Show(lang) = c { v.lang = lang; v.hold = None; } v.label_w = w; cx.notify(); });
                let p = anchor.and_then(|a| badge::place(&a, w, &native::monitors()));
                if p.is_none() { self.shown.clicked(); }
                p
            }
            Some(Cmd::Hide) => None,
            None => return,
        };
        // as the bubble: outside gpui's borrow of the window, so it gets the resize
        match at { Some(p) => cx.foreground_executor().spawn(async move { native::show(&n, p.rect) }).detach(), None => if self.at.is_some() { cx.foreground_executor().spawn(async move { native::hide(&n) }).detach() } }
        *bridge::BADGE_AT.lock().unwrap() = at.map(|p| p.rect); self.at = at;
    }
}

/// The download chip's hover tooltip: one short line — most often "Update to version x.y.z", from the updater's status
/// so it names what the click is about to reach — in a small panel past the chip. It ignores the mouse everywhere
/// (macOS natively; on Windows the hook swallows a click on it, `bridge::TIP_AT`), so it never takes focus or eats a
/// keypress anywhere else.
struct TooltipWin { handle: AnyWindowHandle, view: Entity<TipView>, native: native::Native }
#[derive(Clone)] struct TipView { text: SharedString }
const TIP_TEXT_PX: f64 = 13.0; const TIP_PX: f64 = 8.0; const TIP_PY: f64 = 5.0; const TIP_GAP: f64 = 3.0;
impl Render for TipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div().flex_none().px(px(TIP_PX as f32)).py(px(TIP_PY as f32)).rounded(px(6.)).bg(t.popover).border_1()
            .border_color(t.border).shadow_md().font(thai_font()).text_size(px(TIP_TEXT_PX as f32))
            .text_color(t.popover_foreground).whitespace_nowrap().child(self.text.clone())
    }
}
impl Clone for TooltipWin {
    fn clone(&self) -> Self { TooltipWin { handle: self.handle, view: self.view.clone(), native: self.native.clone() } }
}
impl TooltipWin {
    fn open(cx: &mut App) -> TooltipWin {
        let (handle, view, native) = open_popup(10., 10., cx, |cx| cx.new(|_| TipView { text: SharedString::default() }));
        native::click_through(&native);
        TooltipWin { handle, view, native }
    }
    /// Under `chip` (global units), right edge on the chip's; above it when the screen ends too soon for the tip.
    fn show(&self, chip: Rect, cx: &mut App) { self.show_text(chip, &tip_text(), cx) }
    /// The same placement for an exact text (`--demo-bubble`'s screenshot needs one: the demo's updater never asks GitHub).
    fn show_text(&self, chip: Rect, text: &str, cx: &mut App) {
        let (w, th) = self.handle.update(cx, |_, window, _| tip_box(window, text)).unwrap_or((140.0, TIP_TEXT_PX));
        self.view.update(cx, |v, cx| { v.text = text.into(); cx.notify(); });
        let h = th + 2.0 * TIP_PY + bridge::BORDER * 2.0;
        let mon = placement::monitor_for(&native::monitors(), chip.x + chip.w / 2.0, chip.y + chip.h / 2.0).unwrap_or(0);
        let vis = native::monitors()[mon].visible;
        let r = Rect::new((chip.right() - w).max(vis.x + 8.0), chip.bottom() + TIP_GAP, w, h);
        let r = if r.bottom() > vis.bottom() - 8.0 { Rect::new(r.x, chip.y - TIP_GAP - h, r.w, r.h) } else { r };
        let n = self.native.clone();
        cx.foreground_executor().spawn(async move { native::show(&n, r) }).detach();
        self.at(Some(r));
    }
    fn hide(&self, cx: &mut App) {
        self.at(None);
        let n = self.native.clone();
        cx.foreground_executor().spawn(async move { native::hide(&n) }).detach();
    }
    fn at(&self, r: Option<Rect>) { *bridge::TIP_AT.lock().unwrap() = r; }
}
/// The chip's tip text: the release it is on its way to (downloading, staged, or waiting to be opened by hand); before
/// a version is known, just the promise.
fn tip_text() -> String {
    let v = match update::status() {
        update::Status::Downloading(v) => Some(v),
        update::Status::Ready { version, .. } | update::Status::Available { version, .. } => Some(version),
        _ => None,
    };
    match v {
        Some(v) => format!("Update to version {}", v.trim_start_matches('v')),
        None => "Update typeThai".into(),
    }
}
/// The tip's text as drawn: its width, and its full line box (ascent + descent; Thai marks grow past the px size), so
/// the window is exactly the tip.
fn tip_box(window: &Window, text: &str) -> (f64, f64) {
    let run = TextRun { len: text.len(), font: thai_font(), color: black(), background_color: None, underline: None, strikethrough: None };
    let s = window.text_system().shape_line(text.into(), px(TIP_TEXT_PX as f32), &[run], None);
    (s.width.as_f32().ceil() as f64 + 2.0 * TIP_PX + 2.0 * bridge::BORDER, (s.ascent + s.descent).as_f32().ceil() as f64)
}
/// The tip's text in the bubble demo: with the version after the built one (`--screenshot` exercises `show_text`, which
/// takes the text verbatim, without asking the updater, which never talks to GitHub in the demo).
fn tip_demo_text() -> String {
    let p: Vec<u32> = update::CURRENT.split('.').filter_map(|s| s.parse().ok()).collect();
    let next = if p.len() == 3 { format!("{}.{}.{}", p[0], p[1], p[2] + 1) } else { update::CURRENT.into() };
    format!("Update to version {next}")
}

fn app() -> Application { application().with_assets(AppAssets).with_quit_mode(QuitMode::Explicit) }
/// typeThai Sans: Noto Sans Thai with Noto Sans's Latin letters (SIL OFL 1.1, `fonts/OFL.txt`,
/// `scripts/fonts/build-typethai-sans.py`), bundled so text looks and measures the same on every platform.
const TYPETHAI_SANS: [&[u8]; 4] = [
    include_bytes!("../../fonts/typeThaiSans-Regular.ttf"), include_bytes!("../../fonts/typeThaiSans-Medium.ttf"),
    include_bytes!("../../fonts/typeThaiSans-SemiBold.ttf"), include_bytes!("../../fonts/typeThaiSans-Bold.ttf"),
];
fn setup(cx: &mut App) {
    if let Err(e) = cx.text_system().add_fonts(TYPETHAI_SANS.iter().map(|b| std::borrow::Cow::Borrowed(*b)).collect()) {
        eprintln!("typeThai: typeThai Sans not loaded ({e}); falling back to the system fonts");
    }
    init(cx);
    #[cfg(target_os = "macos")] cx.set_activation_policy(ActivationPolicy::Accessory);
    Theme::sync_system_appearance(None, cx);
}

/// Run the UI until the capture thread reports `Stopped` (or Quit is chosen and it doesn't answer within 2 s).
pub fn run(rx: Receiver<UiMsg>) {
    app().run(move |cx| {
        setup(cx);
        let mut ui = open_bubble(cx);
        let mut badge = BadgeWin::open(cx);
        let tray = super::platform::tray::Tray::new().map_err(|e| eprintln!("tray: {e}")).ok();
        #[cfg(windows)] if tray.is_some() { super::platform::tray::promote_on_taskbar() }
        cx.spawn(async move |cx| {
            let mut quit_at: Option<Instant> = None;
            let mut settings_win: Option<settings::SettingsWin> = None;
            let mut update_gen = 0;
            let mut front_app: Option<bridge::FrontApp> = None;
            loop {
                let mut stop = false;
                cx.update(|cx| {
                    while let Ok(m) = rx.try_recv() { match m {
                        UiMsg::Show { items, layout, anchor } => { badge.bubble(true, cx); ui.show(items, layout, anchor, cx); }
                        UiMsg::Move(a) => { ui.reanchor(a, cx); }
                        UiMsg::Hide => { ui.hide(cx); badge.bubble(false, cx) }
                        UiMsg::Badge { lang, anchor, pin } => if pin { badge.pinned(lang, anchor, cx) } else { badge.changed(lang, anchor, cx) },
                        UiMsg::UnpinBadge => badge.unpin(cx),
                        UiMsg::MoveBadge(anchor) => badge.move_to(anchor, cx),
                        UiMsg::HideBadge => badge.clicked(cx),
                        // over the bubble: on its Tab key; otherwise the pill in the badge window
                        UiMsg::Hold { hold, anchor } => { let over = hold.is_some() && ui.visible;
                            ui.view.update(cx, |v, cx| { v.hold = hold.filter(|_| over); cx.notify(); });
                            badge.hold(hold.filter(|_| !over), anchor, cx) }
                        // the strip's highlighted button grows: its key hint and thumbs move with it
                        UiMsg::Select(i) => { ui.view.update(cx, |v, cx| { v.selected = i; cx.notify(); });
                            if let Some(mut g) = bridge::geom() { g.solid = ui.laid_solid(cx); g.thumbs = ui.laid_thumbs(cx); bridge::set_geom(Some(g)) } }
                        UiMsg::Focus(f) => ui.view.update(cx, |v, cx| { v.focused = f; cx.notify(); }),
                        UiMsg::Paused(p) => { if p { badge.clicked(cx) } if let Some(t) = &tray { t.set_paused(p) } },
                        UiMsg::Stopped => stop = true,
                        UiMsg::Settings(snap) => { if !snap.input_badge { badge.unpin(cx) } settings::apply_snapshot(&mut settings_win, snap, cx) },
                        UiMsg::Permission(needed) => if let Some(t) = &tray { t.set_permission(needed) },
                    } }
                    ui.reconcile(cx); badge.tick(cx);
                    let sh = bridge::SHIFT_HELD.load(std::sync::atomic::Ordering::SeqCst);
                    if sh != ui.shift { ui.shift = sh; ui.view.update(cx, |_, cx| cx.notify()); }
                    if ui.visible && ui.view.read(cx).hold.is_some() { ui.view.update(cx, |_, cx| cx.notify()) }
                    if bridge::take(&bridge::REQ_OPEN_SETTINGS) { settings::open_or_focus(&mut settings_win, cx) }
                    let tray_cmd = tray.as_ref().and_then(|t| t.poll());
                    let tray_cmd = if bridge::take(&bridge::REQ_UPDATE_CLICK) { Some(super::platform::tray::TrayCmd::CheckUpdate) } else { tray_cmd };
                    match tray_cmd {
        Some(super::platform::tray::TrayCmd::TogglePause) => bridge::REQ_TOGGLE_PAUSE.store(true, std::sync::atomic::Ordering::SeqCst),
        Some(super::platform::tray::TrayCmd::CheckUpdate) => match update::click() {
            update::Click::Check => update::check_now(),
            update::Click::Open(page) => update::open_url(&page),
            update::Click::Restart(staged) => match update::restart_into(&staged) {
                // quit as on Quit (state saved); the helper waits for this process to exit
                Ok(()) => { bridge::REQ_QUIT.store(true, std::sync::atomic::Ordering::SeqCst); quit_at.get_or_insert(Instant::now() + Duration::from_secs(2)); }
                Err(e) => { eprintln!("typeThai: cannot install the update: {e}"); update::open_url(update::RELEASES_PAGE) }
            },
        },
        Some(super::platform::tray::TrayCmd::Settings) => settings::open_or_focus(&mut settings_win, cx),
        Some(super::platform::tray::TrayCmd::Permission) => super::platform::request_permission(),
        Some(super::platform::tray::TrayCmd::ToggleApp) => if let Some(f) = bridge::front_app() {
            if f.off { bridge::push_settings(crate::prefs::SettingsCmd::AppOn(f.id)) }
            else { bridge::push_settings(crate::prefs::SettingsCmd::AppOff { id: f.id, name: f.name }) }
        },
        Some(super::platform::tray::TrayCmd::Quit) => { bridge::REQ_QUIT.store(true, std::sync::atomic::Ordering::SeqCst); quit_at.get_or_insert(Instant::now() + Duration::from_secs(2)); }
        None => {}
    }
                    let g = update::generation();
                    if g != update_gen { update_gen = g; if let Some(t) = &tray { t.set_update(&update::status()) } settings::refresh(&settings_win, cx);
                        let chip = update::chip(); let tip = ui.view.read(cx).tip.clone();
                        ui.view.update(cx, |v, _| v.update = chip);
                        if !chip { if let Some(tip) = tip { tip.hide(cx) } }
                    }
                    let front = bridge::front_app();
                    if front != front_app { if let Some(t) = &tray { t.set_front_app(front.as_ref()) } front_app = front; }
                    #[cfg(target_os = "macos")] super::platform::macos::sync_input_source();
                    if stop || quit_at.is_some_and(|t| Instant::now() > t) { cx.quit(); }
                });
                if stop { break }
                cx.background_executor().timer(Duration::from_millis(20)).await;
            }
        }).detach();
    });
}

/// Sample items for `--demo-bubble`.
/// `--probe-caret --secs N`: every 500 ms log the frontmost app, the anchor source and the caret rect, and show a
/// small red marker at the caret (or a thin bar under the field / at the mouse for the fallbacks). No key capture.
#[cfg(target_os = "macos")]
pub fn probe(secs: f64, lookup: fn() -> (Anchor, String), trusted: bool) {
    eprintln!("probe-caret: {secs}s, Accessibility trusted for this process: {trusted}");
    app().run(move |cx| {
        setup(cx);
        let ui = open_bubble(cx);
        ui.view.update(cx, |v, _| v.marker = true);
        cx.spawn(async move |cx| {
            let end = Instant::now() + Duration::from_secs_f64(secs);
            while Instant::now() < end {
                let (a, info) = lookup();
                let src = placement::source_of(&a, &Opts::default());
                eprintln!("probe {:.1}s {info}", secs - end.saturating_duration_since(Instant::now()).as_secs_f64());
                let r = match src {
                    Some(placement::Source::Caret) => a.caret.map(|c| Rect::new(c.x - 1.0, c.y, 3.0, c.h.max(4.0))),
                    Some(placement::Source::Field) => a.field.map(|f| Rect::new(f.x, f.bottom(), f.w.min(200.0), 3.0)),
                    Some(placement::Source::Mouse) => a.mouse.map(|(x, y)| Rect::new(x + 8.0, y + 8.0, 8.0, 8.0)),
                    None => None,
                };
                let n = ui.native.clone();
                cx.update(|cx| match r { Some(r) => cx.foreground_executor().spawn(async move { native::show(&n, r) }).detach(), None => native::hide(&n) });
                cx.background_executor().timer(Duration::from_millis(500)).await;
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

pub fn demo_items() -> Vec<Opt> {
    use crate::engine::InputLang;
    vec![Opt { label: "สวัสดีครับ".into(), reason: Some(Reason::Layout(InputLang::Thai)), rated: true }, Opt { label: "l;ylfu".into(), reason: Some(Reason::Layout(InputLang::Latin)), rated: false },
        Opt { label: "ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋".into(), reason: None, rated: false }]
}

/// `--demo-bubble`: show the bubble with sample (or `items`) rows anchored at each point (global units; a fake 18-unit
/// caret, every 4th point uses the mouse fallback), `secs` seconds each, then quit. No points: centre, bottom (flip) and
/// right edge of every monitor. Several `items` sets are shown one after another at each point, updated in place as
/// while typing. With `shot`, shows once (first point or main-monitor centre), lets it lay out and paint,
/// prints native vs. laid-out geometry and a per-option self-check, captures the bubble's own window to `shot` (PNG), and quits.
/// `backdrop` (Some(true): light) opens a window of sample text behind the first point, to see the bubble over an app.
pub fn demo(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, sets: Vec<Vec<Opt>>, select: usize, focused: bool, hold: Option<f32>, wanted: BubbleLayout, light: bool, backdrop: Option<bool>, update: bool) {
    let sets = if sets.is_empty() { vec![demo_items()] } else { sets };
    app().run(move |cx| {
        setup(cx);
        if light { Theme::change(gpui_kit::component::ThemeMode::Light, None, cx); }
        let mut ui = open_bubble(cx);
        let mut pts = points.clone();
        if let (Some(bright), Some(&(x, y))) = (backdrop, pts.first()) { open_backdrop(x, y, bright, cx) }
        if pts.is_empty() {
            for m in native::monitors() { let v = m.visible; pts.extend([(v.x + v.w / 2.0, v.y + v.h / 2.0), (v.x + v.w * 0.3, v.bottom() - 40.0), (v.right() - 10.0, v.y + 60.0)]); }
        }
        if shot.is_some() { pts.truncate(1); }
        let shot = shot.clone();
        cx.spawn(async move |cx| {
            for (i, (x, y)) in pts.iter().copied().enumerate() { for items in &sets {
                let anchor = if i % 4 == 3 { Anchor { caret: None, field: None, mouse: Some((x, y)) } }
                    else { Anchor { caret: Some(Rect::new(x, y, 2.0, 18.0)), field: Some(Rect::new(x - 40.0, y - 6.0, 360.0, 30.0)), mouse: None } };
                let kind = if anchor.caret.is_some() { "caret" } else { "mouse" };
                let pl = cx.update(|cx| { ui.view.update(cx, |v, _| v.update = update); let p = ui.show(items.clone(), wanted, anchor, cx); ui.view.update(cx, |v, cx| { v.selected = select; v.focused = focused; v.hold = hold.map(held_at); cx.notify(); }); p });
                eprintln!("demo {i}: anchor ({x},{y}) {kind} -> {:?}", pl);
                // let gpui lay out, then fit the window to the real panel (a couple of frames)
                for _ in 0..10 {
                    cx.background_executor().timer(Duration::from_millis(30)).await;
                    if let Some(p) = cx.update(|cx| ui.reconcile(cx)) { eprintln!("demo {i}: refit to laid-out size -> {:?}", p); }
                }
                if let Some(path) = &shot {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let h = ui.handle; let v = ui.view.clone();
                    let info = cx.update(|cx| { let lay = v.read(cx).layout.clone(); let panel = v.read(cx).panel.get();
                        let vp = h.update(cx, |_, w, _| w.viewport_size()).ok();
                        format!("layout {:?}\npanel(laid out) {:?}\nviewport {:?}\nclick cells {:?}\n{}", lay, panel, vp, bridge::geom().map(|g| g.cells), native::describe(&ui.native)) });
                    eprintln!("{info}");
                    let chk = cx.update(|cx| { let w = h.update(cx, |_, w, _| measure(w, items)).unwrap_or_default(); self_check(v.read(cx), &w) });
                    eprintln!("{chk}");
                    match native::capture(&ui.native, path) { Ok(()) => eprintln!("screenshot -> {path}"), Err(e) => eprintln!("screenshot failed: {e}") }
                    // with the chip, also run its tip the way a hover does (`show_text`: the demo's updater never
                    // asks GitHub, so say the version after the built one) and capture the tip's own window
                    if update {
                        let tipchip = cx.update(|cx| { let tip = v.read(cx).tip.clone();
                            let rect = |g: bridge::Geom, c: placement::Rect| Rect::new(g.frame.x + c.x * g.scale, g.frame.y + c.y * g.scale, c.w * g.scale, c.h * g.scale);
                            match (tip, bridge::geom()) { (Some(tip), Some(mut g)) if g.update.is_some() => { let c = g.update.take().unwrap(); Some((tip, rect(g, c))) }, _ => None }
                        });
                        if let Some((tip, chip)) = tipchip {
                            cx.update(|cx| tip.show_text(chip, &tip_demo_text(), cx));
                            cx.background_executor().timer(Duration::from_millis(200)).await;
                            let tp = path.replace(".png", "-tip.png");
                            match native::capture(&tip.native, &tp) { Ok(()) => eprintln!("screenshot -> {tp}"), Err(e) => eprintln!("screenshot failed: {e}") }
                        }
                    }
                } else {
                    cx.background_executor().timer(Duration::from_secs_f64(secs)).await;
                }
            } }
            cx.update(|cx| { ui.hide(cx); cx.quit(); });
        }).detach();
    });
}

/// `--demo-badge`: the input-language badge under a fake 18-unit caret at each point (no points: the main monitor's
/// centre), as on a switch: up 1.2 s, then down; `secs` per point, languages taking turns from `lang`. `pin`: show it
/// as the text-field preview instead (it stays for the whole `secs`, however long). With `shot`, shows it once, captures
/// its window to `shot` (PNG) and quits. `backdrop` as in `demo`.
pub fn demo_badge(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, lang: InputLang, pin: bool, light: bool, backdrop: Option<bool>) {
    app().run(move |cx| {
        setup(cx);
        if light { Theme::change(gpui_kit::component::ThemeMode::Light, None, cx); }
        let mut b = BadgeWin::open(cx);
        let mut pts = points.clone();
        if pts.is_empty() { if let Some(m) = native::monitors().first() { let v = m.visible; pts.push((v.x + v.w / 2.0, v.y + v.h / 2.0)) } }
        if let (Some(bright), Some(&(x, y))) = (backdrop, pts.first()) { open_backdrop(x, y, bright, cx) }
        if shot.is_some() { pts.truncate(1); }
        let shot = shot.clone();
        cx.spawn(async move |cx| {
            // let a backdrop come up first
            cx.background_executor().timer(Duration::from_millis(300)).await;
            for (i, (x, y)) in pts.iter().copied().enumerate() {
                let l = if i % 2 == 0 { lang } else if lang == InputLang::Thai { InputLang::Latin } else { InputLang::Thai };
                let anchor = Anchor { caret: Some(Rect::new(x, y, 2.0, 18.0)), field: Some(Rect::new(x - 40.0, y - 6.0, 360.0, 30.0)), mouse: None };
                cx.update(|cx| if pin { b.pinned(l, anchor, cx) } else { b.changed(l, anchor, cx) });
                eprintln!("demo-badge {i}: {l:?} caret ({x},{y}) -> {:?}", b.at);
                if let Some(path) = &shot {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    eprintln!("{}", native::describe(&b.native));
                    match native::capture(&b.native, path) { Ok(()) => eprintln!("screenshot -> {path}"), Err(e) => eprintln!("screenshot failed: {e}") }
                    break
                }
                let end = Instant::now() + Duration::from_secs_f64(secs);
                while Instant::now() < end {
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                    cx.update(|cx| { let was = b.shown.up(); b.tick(cx); if was && !b.shown.up() { eprintln!("demo-badge {i}: hidden") } });
                }
                cx.update(|_| eprintln!("demo-badge {i}: still up after {secs}s: {}", b.shown.up()));
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

/// A hold that stays at `p` of the way for the length of a demo (it moves a thousandth a second).
fn held_at(p: f32) -> Hold {
    let (now, d) = (Instant::now(), Duration::from_secs(1000));
    Hold { from: now.checked_sub(d.mul_f32(p.clamp(0.0, 1.0))).unwrap_or(now), full: now + d.mul_f32(1.0 - p.clamp(0.0, 1.0)) }
}
/// `--demo-hold`: the hold-Tab indicator with no bubble up, under a fake 18-unit caret at each point (no points: the main
/// monitor's centre), filling from a tap's length to `TAB_HOLD` as a real hold does and then gone, `secs` per point; or
/// stopped at `progress`. With `shot`, shows it once, captures its window to `shot` (PNG) and quits.
pub fn demo_hold(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, progress: Option<f32>, light: bool, backdrop: Option<bool>) {
    use crate::engine::{TAB_CHORD_MIN, TAB_HOLD};
    app().run(move |cx| {
        setup(cx);
        if light { Theme::change(gpui_kit::component::ThemeMode::Light, None, cx); }
        let mut b = BadgeWin::open(cx);
        let mut pts = points.clone();
        if pts.is_empty() { if let Some(m) = native::monitors().first() { let v = m.visible; pts.push((v.x + v.w / 2.0, v.y + v.h / 2.0)) } }
        if let (Some(bright), Some(&(x, y))) = (backdrop, pts.first()) { open_backdrop(x, y, bright, cx) }
        if shot.is_some() { pts.truncate(1); }
        let shot = shot.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(300)).await;
            for (i, (x, y)) in pts.iter().copied().enumerate() {
                let anchor = Anchor { caret: Some(Rect::new(x, y, 2.0, 18.0)), field: Some(Rect::new(x - 40.0, y - 6.0, 360.0, 30.0)), mouse: None };
                let now = Instant::now();
                let h = progress.map(held_at).unwrap_or(Hold { from: now, full: now + (TAB_HOLD - TAB_CHORD_MIN) });
                cx.update(|cx| b.hold(Some(h), Some(anchor), cx));
                eprintln!("demo-hold {i}: caret ({x},{y}) -> {:?}", b.at);
                if let Some(path) = &shot {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    eprintln!("{}", native::describe(&b.native));
                    match native::capture(&b.native, path) { Ok(()) => eprintln!("screenshot -> {path}"), Err(e) => eprintln!("screenshot failed: {e}") }
                    break
                }
                let end = Instant::now() + Duration::from_secs_f64(secs);
                while Instant::now() < end {
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                    // full: the rows would open now, and the indicator goes
                    cx.update(|cx| { b.tick(cx); if b.shown.up() && Instant::now() >= h.full { b.hold(None, None, cx); eprintln!("demo-hold {i}: full, hidden") } });
                }
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

/// A plain window of text around (x, y), under the bubble: an app the bubble floats over.
fn open_backdrop(x: f64, y: f64, bright: bool, cx: &mut App) {
    struct Backdrop(bool);
    impl Render for Backdrop {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (bg, fg) = if self.0 { (rgb(0xfafafa), rgb(0x1f2328)) } else { (rgb(0x1e1e1e), rgb(0xd4d4d4)) };
            div().size_full().bg(bg).text_color(fg).font(thai_font()).text_size(px(15.)).p(px(16.)).flex().flex_col().gap(px(6.))
                .children(["The quick brown fox jumps over the lazy dog.", "สวัสดีครับ ยินดีต้อนรับสู่ typeThai", "l;ylfu", "Lorem ipsum dolor sit amet, consectetur adipiscing.", "ภาษาไทยวันละคำ"].map(|t| div().child(t)))
        }
    }
    let opts = WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px((x - 160.0) as f32), px((y - 70.0) as f32)), size(px(640.), px(230.))))),
        titlebar: None, focus: false, show: true, kind: WindowKind::PopUp, ..Default::default() };
    cx.open_window(opts, |_, cx| cx.new(|_| Backdrop(bright))).expect("backdrop window");
}

/// `--screenshot` self-check, one line per option, one per strip key hint and one for the panel: the measured single-line width
/// against the rendered text box (a row must fit or wrap, a strip option must fit on one line, never clip), the insets
/// around the label column, each strip key hint inside the window and on its button alone, and the laid-out panel against
/// the layout the window was sized from.
fn self_check(v: &BubbleView, measured: &[f64]) -> String {
    let l = &v.layout; let tb = v.text_bounds.borrow(); let rb = v.row_bounds.borrow();
    let pnl = v.panel.get(); let f = |p: Pixels| p.as_f32() as f64;
    let strip = l.kind == BubbleLayout::Horizontal;
    let mut lines: Vec<String> = v.items.iter().enumerate().map(|(i, s)| {
        let lw = l.label_width(i, v.selected); let t = tb.get(i).copied().flatten(); let r = rb.get(i).copied();
        let (tw, th) = t.map_or((-1.0, -1.0), |b| (f(b.size.width), f(b.size.height)));
        // a strip label is centred: its text box, not its column, against the slot
        let tlw = if strip { tw } else { lw };
        let (lpad, rpad) = match (t, r) { (Some(t), Some(r)) => (f(t.origin.x) - f(r.origin.x), f(r.origin.x) + f(r.size.width) - (f(t.origin.x) + tlw)), _ => (-1.0, -1.0) };
        // rows: the row padding, then the label, the badge column, the gap and the reserved Tab + number slot. Strip: the
        // text centred in its slot-wide button.
        let cell = l.cells[i];
        let badge_col = if l.reasons.iter().any(Option::is_some) { bridge::BADGE_D + bridge::HINT_GAP } else { 0.0 };
        let expect = if strip { ((cell.w - tw) / 2.0, (cell.w - tw) / 2.0) } else { (ROW_PX, ROW_PX + bridge::HINT_GAP + bridge::HINT_SLOT_W + badge_col) };
        let wraps = measured[i] > l.label_w[i];
        let pad_ok = (lpad - expect.0).abs() < 1.5 && (rpad - expect.1).abs() < 1.5;
        // the strip never wraps, so its slots sit where the layout put them (the click cells before the first frame)
        let at_ok = !strip || r.is_some_and(|r| (f(r.origin.x) - cell.x).abs() < 1.0 && (f(r.origin.y) - cell.y).abs() < 1.0 && (f(r.size.width) - cell.w).abs() < 1.0);
        let ok = tw <= lw + 0.5 && (wraps || th < 1.5 * FONT_PX * 1.6) && !(strip && wraps) && pnl.is_some() && pad_ok && at_ok;
        format!("option {i}: chars {} measured {:.1} (incl slack) label_w {lw:.1} rendered {tw:.1}x{th:.1} pad L {lpad:.1} R {rpad:.1} (expect {:.1}/{:.1}) {}{} {}",
            s.label.chars().count(), measured[i], expect.0, expect.1, if wraps { "WRAPS" } else { "fits" }, if at_ok { "" } else { " MOVED" }, if ok { "OK" } else { "CHECK-FAIL" })
    }).collect();
    if strip { for (i, at) in l.key_hints(v.selected) {
        // inside the window, and on no other button (the laid-out ones)
        let inside = at.x >= 0.0 && at.y >= 0.0 && at.right() <= l.w && at.bottom() <= l.h;
        let covers = rb.iter().map(|b| Rect::new(f(b.origin.x), f(b.origin.y), f(b.size.width), f(b.size.height)))
            .filter(|c| at.x < c.right() && c.x < at.right() && at.y < c.bottom() && c.y < at.bottom()).count();
        let ok = inside && covers == 1;
        lines.push(format!("key hint {i} at {:.1},{:.1}: inside the window {inside}, on {covers} button(s) {}", at.x, at.y, if ok { "OK" } else { "CHECK-FAIL" }));
    } }
    let (py, pw, ph) = pnl.map_or((-1.0, -1.0, -1.0), |b| (f(b.origin.y), f(b.size.width), f(b.size.height)));
    // rows wrap, so only a strip's height is known before layout; the chip rides after the buttons, so the panel is
    // that much narrower than the content
    let chip = if l.update.is_some() { bridge::OPT_GAP + bridge::UPDATE_CHIP_D } else { 0.0 };
    let (w, h) = (l.w - 2.0 * l.margin - chip, l.h - 2.0 * l.margin);
    let ok = (pw - w).abs() < 1.0 && (py - l.margin).abs() < 0.5 && (!strip || (ph - h).abs() < 1.0);
    lines.push(format!("panel {:?}: laid out {pw:.1}x{ph:.1} at y {py:.1}, layout {w:.1}x{h:.1} at y {:.1} {}", l.kind, l.margin, if ok { "OK" } else { "CHECK-FAIL" }));
    lines.join("\n")
}

fn demo_snap() -> crate::prefs::SettingsSnap {
    let learned = [("brb", 3.0, 0.0), ("gg", 2.0, 0.5), ("lmao", 1.5, 0.0)];
    crate::prefs::SettingsSnap {
        never: ["lol", "ok", "omg"].map(String::from).to_vec(),
        learned: learned.map(|(w, kept, conv)| typethai_core::personal::Learned { word: w.into(), kept, conv }).to_vec(),
        completion: crate::engine::CompleteMode::default(),
        complete_idle: std::time::Duration::from_millis(crate::engine::COMPLETE_IDLE_DEFAULT_MS), tab_hold: crate::engine::TAB_HOLD,
        learn_words: true,
        predict: true,
        accept: crate::engine::AcceptKeys::default(), pick_ctrl: false, pick_arrows: true, input_badge: true,
        badge_idle: Some(std::time::Duration::from_millis(crate::engine::BADGE_IDLE_DEFAULT_MS)),
        next_word: true, bubble: bridge::BubbleLayout::default(),
        words_learned: 1250,
        rewrite: crate::rewrite::Config::default(),
        check_updates: true,
        off_apps: vec![("com.apple.Terminal".into(), "Terminal".into()), ("com.microsoft.VSCode".into(), "Visual Studio Code".into())],
    }
}

/// `--demo-settings`: only the Settings window, filled with sample words and the default rewrite config, on `page`. No key
/// capture, prefs or keychain. Optionally types `search` into the sidebar search box and forces light mode. With `shot`,
/// captures the window to a PNG and quits; otherwise stays open `secs` seconds.
pub fn demo_settings(secs: f64, shot: Option<String>, search: Option<String>, page: usize, light: bool, local: Option<settings::LocalDemo>) {
    app().run(move |cx| {
        setup(cx);
        if light { Theme::change(gpui_kit::component::ThemeMode::Light, None, cx); }
        let Some(h) = settings::open_demo(page, demo_snap(), cx, local).map(|win| win.handle) else { cx.quit(); return };
        let shot = shot.clone(); let search = search.clone();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(400)).await;
            if let Some(text) = &search {
                // the search box is the first tab stop: the sidebar comes before the page
                cx.update(|cx| h.update(cx, |_, window, cx| {
                    window.blur(cx);
                    window.focus_next(cx);
                    for c in text.chars() { window.dispatch_keystroke(Keystroke { modifiers: Modifiers::default(), key: c.to_string(), key_char: Some(c.to_string()) }, cx); }
                })).ok();
                cx.background_executor().timer(Duration::from_millis(400)).await;
            }
            match &shot {
                Some(path) => {
                    let n = cx.update(|cx| h.update(cx, |_, window, _| native::native(window)).ok().flatten());
                    match n.map(|n| native::capture(&n, path)) { Some(Ok(())) => eprintln!("screenshot -> {path}"), Some(Err(e)) => eprintln!("screenshot failed: {e}"), None => eprintln!("screenshot failed: no native window") }
                }
                None => cx.background_executor().timer(Duration::from_secs_f64(secs)).await,
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
}

#[allow(dead_code)]
fn _assert_monitor(_: Monitor) {}
