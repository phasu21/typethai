//! Settings window: words that stay as typed, and the on-device learning list.
use crate::bridge;
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::ui::thai_font;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::*;
use typethai_core::personal::Learned;

pub struct SettingsWin { handle: AnyWindowHandle, view: Entity<SettingsView> }

struct SettingsView {
    input: Entity<InputState>,
    snap: SettingsSnap,
    error: Option<String>,
    loaded: bool,
    confirm_reset: bool,
}

#[derive(Clone, Copy)]
struct Paint { bg: Hsla, fg: Hsla, muted: Hsla, border: Hsla, danger: Hsla }

pub fn open_or_focus(slot: &mut Option<SettingsWin>, cx: &mut App) {
    if !focus_existing(slot, cx) { *slot = open_settings(cx); }
    bridge::request_settings();
}
fn focus_existing(slot: &Option<SettingsWin>, cx: &mut App) -> bool {
    let Some(win) = slot.as_ref() else { return false };
    win.handle.update(cx, |_, window, cx| { window.activate_window(); cx.activate(true); }).is_ok()
}
pub fn apply_snapshot(slot: &mut Option<SettingsWin>, snap: SettingsSnap, cx: &mut App) {
    let alive = slot.as_ref().is_some_and(|win| win.handle.update(cx, |_, _, _| ()).is_ok());
    if !alive { *slot = None; return; }
    let Some(win) = slot.as_ref() else { return };
    win.view.update(cx, |v, cx| v.show(snap, cx));
}

fn open_settings(cx: &mut App) -> Option<SettingsWin> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(480.), px(560.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("typeThai Settings".into()), ..Default::default() }),
        focus: true, show: true, kind: WindowKind::Normal, is_movable: true, is_resizable: true,
        window_min_size: Some(size(px(420.), px(420.))),
        ..Default::default()
    };
    let opened = open_window(opts, cx, |window, cx| build_view(window, cx));
    let (handle, view) = match opened { Ok(v) => v, Err(e) => { eprintln!("typeThai: settings window: {e}"); return None } };
    cx.activate(true);
    handle.update(cx, |_, window, _| {
        window.observe_window_appearance(|window, cx| gpui_kit::component::theme::Theme::sync_system_appearance(Some(window), cx)).detach();
    }).ok()?;
    Some(SettingsWin { handle, view })
}
fn build_view(window: &mut Window, cx: &mut App) -> Entity<SettingsView> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("ok"));
    let watch = input.clone();
    let view = cx.new(|_| SettingsView { input, snap: SettingsSnap::default(), error: None, loaded: false, confirm_reset: false });
    view.update(cx, |_, cx| {
        cx.subscribe_in(&watch, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: false, shift: false } = ev { this.add_word(window, cx); }
        }).detach();
    });
    watch.read(cx).focus_handle(cx).focus(window, cx);
    view
}

fn paint_of(cx: &App) -> Paint {
    let t = cx.theme();
    Paint { bg: t.background, fg: t.foreground, muted: t.muted_foreground, border: t.border, danger: t.danger_foreground }
}
fn heading(text: &str) -> impl IntoElement {
    div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(text.to_string())
}
fn note(paint: Paint, text: &str) -> AnyElement {
    div().text_size(px(13.)).text_color(paint.muted).child(text.to_string()).into_any_element()
}
fn group(rows: Vec<AnyElement>, paint: Paint) -> AnyElement {
    div().rounded(px(10.)).border_1().border_color(paint.border).overflow_hidden().children(rows).into_any_element()
}
fn labeled_row(title: String, detail: Option<String>, paint: Paint, button: Button) -> AnyElement {
    let mut text = div().flex_1().min_w(px(0.)).overflow_hidden().flex().flex_col().justify_center().child(div().child(title));
    if let Some(detail) = detail {
        text = text.child(div().text_size(px(12.)).text_color(paint.muted).child(detail));
    }
    div().min_h(px(40.)).px(px(12.)).flex().flex_row().items_center().gap(px(8.)).child(text).child(button).into_any_element()
}
fn fmt_count(n: f64) -> String {
    if (n - n.round()).abs() < 0.05 { format!("{}", n.round() as i64) } else { format!("{n:.1}") }
}
fn count_text(row: &Learned) -> String {
    format!("kept {} · converted {}", fmt_count(row.kept), fmt_count(row.conv))
}

impl SettingsView {
    fn show(&mut self, snap: SettingsSnap, cx: &mut Context<Self>) {
        self.loaded = true;
        self.snap = snap;
        cx.notify();
    }
    fn add_word(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        let raw = self.input.read(cx).value().to_string();
        let Some(word) = normalize_word(&raw) else {
            self.error = Some("Enter one word, with no spaces.".into());
            cx.notify();
            return;
        };
        if self.snap.never.iter().any(|w| w == &word) {
            self.error = Some(format!("{word} is already on the list."));
            cx.notify();
            return;
        }
        self.error = None;
        self.input.update(cx, |input, cx| input.set_value("", window, cx));
        bridge::push_settings(SettingsCmd::AddNever(word));
        cx.notify();
    }
    fn remove_never(&mut self, word: String, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        bridge::push_settings(SettingsCmd::RemoveNever(word));
        cx.notify();
    }
    fn forget_word(&mut self, word: String, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        bridge::push_settings(SettingsCmd::Forget(word));
        cx.notify();
    }
    fn ask_reset(&mut self, cx: &mut Context<Self>) {
        if !self.confirm_reset { self.confirm_reset = true; cx.notify(); return; }
        self.confirm_reset = false;
        bridge::push_settings(SettingsCmd::ResetLearning);
        cx.notify();
    }
    fn never_list(&self, paint: Paint, cx: &mut Context<Self>) -> AnyElement {
        if !self.loaded { return note(paint, "Loading…"); }
        if self.snap.never.is_empty() { return note(paint, "No words yet. Add one, for example ok."); }
        let words = self.snap.never.clone();
        group(words.iter().map(|word| self.never_row(word, paint, cx)).collect(), paint)
    }
    fn never_row(&self, word: &str, paint: Paint, cx: &mut Context<Self>) -> AnyElement {
        let owned = word.to_string();
        let button = Button::new(SharedString::from(format!("rm-{owned}"))).label("Remove").ghost().small()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.remove_never(owned.clone(), cx)));
        labeled_row(word.to_string(), None, paint, button)
    }
    fn learn_list(&self, paint: Paint, cx: &mut Context<Self>) -> AnyElement {
        if !self.loaded { return note(paint, "Loading…"); }
        if self.snap.learned.is_empty() {
            return note(paint, "Nothing learned yet. Undoing a suggestion teaches typeThai to leave that word alone.");
        }
        let rows = self.snap.learned.clone();
        group(rows.iter().map(|row| self.learn_row(row, paint, cx)).collect(), paint)
    }
    fn learn_row(&self, row: &Learned, paint: Paint, cx: &mut Context<Self>) -> AnyElement {
        let word = row.word.clone();
        let button = Button::new(SharedString::from(format!("fg-{word}"))).label("Forget").ghost().small()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.forget_word(word.clone(), cx)));
        labeled_row(row.word.clone(), Some(count_text(row)), paint, button)
    }
    fn never_section(&self, paint: Paint, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap(px(8.))
            .child(heading("Don't translate"))
            .child(note(paint, "These words stay as you type them. ok will not be offered as นา. You can type ok or นา."))
            .child(self.add_row(cx))
            .children(self.error.clone().map(|err| div().text_size(px(12.)).text_color(paint.danger).child(err)))
            .child(self.never_list(paint, cx))
    }
    fn add_row(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.input.clone();
        div().flex().flex_row().items_center().gap(px(8.))
            .child(div().flex_1().child(Input::new(&input)))
            .child(Button::new("add-never").label("Add").primary()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.add_word(window, cx))))
    }
    fn learn_section(&self, paint: Paint, cx: &mut Context<Self>) -> impl IntoElement {
        let label = if self.confirm_reset { "Click again to reset" } else { "Reset learning" };
        div().flex().flex_col().gap(px(8.))
            .child(heading("Learning"))
            .child(note(paint, "Words typeThai has learned from your undos and manual conversions."))
            .child(self.learn_list(paint, cx))
            .child(Button::new("reset-learning").label(label).danger()
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.ask_reset(cx))))
            .child(note(paint, "This clears learned words. Words you asked not to translate stay."))
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let paint = paint_of(cx);
        div().size_full().flex().flex_col().bg(paint.bg).text_color(paint.fg).font(thai_font())
            .child(div().id("settings-scroll").size_full().overflow_y_scroll().p(px(20.)).flex().flex_col().gap(px(28.))
                .child(self.never_section(paint, cx))
                .child(self.learn_section(paint, cx)))
    }
}
