//! Settings window: when word suggestions show, words that stay as typed, the on-device learning list, and AI rewrite,
//! one sidebar page each.
use crate::bridge;
use crate::engine::CompleteMode;
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::rewrite;
use crate::ui::thai_font;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::setting::{RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{ActiveTheme, IconName, Sizable};
use gpui_kit::*;
use typethai_core::personal::Learned;

pub struct SettingsWin { pub(super) handle: AnyWindowHandle, view: Entity<SettingsView> }

struct SettingsView {
    input: Entity<InputState>,
    snap: SettingsSnap,
    error: Option<String>,
    loaded: bool,
    confirm_reset: bool,
    key: Entity<InputState>,
    model: SharedString,
    base_url: SharedString,
    instructions: Entity<TextareaState>,
    key_state: KeyState,
    rewrite_error: Option<String>,
    first_page: usize,
}
enum KeyState { Checking, Missing, Saved, Failed(String) }
const COMPLETION: [(CompleteMode, &str); 3] = [(CompleteMode::Off, "Off"), (CompleteMode::Pause, "After a pause"), (CompleteMode::Typing, "While typing")];
const KEY_STORE: &str = if cfg!(windows) { "Credential Manager" } else { "the Keychain" };

#[derive(Clone, Copy)]
struct Paint { muted: Hsla, border: Hsla, danger: Hsla }

pub fn open_or_focus(slot: &mut Option<SettingsWin>, cx: &mut App) {
    if !focus_existing(slot, cx) { *slot = open_settings(0, None, cx); }
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
    let _ = win.handle.update(cx, |_, window, cx| win.view.update(cx, |v, cx| v.show(snap, window, cx)));
}
/// `--demo-settings`: the window on `page`, filled with `snap`, never reading the keychain.
pub fn open_demo(page: usize, snap: SettingsSnap, cx: &mut App) -> Option<SettingsWin> {
    let mut slot = open_settings(page, Some(KeyState::Missing), cx);
    apply_snapshot(&mut slot, snap, cx);
    slot
}

fn open_settings(first_page: usize, key_state: Option<KeyState>, cx: &mut App) -> Option<SettingsWin> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(760.), px(560.)), cx))),
        titlebar: Some(TitlebarOptions { title: Some("typeThai Settings".into()), ..Default::default() }),
        focus: true, show: true, kind: WindowKind::Normal, is_movable: true, is_resizable: true,
        window_min_size: Some(size(px(560.), px(420.))),
        ..Default::default()
    };
    let opened = open_window(opts, cx, |window, cx| build_view(first_page, key_state, window, cx));
    let (handle, view) = match opened { Ok(v) => v, Err(e) => { eprintln!("typeThai: settings window: {e}"); return None } };
    cx.activate(true);
    handle.update(cx, |_, window, _| {
        window.observe_window_appearance(|window, cx| gpui_kit::component::theme::Theme::sync_system_appearance(Some(window), cx)).detach();
    }).ok()?;
    Some(SettingsWin { handle, view })
}
fn build_view(first_page: usize, key_state: Option<KeyState>, window: &mut Window, cx: &mut App) -> Entity<SettingsView> {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("ok"));
    let watch = input.clone();
    let key = cx.new(|cx| InputState::new(window, cx).masked(true).placeholder("API key"));
    let instructions = cx.new(|cx| TextareaState::new(window, cx).auto_grow(3, 8).placeholder("For example: Use formal Thai. Keep brand names in English."));
    let checked = key_state.is_some();
    let view = cx.new(|_| SettingsView {
        input, snap: SettingsSnap::default(), error: None, loaded: false, confirm_reset: false, key,
        model: SharedString::default(), base_url: SharedString::default(), instructions, key_state: key_state.unwrap_or(KeyState::Checking), rewrite_error: None, first_page,
    });
    view.update(cx, |_, cx| {
        cx.subscribe_in(&watch, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: false, shift: false } = ev { this.add_word(window, cx); }
        }).detach();
    });
    watch.read(cx).focus_handle(cx).focus(window, cx);
    if checked { return view }
    view.update(cx, |_, cx| {
        // the keyring may block on a Keychain prompt: never on the UI thread
        cx.spawn(async move |this, cx| {
            let has = cx.background_executor().spawn(async { rewrite::has_key() }).await;
            let _ = this.update(cx, |v, cx| { v.key_state = if has { KeyState::Saved } else { KeyState::Missing }; cx.notify(); });
        }).detach();
    });
    view
}

fn paint_of(cx: &App) -> Paint {
    let t = cx.theme();
    Paint { muted: t.muted_foreground, border: t.border, danger: t.danger_foreground }
}
fn note(paint: Paint, text: &str) -> AnyElement {
    div().text_size(px(13.)).text_color(paint.muted).child(text.to_string()).into_any_element()
}
fn error_text(paint: Paint, err: &Option<String>) -> Option<Div> {
    err.clone().map(|err| div().text_size(px(12.)).text_color(paint.danger).child(err))
}
fn group(rows: Vec<AnyElement>, paint: Paint) -> AnyElement {
    div().rounded(px(10.)).border_1().border_color(paint.border).overflow_hidden().children(rows).into_any_element()
}
fn labeled_row(title: String, detail: Option<String>, paint: Paint, button: Button) -> AnyElement {
    let mut text = div().flex_1().min_w(px(0.)).overflow_hidden().flex().flex_col().justify_center().child(div().child(title));
    if let Some(detail) = detail {
        text = text.child(div().text_size(px(12.)).text_color(paint.muted).child(detail));
    }
    div().min_h(px(40.)).px(px(12.)).py(px(6.)).flex().flex_row().items_center().gap(px(8.)).child(text).child(button).into_any_element()
}
/// Same width rule as the library's string fields, so custom fields line up with them.
fn field_box(options: &RenderOptions) -> Div {
    if options.layout() == Axis::Horizontal { div().w_64() } else { div().w_full() }
}
fn fmt_count(n: f64) -> String {
    if (n - n.round()).abs() < 0.05 { format!("{}", n.round() as i64) } else { format!("{n:.1}") }
}
fn count_text(row: &Learned) -> String {
    format!("kept {} · converted {}", fmt_count(row.kept), fmt_count(row.conv))
}
fn click(view: &Entity<SettingsView>, f: impl Fn(&mut SettingsView, &mut Window, &mut Context<SettingsView>) + 'static) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let view = view.clone();
    move |_, window, cx| view.update(cx, |v, cx| f(v, window, cx))
}
/// A custom element item that reads the view each time the page renders it.
fn element(view: &Entity<SettingsView>, render: impl Fn(&SettingsView, &Entity<SettingsView>, &RenderOptions, Paint) -> AnyElement + 'static) -> SettingItem {
    let view = view.clone();
    SettingItem::render(move |options, _, cx| render(view.read(cx), &view, options, paint_of(cx)))
}
fn custom_field(view: &Entity<SettingsView>, render: impl Fn(&SettingsView, &Entity<SettingsView>, &RenderOptions, Paint) -> AnyElement + 'static) -> SettingField<SharedString> {
    let view = view.clone();
    SettingField::render(move |options, _, cx| render(view.read(cx), &view, options, paint_of(cx)))
}
fn draft_field(view: &Entity<SettingsView>, get: fn(&SettingsView) -> SharedString, set: fn(&mut SettingsView, SharedString)) -> SettingField<SharedString> {
    let (read, write) = (view.clone(), view.clone());
    SettingField::input(move |cx| get(read.read(cx)), move |s, cx| write.update(cx, |v, _| set(v, s)))
}
/// Search matches item titles, descriptions and keywords, not page titles: every item on a page carries the page's keywords.
fn page(title: &str, icon: IconName, description: String, keywords: Vec<SharedString>, items: Vec<SettingItem>) -> SettingPage {
    SettingPage::new(title.to_string()).icon(icon).description(description)
        .group(SettingGroup::new().items(items.into_iter().map(|item| item.keywords(keywords.clone()))))
}
fn keywords<'a>(fixed: &[&str], words: impl Iterator<Item = &'a String>) -> Vec<SharedString> {
    fixed.iter().map(|s| SharedString::from(s.to_string())).chain(words.map(|w| SharedString::from(w.clone()))).collect()
}

impl SettingsView {
    fn show(&mut self, snap: SettingsSnap, window: &mut Window, cx: &mut Context<Self>) {
        // fill the rewrite fields once, so later snapshots do not overwrite what is being typed
        if !self.loaded { self.fill_rewrite(&snap.rewrite, window, cx); }
        self.loaded = true;
        self.snap = snap;
        cx.notify();
    }
    fn fill_rewrite(&mut self, c: &rewrite::Config, window: &mut Window, cx: &mut Context<Self>) {
        self.model = c.model.clone().into();
        self.base_url = c.base_url.clone().into();
        let instructions = c.instructions.clone();
        self.instructions.update(cx, |i, cx| i.set_value(instructions, window, cx));
    }
    fn set_rewrite_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        bridge::push_settings(SettingsCmd::SetRewrite(rewrite::Config { enabled, ..self.snap.rewrite.clone() }));
        cx.notify();
    }
    fn save_rewrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let instructions = self.instructions.read(cx).value();
        match rewrite::Config::new(self.snap.rewrite.enabled, &self.base_url, &self.model, &instructions) {
            Ok(c) => { self.rewrite_error = None; self.fill_rewrite(&c, window, cx); bridge::push_settings(SettingsCmd::SetRewrite(c)); }
            Err(e) => self.rewrite_error = Some(e),
        }
        cx.notify();
    }
    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = self.key.read(cx).value().to_string();
        self.key.update(cx, |i, cx| i.set_value("", window, cx));
        self.key_state = KeyState::Checking;
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { rewrite::save_key(&key).map(|()| !key.trim().is_empty()) }).await;
            let _ = this.update(cx, |v, cx| { v.key_state = match r { Ok(true) => KeyState::Saved, Ok(false) => KeyState::Missing, Err(e) => KeyState::Failed(e) }; cx.notify(); });
        }).detach();
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
    fn never_list(&self, view: &Entity<Self>, paint: Paint) -> AnyElement {
        if !self.loaded { return note(paint, "Loading…"); }
        if self.snap.never.is_empty() { return note(paint, "No words yet. Add one, for example ok."); }
        group(self.snap.never.iter().map(|word| {
            let owned = word.clone();
            let button = Button::new(SharedString::from(format!("rm-{word}"))).label("Remove").ghost().small()
                .on_click(click(view, move |v, _, cx| v.remove_never(owned.clone(), cx)));
            labeled_row(word.clone(), None, paint, button)
        }).collect(), paint)
    }
    fn learn_list(&self, view: &Entity<Self>, paint: Paint) -> AnyElement {
        if !self.loaded { return note(paint, "Loading…"); }
        if self.snap.learned.is_empty() {
            return note(paint, "Nothing learned yet. Undoing a suggestion teaches typeThai to leave that word alone.");
        }
        group(self.snap.learned.iter().map(|row| {
            let word = row.word.clone();
            let button = Button::new(SharedString::from(format!("fg-{word}"))).label("Forget").ghost().small()
                .on_click(click(view, move |v, _, cx| v.forget_word(word.clone(), cx)));
            labeled_row(row.word.clone(), Some(count_text(row)), paint, button)
        }).collect(), paint)
    }
    fn never_page(&self, view: &Entity<Self>) -> SettingPage {
        let add = custom_field(view, |v, view, options, paint| field_box(options).flex().flex_col().gap(px(4.))
            .child(div().flex().flex_row().items_center().gap(px(8.)).child(div().flex_1().child(Input::new(&v.input)))
                .child(Button::new("add-never").label("Add").primary().on_click(click(view, |v, window, cx| v.add_word(window, cx)))))
            .children(error_text(paint, &v.error)).into_any_element());
        page("Don't translate", IconName::Ban,
            "These words stay as you type them. ok will not be offered as นา. You can type ok or นา.".into(),
            keywords(&["don't translate", "translate", "word", "add", "remove", "ok", "นา"], self.snap.never.iter()),
            vec![SettingItem::new("Add a word", add), element(view, |v, view, _, paint| v.never_list(view, paint))])
    }
    fn learn_page(&self, view: &Entity<Self>) -> SettingPage {
        let reset = custom_field(view, |v, view, _, _| Button::new("reset-learning").label(if v.confirm_reset { "Click again to reset" } else { "Reset" }).danger()
            .on_click(click(view, |v, _, cx| v.ask_reset(cx))).into_any_element());
        page("Learning", IconName::BookOpen, "Words typeThai has learned from your undos and manual conversions.".into(),
            keywords(&["learning", "learned", "forget", "reset", "undo"], self.snap.learned.iter().map(|row| &row.word)),
            vec![element(view, |v, view, _, paint| v.learn_list(view, paint)),
                SettingItem::new("Reset learning", reset).description("This clears learned words. Words you asked not to translate stay.")])
    }
    fn completion_page(&self, view: &Entity<Self>) -> SettingPage {
        let options = COMPLETION.iter().map(|(_, label)| (SharedString::from(*label), SharedString::from(*label))).collect();
        let (read, write) = (view.clone(), view.clone());
        let mode = SettingField::dropdown(options,
            move |cx| COMPLETION.iter().find(|(m, _)| *m == read.read(cx).snap.completion).map_or("", |(_, l)| l).into(),
            move |label, cx| if let Some((m, _)) = COMPLETION.iter().find(|(_, l)| *l == label.as_ref()) { write.update(cx, |v, cx| v.set_completion(*m, cx)) });
        page("Word suggestions", IconName::CaseSensitive, "The bubble can finish the word you are typing, in Thai or English. Tab accepts the highlighted word.".into(),
            keywords(&["word suggestions", "completion", "complete", "autocomplete", "pause", "typing"], std::iter::empty()),
            vec![SettingItem::new("Show suggestions", mode)])
    }
    fn set_completion(&mut self, mode: CompleteMode, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        self.snap.completion = mode;
        bridge::push_settings(SettingsCmd::SetCompletion(mode));
        cx.notify();
    }
    fn rewrite_page(&self, view: &Entity<Self>) -> SettingPage {
        let hotkey = if cfg!(windows) { "Ctrl+Alt+R" } else { "Ctrl+Option+R" };
        let key_note = match &self.key_state {
            KeyState::Checking => "Checking for a saved key…".to_string(),
            KeyState::Missing => "No key saved yet.".to_string(),
            KeyState::Saved => format!("Key saved in {KEY_STORE}. Save an empty key to remove it."),
            KeyState::Failed(e) => format!("Could not save the key: {e}"),
        };
        let enabled = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.rewrite.enabled, move |on, cx| write.update(cx, |v, cx| v.set_rewrite_enabled(on, cx))) };
        let key = custom_field(view, |v, view, options, _| field_box(options).child(Input::new(&v.key)
            .suffix(Button::new("save-key").label("Save key").ghost().xsmall().on_click(click(view, |v, window, cx| v.save_key(window, cx))))).into_any_element());
        let instructions = custom_field(view, |v, _, options, _| field_box(options).child(Textarea::new(&v.instructions)).into_any_element());
        let save = element(view, |v, view, _, paint| div().flex().flex_row().items_center().justify_end().gap(px(12.))
            .children(error_text(paint, &v.rewrite_error).map(|e| e.flex_1()))
            .child(Button::new("save-rewrite").label("Save").primary().on_click(click(view, |v, window, cx| v.save_rewrite(window, cx))))
            .into_any_element());
        page("AI rewrite", IconName::Bot,
            format!("Select text and press {hotkey} to fix typos or translate between Thai and English. The text is sent to the service below."),
            keywords(&["ai rewrite", "rewrite", "api key", "openrouter", "model", "base url", "translate", "typo", "hotkey", "prompt", "instructions", hotkey], std::iter::empty()),
            vec![SettingItem::new("Rewrite selected text", enabled), SettingItem::new("API key", key).description(key_note),
                SettingItem::new("Model", draft_field(view, |v| v.model.clone(), |v, s| v.model = s)),
                SettingItem::new("Base URL", draft_field(view, |v| v.base_url.clone(), |v, s| v.base_url = s)),
                SettingItem::new("Instructions", instructions).layout(Axis::Vertical).description("Added to both Fix typos and Translate."), save])
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let pages = [self.completion_page(&view), self.never_page(&view), self.learn_page(&view), self.rewrite_page(&view)];
        let t = cx.theme();
        div().size_full().bg(t.background).text_color(t.foreground).font(thai_font())
            .child(Settings::new("settings").sidebar_width(px(200.)).sidebar_size_range(px(160.)..px(280.))
                .default_selected_index(SelectIndex { page_ix: self.first_page, group_ix: None }).pages(pages))
    }
}
