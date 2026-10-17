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
#[cfg(target_os = "macos")] pub mod native_mac;
#[cfg(target_os = "macos")] pub use native_mac as native;
#[cfg(windows)] mod native_win;
#[cfg(windows)] use native_win as native;

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
}

/// The bundled typeThai Sans; the system Thai fonts only if it failed to load.
pub(crate) fn thai_font() -> Font {
    let mut f = font("typeThai Sans");
    f.fallbacks = Some(FontFallbacks::from_fonts(["Thonburi", "Sukhumvit Set", "Ayuthaya", "Leelawadee UI", "Leelawadee", "Tahoma"].map(String::from).to_vec()));
    f
}

impl Render for BubbleView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.marker { return div().size_full().bg(rgb(0xff2d2d)).into_any_element() }
        let t = cx.theme(); let l = &self.layout; let selected = self.selected;
        // the strip's pill is too small for "Enter"
        let (apply_key, apply_pill) = if self.focused { ("Enter", "↵") } else { ("Tab", "Tab") };
        let hold = self.hold.map(|h| h.progress(Instant::now()));
        let dark = t.is_dark();
        let hover = t.list_hover;
        // selected row is a little stronger in dark mode
        let sel_bg = if dark { t.foreground.opacity(0.22) } else { t.accent };
        // fixed-size keycaps: the platform fonts differ in width, and the hint slot is reserved in the layout
        let (cap_fg, cap_bg) = (t.muted_foreground, t.tokens.muted);
        let cap = move |text: String, w: f64| div().w(px(w as f32)).h(px(bridge::KEYCAP_H as f32)).flex_none().flex().items_center().justify_center()
            .rounded(px(4.)).text_color(cap_fg).text_size(px(bridge::KEYCAP_FONT_PX as f32)).child(text);
        // why an option is offered, round like gpui-kit's Badge on the option's corner: the flag of the language a
        // conversion's text becomes, or the typo icon in the danger red on the fill it sits on (its button's, or a row's),
        // so it reads as notched into the option. Its ring is the button's border colour. gpui-kit's `Badge::icon` tints
        // its icon one colour and sits inside the corner, so the flag needs this.
        let (ring, danger) = (t.input, t.danger);
        let (btn_bg, btn_sel): (Background, Background) = (t.tokens.button.into(), t.tokens.button_active.into());
        let (row_bg, row_sel): (Background, Background) = (t.popover.into(), sel_bg.into());
        let badge = move |r: Reason, fill: Background| div().size(px(bridge::BADGE_D as f32)).flex_none().rounded_full().border_1().border_color(ring)
            .bg(fill).flex().items_center().justify_center()
            .child(match r {
                Reason::Layout(lang) => img(bridge::flag(lang)).size_full().rounded_full().into_any_element(),
                Reason::Typo => Icon::new(gpui_kit::assets::IconName::SpellCheck).with_size(px(bridge::BADGE_ICON as f32)).text_color(danger).into_any_element(),
            });
        let slot = self.panel.clone(); let rows_slot = self.row_bounds.clone();
        { let mut tb = self.text_bounds.borrow_mut(); tb.clear(); tb.resize(self.items.len(), None); }
        // a fixed label column; the inner text box shrinks to the rendered text so the self-check can measure it
        let column = |i: usize, o: &Opt| { let tb = self.text_bounds.clone(); let w = px(l.label_w[i] as f32);
            div().w(w).flex_none().flex().text_size(px(FONT_PX as f32))
                .on_children_prepainted(move |b, _, _| { if let Some(s) = tb.borrow_mut().get_mut(i) { *s = b.first().copied(); } })
                .child(div().max_w(w).whitespace_normal().child(o.label.clone())) };
        // The window is borderless and transparent: the rows' panel, or the strip's groups, and the badges are all that is
        // drawn. Their laid-out bounds are recorded every frame and `Ui::reconcile` resizes / re-places the window and
        // refreshes the click cells.
        let (surface, edge) = (t.popover, t.border);
        let root = div().size_full().relative().flex().items_start().justify_start()
            .font(thai_font()).text_size(px(FONT_PX as f32)).text_color(t.popover_foreground)
            .on_children_prepainted(move |b, _, _| slot.set(b.first().copied()));
        match l.kind {
            // No panel: a group is a solid box with a shadow around its buttons, so it reads over any app. One gpui-kit
            // Button per option, its key hint a pill on its top-left corner: Tab (Enter while the arrow keys are in the
            // bubble) on the highlighted option, the option's number on the others.
            BubbleLayout::Horizontal => {
                { let mut rb = rows_slot.borrow_mut(); rb.clear(); rb.resize(self.items.len(), Bounds::default()); }
                let groups = l.runs.iter().map(|run| { let (start, rb) = (run.start, rows_slot.clone());
                    div().flex().flex_row().items_center().gap(px(bridge::OPT_GAP as f32)).p(px(bridge::GROUP_PAD as f32))
                        .bg(surface).border_1().border_color(edge).rounded(px(bridge::RADIUS as f32)).shadow_md()
                        .on_children_prepainted(move |b, _, _| { let mut rb = rb.borrow_mut(); for (k, b) in b.into_iter().enumerate() { if let Some(s) = rb.get_mut(start + k) { *s = b } } })
                        .children((run.start..run.start + run.len).map(|i| Button::new(("opt", i)).selected(i == selected).child(column(i, &self.items[i]).mt(px(bridge::LABEL_DROP as f32))))) });
                // the badges and key hints come after all the buttons, so a neighbour never paints over one; filled like
                // their button and ringed in its border colour, so they read as notched into it
                let badges = l.badges_of().into_iter().map(|(i, r, at)| div().absolute().left(px(at.x as f32)).top(px(at.y as f32)).child(badge(r, if i == selected { btn_sel } else { btn_bg })));
                // solid in the user's accent colour, so the key reads over the button's border and over any app
                let a = native::accent().unwrap_or(bridge::ACCENT_FALLBACK);
                let key_bg = Hsla::from(Rgba { r: a.0, g: a.1, b: a.2, a: 1.0 });
                let key_fg = if bridge::dark_text_on(a) { Hsla::from(rgb(0x1a1a1a)) } else { Hsla::from(rgb(0xffffff)) };
                // the others grey (the theme's muted keycap colours over the surface, still solid)
                let keys = l.key_hints(selected).into_iter().enumerate().map(|(i, at)| { let on = i == selected;
                    let key = div().absolute().left(px(at.x as f32)).top(px(at.y as f32)).w(px(at.w as f32)).h(px(at.h as f32)).rounded(px(bridge::KEY_RADIUS as f32)).bg(surface);
                    match hold.filter(|_| on) {
                        // Tab held: grey, the accent sweeping in as the hold fills
                        Some(p) => key.child(sweep(at.w, at.h, bridge::KEY_RADIUS, apply_pill, bridge::KEY_FONT_PX, (*cap_bg, cap_fg), (key_bg, key_fg), p)),
                        None => key.child(div().size_full().flex().items_center().justify_center().rounded(px(bridge::KEY_RADIUS as f32))
                            .bg(if on { key_bg } else { *cap_bg }).text_color(if on { key_fg } else { cap_fg }).text_size(px(bridge::KEY_FONT_PX as f32))
                            .child(if on { apply_pill.to_string() } else { (i + 1).to_string() })),
                    } });
                root.pl(px(l.margin as f32)).pt(px(l.margin as f32))
                    .child(div().flex_none().flex().flex_row().items_start().gap(px(bridge::GROUP_GAP_X as f32)).children(groups))
                    .children(badges).children(keys).into_any_element()
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
                            .child(div().w(px(bridge::HINT_SLOT_W as f32)).flex_none().flex().flex_row().justify_end().gap(px(bridge::KEYCAP_GAP as f32))
                                .child(match hold.filter(|_| i == selected) {
                                    Some(p) => { let a = native::accent().unwrap_or(bridge::ACCENT_FALLBACK);
                                        let fill = (Hsla::from(Rgba { r: a.0, g: a.1, b: a.2, a: 1.0 }), if bridge::dark_text_on(a) { Hsla::from(rgb(0x1a1a1a)) } else { Hsla::from(rgb(0xffffff)) });
                                        sweep(bridge::TAB_CAP_W, bridge::KEYCAP_H, 4.0, apply_key, bridge::KEYCAP_FONT_PX, (*cap_bg, cap_fg), fill, p).flex_none() }
                                    None => cap(if i == selected { apply_key } else { "Tab" }.into(), bridge::TAB_CAP_W).bg(cap_bg),
                                })
                                .when(i != selected, |d| d.child(cap((i + 1).to_string(), bridge::DIGIT_CAP_W).bg(cap_bg))))
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
// the typo badge's icon, the Settings updates page's and the hold indicator's, which the component icons lack
gpui_kit::assets::icon_assets!(BadgeIcons, [SpellCheck, Download, Sparkles]);
/// The component icons, the typo badge's icon and the flags on conversion badges (flag-icons, MIT: `flags/LICENSE`).
struct AppAssets;
impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        let flag: Option<&'static [u8]> = match path {
            "flags/th.svg" => Some(include_bytes!("../../flags/th.svg")), "flags/us.svg" => Some(include_bytes!("../../flags/us.svg")), _ => None };
        if let Some(b) = flag { return Ok(Some(std::borrow::Cow::Borrowed(b))) }
        match BadgeIcons.load(path)? { Some(b) => Ok(Some(b)), None => assets::Assets.load(path) }
    }
    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = assets::Assets.list(path)?; all.extend(BadgeIcons.list(path)?); all.sort(); all.dedup(); Ok(all)
    }
}

/// `cells`: the option boxes last published to `bridge::GEOM`.
struct Ui { last: Option<Rect>, native: native::Native, handle: AnyWindowHandle, view: Entity<BubbleView>, visible: bool, anchor: Option<Anchor>, size: (f64, f64), cells: Vec<Rect> }
impl Ui {
    /// Measure labels -> layout (a strip, or rows when the strip does not fit) -> place -> show. The exact row heights
    /// (and widths, if fonts differ) come from the real layout via `reconcile` on the next frames. Returns the initial placement.
    fn show(&mut self, items: Vec<Opt>, wanted: BubbleLayout, anchor: Anchor, cx: &mut App) -> Option<Placement> {
        let view = self.view.clone(); let (visible, real) = (self.visible, self.size);
        let res = self.handle.update(cx, |_, window, cx| {
            // up to the visible width of the monitor the bubble will be on (minus margins): rows wrap, a strip gives way to rows
            let mons = native::monitors();
            let max_w = anchor.caret.map(|c| (c.x, c.y)).or(anchor.mouse).and_then(|(x, y)| placement::monitor_for(&mons, x, y)).map(|i| bridge::max_width_for(mons[i].visible.w)).unwrap_or(600.0);
            let l = bridge::layout(&measure(window, &items), &items.iter().map(|o| o.reason).collect::<Vec<_>>(), max_w, wanted);
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
    /// What takes clicks: the rows' panel as laid out, or the layout's surfaces (a strip's are exact), and the badges.
    fn laid_solid(&self, cx: &App) -> Vec<Rect> {
        let v = self.view.read(cx); let mut solid = v.layout.solid(v.selected);
        if let (BubbleLayout::Vertical, Some(b)) = (v.layout.kind, v.panel.get()) {
            solid[0] = Rect::new(b.origin.x.as_f32() as f64, b.origin.y.as_f32() as f64, b.size.width.as_f32() as f64, b.size.height.as_f32() as f64);
        }
        solid
    }
    fn place(&mut self, size: (f64, f64), cx: &mut App) -> Option<Placement> {
        let anchor = self.anchor?;
        let mons = native::monitors();
        let l = &self.view.read(cx).layout;
        let opts = Opts { caret_dx: -l.text_x(), inset: l.margin, ..Opts::default() };
        let p = placement::place(&anchor, size, &mons, &opts)?;
        // run after the current gpui update returns, so gpui isn't borrowed and receives the resulting resize
        let n = self.native.clone(); let r = p.rect;
        // unchanged rect (e.g. a caret-follow that didn't move): don't touch the window -> no flicker
        if self.last != Some(r) || !self.visible { self.last = Some(r); cx.foreground_executor().spawn(async move { native::show(&n, r) }).detach(); }
        self.cells = self.laid_cells(cx);
        bridge::set_geom(Some(Geom { frame: p.rect, scale: mons[p.monitor].size_scale, cells: self.cells.clone(), solid: self.laid_solid(cx) }));
        self.size = size;
        Some(p)
    }
    /// Called every UI tick: if gpui's laid-out panel size differs from the size the window was placed with, re-run
    /// placement with the real size (so the flip/clamp uses it) and resize the window to it. Options that moved inside
    /// the same size (the strip's Tab keycap following the highlight) only refresh the click cells.
    fn reconcile(&mut self, cx: &mut App) -> Option<Placement> {
        if !self.visible { return None }
        let b = self.view.read(cx).panel.get()?;
        // the window holds what is drawn and the transparent margin around it
        let m = self.view.read(cx).layout.margin;
        let real = ((b.origin.x + b.size.width).as_f32().ceil() as f64 + m, (b.origin.y + b.size.height).as_f32().ceil() as f64 + m);
        if (real.0 - self.size.0).abs() < 0.5 && (real.1 - self.size.1).abs() < 0.5 && self.laid_cells(cx) == self.cells { return None }
        self.place(real, cx)
    }
    /// Caret moved while shown: same size, new anchor.
    fn reanchor(&mut self, anchor: Anchor, cx: &mut App) -> Option<Placement> {
        if !self.visible || self.size.0 <= 0.0 { return None }
        self.anchor = Some(anchor); let sz = self.size; self.place(sz, cx)
    }
    fn hide(&mut self, cx: &mut App) {
        bridge::set_geom(None); self.anchor = None; self.last = None;
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
    let (handle, view, n) = open_popup(300., 80., cx, |cx| cx.new(|_| BubbleView { items: vec![], selected: 0, focused: false, hold: None, layout: bridge::layout(&[], &[], 600.0, BubbleLayout::Vertical), panel: Default::default(), row_bounds: Default::default(), text_bounds: Default::default(), marker: false }));
    Ui { last: None, native: n, handle, view, visible: false, anchor: None, size: (0.0, 0.0), cells: vec![] }
}

/// The input-language badge: a pill like the strip's groups, the language's round flag (as on a conversion's badge)
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
struct BadgeWin { handle: AnyWindowHandle, view: Entity<BadgeView>, native: native::Native, shown: badge::Shown, at: Option<Placement> }
impl BadgeWin {
    fn open(cx: &mut App) -> BadgeWin {
        let (handle, view, native) = open_popup(80., 46., cx, |cx| cx.new(|_| BadgeView { lang: InputLang::Latin, label_w: 0.0, hold: None }));
        native::click_through(&native);
        BadgeWin { handle, view, native, shown: badge::Shown::default(), at: None }
    }
    fn changed(&mut self, lang: InputLang, anchor: Anchor, cx: &mut App) { let c = self.shown.changed(lang, Instant::now()); self.run(c, Some(anchor), cx) }
    fn bubble(&mut self, visible: bool, cx: &mut App) { let c = self.shown.bubble(visible); self.run(c, None, cx) }
    fn clicked(&mut self, cx: &mut App) { let c = self.shown.clicked(); self.run(c, None, cx) }
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
            loop {
                let mut stop = false;
                cx.update(|cx| {
                    while let Ok(m) = rx.try_recv() { match m {
                        UiMsg::Show { items, layout, anchor } => { badge.bubble(true, cx); ui.show(items, layout, anchor, cx); }
                        UiMsg::Move(a) => { ui.reanchor(a, cx); }
                        UiMsg::Hide => { ui.hide(cx); badge.bubble(false, cx) }
                        UiMsg::Badge { lang, anchor } => badge.changed(lang, anchor, cx),
                        UiMsg::HideBadge => badge.clicked(cx),
                        // over the bubble: on its Tab key; otherwise the pill in the badge window
                        UiMsg::Hold { hold, anchor } => { let over = hold.is_some() && ui.visible;
                            ui.view.update(cx, |v, cx| { v.hold = hold.filter(|_| over); cx.notify(); });
                            badge.hold(hold.filter(|_| !over), anchor, cx) }
                        UiMsg::Select(i) => ui.view.update(cx, |v, cx| { v.selected = i; cx.notify(); }),
                        UiMsg::Focus(f) => ui.view.update(cx, |v, cx| { v.focused = f; cx.notify(); }),
                        UiMsg::Paused(p) => if let Some(t) = &tray { t.set_paused(p) },
                        UiMsg::Stopped => stop = true,
                        UiMsg::Settings(snap) => settings::apply_snapshot(&mut settings_win, snap, cx),
                    } }
                    ui.reconcile(cx); badge.tick(cx);
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
        Some(super::platform::tray::TrayCmd::Quit) => { bridge::REQ_QUIT.store(true, std::sync::atomic::Ordering::SeqCst); quit_at.get_or_insert(Instant::now() + Duration::from_secs(2)); }
        None => {}
    }
                    let g = update::generation();
                    if g != update_gen { update_gen = g; if let Some(t) = &tray { t.set_update(&update::status()) } settings::refresh(&settings_win, cx) }
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
    vec![Opt { label: "สวัสดีครับ".into(), reason: Some(Reason::Layout(InputLang::Thai)) }, Opt { label: "l;ylfu".into(), reason: Some(Reason::Layout(InputLang::Latin)) },
        Opt { label: "ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋".into(), reason: None }]
}

/// `--demo-bubble`: show the bubble with sample (or `items`) rows anchored at each point (global units; a fake 18-unit
/// caret, every 4th point uses the mouse fallback), `secs` seconds each, then quit. No points: centre, bottom (flip) and
/// right edge of every monitor. Several `items` sets are shown one after another at each point, updated in place as
/// while typing. With `shot`, shows once (first point or main-monitor centre), lets it lay out and paint,
/// prints native vs. laid-out geometry and a per-option self-check, captures the bubble's own window to `shot` (PNG), and quits.
/// `backdrop` (Some(true): light) opens a window of sample text behind the first point, to see the bubble over an app.
pub fn demo(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, sets: Vec<Vec<Opt>>, select: usize, focused: bool, hold: Option<f32>, wanted: BubbleLayout, light: bool, backdrop: Option<bool>) {
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
                let pl = cx.update(|cx| { let p = ui.show(items.clone(), wanted, anchor, cx); ui.view.update(cx, |v, cx| { v.selected = select; v.focused = focused; v.hold = hold.map(held_at); cx.notify(); }); p });
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
                } else {
                    cx.background_executor().timer(Duration::from_secs_f64(secs)).await;
                }
            } }
            cx.update(|cx| { ui.hide(cx); cx.quit(); });
        }).detach();
    });
}

/// `--demo-badge`: the input-language badge under a fake 18-unit caret at each point (no points: the main monitor's
/// centre), as on a switch: up 1.2 s, then down; `secs` per point, languages taking turns from `lang`. With `shot`, shows
/// it once, captures its window to `shot` (PNG) and quits. `backdrop` as in `demo`.
pub fn demo_badge(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, lang: InputLang, light: bool, backdrop: Option<bool>) {
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
                cx.update(|cx| b.changed(l, anchor, cx));
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

/// `--screenshot` self-check, one line per option, one per strip badge and one for the panel: the measured single-line width
/// against the rendered text box (a row must fit or wrap, a strip option must fit on one line, never clip), the insets
/// around the label column, each strip badge inside the window and on its button alone, and the laid-out panel against
/// the layout the window was sized from.
fn self_check(v: &BubbleView, measured: &[f64]) -> String {
    let l = &v.layout; let tb = v.text_bounds.borrow(); let rb = v.row_bounds.borrow();
    let pnl = v.panel.get(); let f = |p: Pixels| p.as_f32() as f64;
    let strip = l.kind == BubbleLayout::Horizontal;
    let mut lines: Vec<String> = v.items.iter().enumerate().map(|(i, s)| {
        let lw = l.label_w[i]; let t = tb.get(i).copied().flatten(); let r = rb.get(i).copied();
        let (tw, th) = t.map_or((-1.0, -1.0), |b| (f(b.size.width), f(b.size.height)));
        let (lpad, rpad) = match (t, r) { (Some(t), Some(r)) => (f(t.origin.x) - f(r.origin.x), f(r.origin.x) + f(r.size.width) - (f(t.origin.x) + lw)), _ => (-1.0, -1.0) };
        // rows: the row padding, then the label, the badge column, the gap and the reserved Tab + number slot. Strip: the
        // button's border and padding.
        let edge = bridge::OPT_BORDER + bridge::OPT_PX;
        let badge_col = if l.reasons.iter().any(Option::is_some) { bridge::BADGE_D + bridge::HINT_GAP } else { 0.0 };
        let expect = if strip { (edge, edge) } else { (ROW_PX, ROW_PX + bridge::HINT_GAP + bridge::HINT_SLOT_W + badge_col) };
        let wraps = measured[i] > lw;
        let pad_ok = (lpad - expect.0).abs() < 1.5 && (rpad - expect.1).abs() < 1.5;
        // the strip never wraps, so its buttons sit where the layout put them (the click cells before the first frame)
        let cell = l.cells[i];
        let at_ok = !strip || r.is_some_and(|r| (f(r.origin.x) - cell.x).abs() < 1.0 && (f(r.origin.y) - cell.y).abs() < 1.0 && (f(r.size.width) - cell.w).abs() < 1.0);
        let ok = tw <= lw + 0.5 && (wraps || th < 1.5 * FONT_PX * 1.6) && !(strip && wraps) && pnl.is_some() && pad_ok && at_ok;
        format!("option {i}: chars {} measured {:.1} (incl slack) label_w {lw:.1} rendered {tw:.1}x{th:.1} pad L {lpad:.1} R {rpad:.1} (expect {:.1}/{:.1}) {}{} {}",
            s.label.chars().count(), measured[i], expect.0, expect.1, if wraps { "WRAPS" } else { "fits" }, if at_ok { "" } else { " MOVED" }, if ok { "OK" } else { "CHECK-FAIL" })
    }).collect();
    let hints = l.key_hints(v.selected).into_iter().enumerate().map(|(i, at)| (format!("key hint {i}"), at));
    if strip { for (r, at) in l.badges().into_iter().map(|(r, at)| (format!("badge {r:?}"), at)).chain(hints) {
        // inside the window, and on no other button (the laid-out ones)
        let inside = at.x >= 0.0 && at.y >= 0.0 && at.right() <= l.w && at.bottom() <= l.h;
        let covers = rb.iter().map(|b| Rect::new(f(b.origin.x), f(b.origin.y), f(b.size.width), f(b.size.height)))
            .filter(|c| at.x < c.right() && c.x < at.right() && at.y < c.bottom() && c.y < at.bottom()).count();
        let ok = inside && covers == 1;
        lines.push(format!("{r} at {:.1},{:.1}: inside the window {inside}, on {covers} button(s) {}", at.x, at.y, if ok { "OK" } else { "CHECK-FAIL" }));
    } }
    let (py, pw, ph) = pnl.map_or((-1.0, -1.0, -1.0), |b| (f(b.origin.y), f(b.size.width), f(b.size.height)));
    // rows wrap, so only a strip's height is known before layout
    let (w, h) = (l.w - 2.0 * l.margin, l.h - 2.0 * l.margin);
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
        complete_idle: std::time::Duration::from_millis(crate::engine::COMPLETE_IDLE_DEFAULT_MS),
        learn_words: true,
        accept: crate::engine::AcceptKeys::default(), pick_ctrl: false, pick_arrows: false, input_badge: true, bubble: bridge::BubbleLayout::default(),
        words_learned: 1250,
        rewrite: crate::rewrite::Config::default(),
        check_updates: true,
    }
}

/// `--demo-settings`: only the Settings window, filled with sample words and the default rewrite config, on `page`. No key
/// capture, prefs or keychain. Optionally types `search` into the sidebar search box and forces light mode. With `shot`,
/// captures the window to a PNG and quits; otherwise stays open `secs` seconds.
pub fn demo_settings(secs: f64, shot: Option<String>, search: Option<String>, page: usize, light: bool) {
    app().run(move |cx| {
        setup(cx);
        if light { Theme::change(gpui_kit::component::ThemeMode::Light, None, cx); }
        let Some(h) = settings::open_demo(page, demo_snap(), cx).map(|win| win.handle) else { cx.quit(); return };
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
