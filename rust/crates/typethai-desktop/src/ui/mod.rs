//! gpui-kit suggestion bubble + tray, running on the main thread's gpui event loop.
//!
//! The bubble is one persistent gpui window of kind `PopUp` (macOS: a non-activating NSPanel at popup level that joins
//! all Spaces; Windows: a topmost tool window to which we add WS_EX_NOACTIVATE), created with `focus: false, show: false`.
//! It is positioned/shown/hidden natively (see `native_*`) at the rect computed by `placement::place`, so placement uses
//! one global coordinate space on both OSes. Clicks never reach it: the capture thread hit-tests mouse-downs against the
//! shared `bridge::GEOM` and swallows them, so the app being typed in keeps focus.
use crate::bridge::{self, Geom, Layout, UiMsg, FONT_PX, PAD, ROW_H, ROW_PX};
use crate::placement::{self, Anchor, Monitor, Opts, Placement, Rect};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{theme::Theme, ActiveTheme};
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
    pub items: Vec<String>, pub layout: Layout,
    /// Highlighted row (Tab applies it); 0 on every new bubble.
    pub selected: usize,
    /// The panel's bounds as actually laid out by gpui in the last frame (logical px, window-relative).
    pub panel: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    /// Each row's laid-out bounds in the last frame (rows wrap, so heights differ) — for the click hit-test.
    pub row_bounds: std::rc::Rc<std::cell::RefCell<Vec<Bounds<Pixels>>>>,
    /// Rendered (laid-out) text box of each label, for the `--screenshot` self-check (measured vs rendered).
    pub text_bounds: std::rc::Rc<std::cell::RefCell<Vec<Option<Bounds<Pixels>>>>>,
    /// `--probe-caret`: draw a solid red marker instead of the rows.
    pub marker: bool,
}

/// System UI font with explicit Thai fallbacks (macOS: Thonburi / Sukhumvit Set; Windows: Leelawadee UI / Tahoma).
pub(crate) fn thai_font() -> Font {
    let mut f = font(".SystemUIFont");
    f.fallbacks = Some(FontFallbacks::from_fonts(["Thonburi", "Sukhumvit Set", "Ayuthaya", "Leelawadee UI", "Leelawadee", "Tahoma", "Noto Sans Thai"].map(String::from).to_vec()));
    f
}

impl Render for BubbleView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.marker { return div().size_full().bg(rgb(0xff2d2d)).into_any_element() }
        let t = cx.theme(); let l = self.layout; let selected = self.selected;
        let dark = t.is_dark();
        let hover = t.list_hover;
        // selected row is a little stronger in dark mode; its Kbd says Tab applies it
        let sel_bg = if dark { t.foreground.opacity(0.22) } else { t.accent };
        let tab_key = Keystroke::parse("tab").expect("tab");
        let slot = self.panel.clone(); let rows_slot = self.row_bounds.clone();
        { let mut tb = self.text_bounds.borrow_mut(); tb.clear(); tb.resize(self.items.len(), None); }
        // The panel is the only visible surface: the window is borderless + transparent. Rows wrap and their heights are
        // content-driven; the laid-out bounds are recorded every frame and `Ui::reconcile` resizes / re-places the window.
        div().size_full().flex().items_start().justify_start()
            .on_children_prepainted(move |b, _, _| slot.set(b.first().copied()))
            .child(div().flex_none().w(px(l.w as f32)).p(px(PAD as f32)).flex().flex_col().overflow_hidden()
            .on_children_prepainted(move |b, _, _| *rows_slot.borrow_mut() = b)
            .bg(t.popover).text_color(t.popover_foreground).border_1().border_color(t.border).rounded(px(bridge::RADIUS as f32))
            .font(thai_font()).text_size(px(FONT_PX as f32))
            .children(self.items.iter().enumerate().map(|(i, label)| {
                div().id(i).min_h(px(ROW_H as f32)).flex().flex_row().items_center().justify_between().px(px(ROW_PX as f32)).py(px(5.)).rounded(px(5.))
                    .when(i == selected, |d| d.bg(sel_bg))
                    .when(i != selected, |d| d.hover(move |s| s.bg(hover)))
                    // full text, wrapped within the label column (no ellipsis). The keycap slot stays out of this column.
                    .child({ let tb = self.text_bounds.clone();
                        // fixed column; the inner text box shrinks to the rendered text so its width can be checked
                        div().w(px(l.label_w as f32)).flex_none().flex()
                            .on_children_prepainted(move |b, _, _| { if let Some(s) = tb.borrow_mut().get_mut(i) { *s = b.first().copied(); } })
                            .child(div().max_w(px(l.label_w as f32)).whitespace_normal().child(label.clone())) })
                    .when(i == selected, |d| d.child(Kbd::new(tab_key.clone())))
            }))).into_any_element()
    }
}

/// Measure each label with the real text system (same font + size as the view, incl. Thai fallbacks).
fn measure(window: &Window, items: &[String]) -> Vec<f64> {
    let f = thai_font();
    items.iter().map(|s| {
        let run = TextRun { len: s.len(), font: f.clone(), color: black(), background_color: None, underline: None, strikethrough: None };
        window.text_system().shape_line(s.clone().into(), px(FONT_PX as f32), &[run], None).width().as_f32() as f64
            + bridge::MEASURE_SLACK
    }).collect()
}

struct Ui { last: Option<Rect>, native: native::Native, handle: AnyWindowHandle, view: Entity<BubbleView>, visible: bool, anchor: Option<Anchor>, size: (f64, f64), rows: usize }
impl Ui {
    /// Measure labels -> width + first height guess -> place -> show. The exact height (and width, if fonts differ)
    /// comes from the real layout via `reconcile` on the next frames. Returns the initial placement.
    fn show(&mut self, items: Vec<String>, anchor: Anchor, cx: &mut App) -> Option<Placement> {
        let view = self.view.clone();
        let rows = items.len();
        let res = self.handle.update(cx, |_, window, cx| {
            // widen up to the visible width of the monitor the bubble will be on (minus margins), then wrap
            let mons = native::monitors();
            let max_w = anchor.caret.map(|c| (c.x, c.y)).or(anchor.mouse).and_then(|(x, y)| placement::monitor_for(&mons, x, y)).map(|i| bridge::max_width_for(mons[i].visible.w)).unwrap_or(600.0);
            let l = bridge::layout(&measure(window, &items), max_w);
            view.update(cx, |v, cx| { v.items = items; v.layout = l; v.selected = 0; v.panel.set(None); v.row_bounds.borrow_mut().clear(); cx.notify(); });
            Some(l)
        }).ok().flatten()?;
        self.anchor = Some(anchor); self.rows = rows; self.size = (0.0, 0.0);
        let p = self.place((res.w, res.h), cx);
        if p.is_some() { self.visible = true; }
        p
    }
    fn place(&mut self, size: (f64, f64), cx: &mut App) -> Option<Placement> {
        let anchor = self.anchor?;
        let mons = native::monitors();
        let p = placement::place(&anchor, size, &mons, &Opts::default())?;
        let rows = self.rows;
        // run after the current gpui update returns, so gpui isn't borrowed and receives the resulting resize
        let n = self.native.clone(); let r = p.rect;
        // unchanged rect (e.g. a caret-follow that didn't move): don't touch the window -> no flicker
        if self.last != Some(r) || !self.visible { self.last = Some(r); cx.foreground_executor().spawn(async move { native::show(&n, r) }).detach(); }
        let scale = mons[p.monitor].size_scale;
        let rb = self.view.read(cx).row_bounds.borrow().clone();
        bridge::set_geom(Some(if rb.len() == rows {
            Geom { frame: p.rect, scale, rows: rb.iter().map(|b| (b.origin.y.as_f32() as f64, b.size.height.as_f32() as f64)).collect() }
        } else { Geom::uniform(p.rect, rows, scale) }));
        self.size = size;
        Some(p)
    }
    /// Called every UI tick: if gpui's laid-out panel size differs from the size the window was placed with,
    /// re-run placement with the real size (so the flip/clamp uses it) and resize the window to it.
    fn reconcile(&mut self, cx: &mut App) -> Option<Placement> {
        if !self.visible { return None }
        let b = self.view.read(cx).panel.get()?;
        let real = (b.size.width.as_f32().ceil() as f64, b.size.height.as_f32().ceil() as f64);
        if (real.0 - self.size.0).abs() < 0.5 && (real.1 - self.size.1).abs() < 0.5 { return None }
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

fn open_bubble(cx: &mut App) -> Ui {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(300.), px(80.))))),
        titlebar: None, focus: false, show: false, kind: WindowKind::PopUp,
        is_movable: false, is_resizable: false, is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let (handle, view) = open_window(opts, cx, |_, cx| cx.new(|_| BubbleView { items: vec![], selected: 0, layout: bridge::layout(&[], 600.0), panel: Default::default(), row_bounds: Default::default(), text_bounds: Default::default(), marker: false })).expect("bubble window");
    let n = handle.update(cx, |_, window, _| {
        let n = native::native(window).expect("native bubble window");
        native::prepare(&n);
        // follow the system light/dark appearance
        window.observe_window_appearance(|window, cx| Theme::sync_system_appearance(Some(window), cx)).detach();
        n
    }).expect("bubble window");
    Ui { last: None, native: n, handle, view, visible: false, anchor: None, size: (0.0, 0.0), rows: 0 }
}

fn app() -> Application { application().with_assets(assets::Assets).with_quit_mode(QuitMode::Explicit) }
fn setup(cx: &mut App) {
    init(cx);
    #[cfg(target_os = "macos")] cx.set_activation_policy(ActivationPolicy::Accessory);
    Theme::sync_system_appearance(None, cx);
}

/// Run the UI until the capture thread reports `Stopped` (or Quit is chosen and it doesn't answer within 2 s).
pub fn run(rx: Receiver<UiMsg>) {
    app().run(move |cx| {
        setup(cx);
        let mut ui = open_bubble(cx);
        let tray = super::platform::tray::Tray::new().map_err(|e| eprintln!("tray: {e}")).ok();
        cx.spawn(async move |cx| {
            let mut quit_at: Option<Instant> = None;
            let mut settings_win: Option<settings::SettingsWin> = None;
            loop {
                let mut stop = false;
                cx.update(|cx| {
                    while let Ok(m) = rx.try_recv() { match m {
                        UiMsg::Show { items, anchor } => { ui.show(items, anchor, cx); }
                        UiMsg::Move(a) => { ui.reanchor(a, cx); }
                        UiMsg::Hide => ui.hide(cx),
                        UiMsg::Select(i) => ui.view.update(cx, |v, cx| { v.selected = i; cx.notify(); }),
                        UiMsg::Paused(p) => if let Some(t) = &tray { t.set_paused(p) },
                        UiMsg::Stopped => stop = true,
                        UiMsg::Settings(snap) => settings::apply_snapshot(&mut settings_win, snap, cx),
                    } }
                    ui.reconcile(cx);
                    if bridge::take(&bridge::REQ_OPEN_SETTINGS) { settings::open_or_focus(&mut settings_win, cx) }
                    match tray.as_ref().and_then(|t| t.poll()) {
        Some(super::platform::tray::TrayCmd::TogglePause) => bridge::REQ_TOGGLE_PAUSE.store(true, std::sync::atomic::Ordering::SeqCst),
        Some(super::platform::tray::TrayCmd::CheckUpdate) => { std::thread::spawn(|| { crate::platform::check_for_updates(); }); }
        Some(super::platform::tray::TrayCmd::Settings) => settings::open_or_focus(&mut settings_win, cx),
        Some(super::platform::tray::TrayCmd::Quit) => { bridge::REQ_QUIT.store(true, std::sync::atomic::Ordering::SeqCst); quit_at.get_or_insert(Instant::now() + Duration::from_secs(2)); }
        None => {}
    }
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
        native::set_radius(&ui.native, 0.0);
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

pub fn demo_items() -> Vec<String> {
    vec!["l;ylfu".into(), "สวัสดีครับ".into(), "ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋".into()]
}

/// `--demo-bubble`: show the bubble with sample (or `items`) rows anchored at each point (global units; a fake 18-unit
/// caret, every 4th point uses the mouse fallback), `secs` seconds each, then quit. No points: centre, bottom (flip) and
/// right edge of every monitor. With `shot`, shows once (first point or main-monitor centre), lets it lay out and paint,
/// prints native vs. laid-out geometry, captures the bubble's own window to `shot` (PNG), and quits.
pub fn demo(points: Vec<(f64, f64)>, secs: f64, shot: Option<String>, items: Option<Vec<String>>, select: usize) {
    let items = items.unwrap_or_else(demo_items);
    app().run(move |cx| {
        setup(cx);
        let mut ui = open_bubble(cx);
        let mut pts = points.clone();
        if pts.is_empty() {
            for m in native::monitors() { let v = m.visible; pts.extend([(v.x + v.w / 2.0, v.y + v.h / 2.0), (v.x + v.w * 0.3, v.bottom() - 40.0), (v.right() - 10.0, v.y + 60.0)]); }
        }
        if shot.is_some() { pts.truncate(1); }
        let items = items.clone(); let shot = shot.clone();
        cx.spawn(async move |cx| {
            for (i, (x, y)) in pts.iter().copied().enumerate() {
                let anchor = if i % 4 == 3 { Anchor { caret: None, field: None, mouse: Some((x, y)) } }
                    else { Anchor { caret: Some(Rect::new(x, y, 2.0, 18.0)), field: Some(Rect::new(x - 40.0, y - 6.0, 360.0, 30.0)), mouse: None } };
                let kind = if anchor.caret.is_some() { "caret" } else { "mouse" };
                let pl = cx.update(|cx| { let p = ui.show(items.clone(), anchor, cx); ui.view.update(cx, |v, cx| { v.selected = select; cx.notify(); }); p });
                eprintln!("demo {i}: anchor ({x},{y}) {kind} -> {:?}", pl);
                // let gpui lay out, then fit the window to the real panel (a couple of frames)
                for _ in 0..10 {
                    cx.background_executor().timer(Duration::from_millis(30)).await;
                    if let Some(p) = cx.update(|cx| ui.reconcile(cx)) { eprintln!("demo {i}: refit to laid-out size -> {:?}", p); }
                }
                if let Some(path) = &shot {
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let h = ui.handle; let v = ui.view.clone();
                    let info = cx.update(|cx| { let lay = v.read(cx).layout; let panel = v.read(cx).panel.get();
                        let vp = h.update(cx, |_, w, _| w.viewport_size()).ok();
                        format!("layout {:?}\npanel(laid out) {:?}\nviewport {:?}\n{}", lay, panel, vp, native::describe(&ui.native)) });
                    eprintln!("{info}");
                    // self-check: measured single-line width vs the rendered text box; a row must fit or wrap, never clip
                    let chk = cx.update(|cx| { let w = h.update(cx, |_, w, _| measure(w, &items)).unwrap_or_default();
                        let vv = v.read(cx); let lw = vv.layout.label_w; let tb = vv.text_bounds.borrow().clone(); let rb = vv.row_bounds.borrow().clone();
                        let pnl = vv.panel.get();
                        items.iter().enumerate().map(|(i, s)| {
                            let t = tb.get(i).copied().flatten(); let r = rb.get(i).copied();
                            let (tw, th) = t.map(|b| (b.size.width.as_f32() as f64, b.size.height.as_f32() as f64)).unwrap_or((-1.0, -1.0));
                            let (lpad, rpad) = match (t, r) { (Some(t), Some(r)) => { let tl = t.origin.x.as_f32() as f64; let rl = r.origin.x.as_f32() as f64;
                                (tl - rl, (rl + r.size.width.as_f32() as f64) - (tl + lw)) } _ => (-1.0, -1.0) };
                            let wraps = w[i] > lw;
                            // right inset includes the reserved Tab keycap; left inset is the row padding
                            let slot = bridge::BADGE_GAP + bridge::BADGE_W;
                            let pad_ok = (lpad - ROW_PX).abs() < 1.5 && (rpad - (ROW_PX + slot)).abs() < 1.5;
                            let ok = tw <= lw + 0.5 && (wraps || th < 1.5 * FONT_PX * 1.6) && pnl.is_some() && pad_ok;
                            format!("row {i}: chars {} measured {:.1} (incl slack) label_w {lw:.1} rendered {tw:.1}x{th:.1} pad L {lpad:.1} R {rpad:.1} (expect {:.1}/{:.1}) {} {}",
                                s.chars().count(), w[i], ROW_PX, ROW_PX + slot, if wraps { "WRAPS" } else { "fits" }, if ok { "OK" } else { "CHECK-FAIL" })
                        }).collect::<Vec<_>>().join("\n") });
                    eprintln!("{chk}");
                    match native::capture(&ui.native, path) { Ok(()) => eprintln!("screenshot -> {path}"), Err(e) => eprintln!("screenshot failed: {e}") }
                } else {
                    cx.background_executor().timer(Duration::from_secs_f64(secs)).await;
                }
            }
            cx.update(|cx| { ui.hide(cx); cx.quit(); });
        }).detach();
    });
}

fn demo_snap() -> crate::prefs::SettingsSnap {
    let learned = [("brb", 3.0, 0.0), ("gg", 2.0, 0.5), ("lmao", 1.5, 0.0)];
    crate::prefs::SettingsSnap {
        never: ["lol", "ok", "omg"].map(String::from).to_vec(),
        learned: learned.map(|(w, kept, conv)| typethai_core::personal::Learned { word: w.into(), kept, conv }).to_vec(),
        completion: crate::engine::CompleteMode::default(),
        complete_idle: std::time::Duration::from_millis(crate::engine::COMPLETE_IDLE_DEFAULT_MS),
        learn_words: true,
        accept: crate::engine::AcceptKeys::default(),
        words_learned: 1250,
        rewrite: crate::rewrite::Config::default(),
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
