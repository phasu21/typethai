//! Settings window: when word suggestions show, words that stay as typed, the on-device learning list, AI rewrite and
//! updates, one sidebar page each.
use crate::bridge::{self, BubbleLayout};
use crate::engine::{AcceptKeys, CompleteMode, BADGE_IDLE_CHOICES_MS, COMPLETE_IDLE_CHOICES_MS, TAB_HOLD_CHOICES_MS};
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::{local_setup, rewrite, update};
use crate::ui::thai_font;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::hover_card::HoverCard;
use gpui_kit::component::label::Label;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::setting::{RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, IconName, Sizable};
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use typethai_core::personal::Learned;

pub struct SettingsWin { pub(super) handle: AnyWindowHandle, view: Entity<SettingsView> }

struct SettingsView {
    input: Entity<InputState>,
    snap: SettingsSnap,
    error: Option<String>,
    loaded: bool,
    confirm_reset: bool,
    confirm_forget: bool,
    key: Entity<InputState>,
    model: SharedString,
    base_url: SharedString,
    instructions: Entity<TextareaState>,
    key_state: KeyState,
    rewrite_error: Option<String>,
    /// The Ollama tag being used; applied at once, before the next snapshot arrives.
    local_model: SharedString,
    local: LocalState,
    local_checked: bool,
    /// Set by `--local-state` / `--local-cycle`: the row shows this instead of asking Ollama.
    demo: Option<LocalDemo>,
    /// Which half of the AI rewrite page is shown: the local model, or the cloud service.
    rewrite_tab: RewriteTab,
    first_page: usize,
}
/// The AI rewrite page's two halves: the local Ollama model, or the cloud service with a key.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RewriteTab { Local, Cloud }
/// The Local model setup row: the last check, the download's progress line, or its result.
#[derive(Clone)]
pub enum LocalState { Unknown, Step(local_setup::Step), Working(String), Done(String), Failed(String) }
/// `--demo-settings --local-state <name>` / `--local-cycle`: show the row's states instead of checking the machine, so
/// every branch (and the button it drives) can be reviewed without moving Ollama around. Clicks advance the sequence.
#[derive(Clone)]
pub enum LocalDemo { Fixed(LocalState), Cycle(usize) }
/// Every state the row can be in, in the order the cycle shows them.
pub fn demo_states() -> Vec<LocalState> {
    let (tag, size) = (rewrite::DEFAULT_LOCAL_MODEL, "9.6 GB");
    vec![
        LocalState::Unknown,
        LocalState::Step(local_setup::Step::Install),
        LocalState::Step(local_setup::Step::Start),
        LocalState::Step(local_setup::Step::Pull),
        LocalState::Working(format!("Downloading {tag} — this can take a while")),
        LocalState::Working("pulling manifest".into()),
        LocalState::Working(format!("pulling 6d0b3f1c2a44: 42% ▕████████░░░░░░░░░░░░▏ 4.0 GB/{size}")),
        LocalState::Working("verifying sha256 digest".into()),
        LocalState::Done(format!("{tag} is ready")),
        LocalState::Failed("ollama pull failed (exit status: 1)".into()),
    ]
}
/// `--local-state` names, including the ones people reach for first.
pub fn demo_local(name: &str) -> Option<LocalDemo> {
    let states = demo_states();
    let fixed = |n: usize| Some(LocalDemo::Fixed(states[n].clone()));
    match name {
        "cycle" => Some(LocalDemo::Cycle(0)),
        "checking" | "unknown" => fixed(0),
        "install" | "not-installed" | "no-ollama" => fixed(1),
        "start" | "not-running" | "stopped" => fixed(2),
        "pull" | "not-downloaded" | "missing-model" => fixed(3),
        "downloading" | "download" => fixed(5),
        "ready" => fixed(8),
        "failed" | "error" => fixed(9),
        _ => None,
    }
}
enum KeyState { Checking, Missing, Saved, Failed(String) }
const COMPLETION: [(CompleteMode, &str); 3] = [(CompleteMode::Off, "Off"), (CompleteMode::Pause, "After a pause"), (CompleteMode::Typing, "While typing")];
const ACCEPT: [(AcceptKeys, &str); 2] = [(AcceptKeys::Tab, "Tab"), (AcceptKeys::TabOrSpace, "Tab or Space")];
const BUBBLE: [(BubbleLayout, &str); 2] = [(BubbleLayout::Horizontal, "Horizontal"), (BubbleLayout::Vertical, "Vertical")];
/// "Pick with": (pick_arrows, pick_ctrl). Both off (only from a hand-edited prefs.json) shows no choice.
const PICK: [((bool, bool), &str); 3] = [((true, false), "Arrow keys"), ((false, true), "Ctrl+1–3"), ((true, true), "Arrow keys and Ctrl+1–3")];
/// The dropdown's "never bring it back" choice.
const BADGE_OFF: &str = "Off";
fn delay_label(ms: u64) -> SharedString { if ms % 1000 == 0 { format!("{} s", ms / 1000).into() } else { format!("{ms} ms").into() } }
const KEY_STORE: &str = if cfg!(windows) { "Credential Manager" } else { "the Keychain" };

#[derive(Clone, Copy)]
struct Paint { muted: Hsla, border: Hsla, danger: Hsla }

pub fn open_or_focus(slot: &mut Option<SettingsWin>, cx: &mut App) {
    if !focus_existing(slot, cx) { *slot = open_settings(bridge::SETTINGS_PAGE.load(std::sync::atomic::Ordering::SeqCst), None, cx); }
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
/// Redraw an open window: the updates page shows the updater's status.
pub fn refresh(slot: &Option<SettingsWin>, cx: &mut App) {
    if let Some(win) = slot { let _ = win.handle.update(cx, |_, _, cx| win.view.update(cx, |_, cx| cx.notify())); }
}
/// An ⓘ whose hover card holds the explanation that would otherwise be a paragraph under the row.
fn info_icon(id: &'static str, help: Vec<String>) -> AnyElement {
    HoverCard::new(ElementId::Name(id.into()))
        .anchor(Anchor::TopRight)
        .open_delay(std::time::Duration::from_millis(250))
        .trigger(Button::new(id).ghost().xsmall().compact().icon(IconName::Info))
        .content(move |_, _, _| div().w(px(320.)).flex().flex_col().gap(px(6.))
            .children(help.iter().map(|line| div().text_size(px(13.)).child(line.clone()))))
        .into_any_element()
}
/// What the setup button says, and whether it can be pressed: the whole state machine in one place.
pub fn local_face(state: &LocalState) -> (String, bool, bool) {
    match state {
        LocalState::Unknown => ("Checking…".into(), false, true),
        LocalState::Step(local_setup::Step::Install) => ("Get Ollama".into(), true, false),
        LocalState::Step(local_setup::Step::Start) => ("Start Ollama".into(), true, false),
        LocalState::Step(local_setup::Step::Pull) => ("Download model".into(), true, false),
        LocalState::Step(local_setup::Step::Ready) | LocalState::Done(_) => ("Ready".into(), false, false),
        LocalState::Working(line) => (match local_setup::pull_percent(line) {
            Some(percent) => format!("Downloading {percent}%"),
            None if line.starts_with("Starting") => "Starting Ollama…".into(),
            None => "Downloading…".into(),
        }, false, true),
        LocalState::Failed(_) => ("Try again".into(), true, false),
    }
}

/// One row in the library's own shape — title, an optional one-line note, the control on the right — with the long
/// explanation behind an ⓘ instead of a paragraph.
fn help_row(id: &'static str, title: &str, note: Option<String>, help: Vec<String>, paint: Paint, control: AnyElement) -> AnyElement {
    let mut head = div().flex().flex_row().items_center().gap(px(6.)).child(Label::new(title.to_string()).text_sm());
    if !help.is_empty() { head = head.child(info_icon(id, help)); }
    let mut text = div().flex_1().max_w(px(380.)).flex().flex_col().gap(px(2.)).child(head);
    if let Some(note) = note { text = text.child(div().text_size(px(13.)).text_color(paint.muted).child(note)); }
    div().w_full().flex().flex_row().items_center().justify_between().gap(px(12.)).child(text).child(control).into_any_element()
}
/// `--demo-settings`: the window on `page`, filled with `snap`, never reading the keychain.
pub fn open_demo(page: usize, snap: SettingsSnap, cx: &mut App, local: Option<LocalDemo>) -> Option<SettingsWin> {
    let mut slot = open_settings(page, Some(KeyState::Missing), cx);
    if let Some(win) = slot.as_ref() {
        let _ = win.handle.update(cx, |_, _, cx| win.view.update(cx, |v, _| { v.demo = local; }));
    }
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
        input, snap: SettingsSnap::default(), error: None, loaded: false, confirm_reset: false, confirm_forget: false, key,
        model: SharedString::default(), base_url: SharedString::default(), instructions, key_state: key_state.unwrap_or(KeyState::Checking), rewrite_error: None,
        local_model: SharedString::from(rewrite::DEFAULT_LOCAL_MODEL), local: LocalState::Unknown, local_checked: false, demo: None, rewrite_tab: RewriteTab::Local, first_page,
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
fn page(title: &str, icon: impl Into<Icon>, description: String, keywords: Vec<SharedString>, items: Vec<SettingItem>) -> SettingPage {
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
        if !self.local_checked {
            self.local_checked = true;
            match self.demo.clone() {
                Some(LocalDemo::Cycle(index)) => self.step_demo(index, cx),
                Some(LocalDemo::Fixed(state)) => self.local = state,
                None => self.check_local(cx),
            }
        }
        cx.notify();
    }
    fn fill_rewrite(&mut self, c: &rewrite::Config, window: &mut Window, cx: &mut Context<Self>) {
        self.model = c.model.clone().into();
        self.base_url = c.base_url.clone().into();
        self.local_model = c.local_model.clone().into();
        let instructions = c.instructions.clone();
        self.instructions.update(cx, |i, cx| i.set_value(instructions, window, cx));
    }
    fn set_rewrite_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        bridge::push_settings(SettingsCmd::SetRewrite(rewrite::Config { enabled, ..self.snap.rewrite.clone() }));
        cx.notify();
    }
    fn save_rewrite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let instructions = self.instructions.read(cx).value();
        match rewrite::Config::new(self.snap.rewrite.enabled, &self.base_url, &self.model, &instructions)
            .and_then(|c| c.with_local_model(&self.local_model))
        {
            Ok(c) => { self.rewrite_error = None; self.fill_rewrite(&c, window, cx); bridge::push_settings(SettingsCmd::SetRewrite(c)); }
            Err(e) => self.rewrite_error = Some(e),
        }
        cx.notify();
    }
    /// The setup button *is* the state machine: its face says what pressing it does, or what is running.
    fn local_face(&self) -> (String, bool, bool) { local_face(&self.local) }

    /// The one line the row shows without hovering: what went wrong.
    fn local_note(&self) -> Option<String> {
        match &self.local { LocalState::Failed(e) => Some(e.clone()), _ => None }
    }
    /// What the row's ⓘ explains, including the live detail the button cannot hold.
    fn local_help(&self) -> Vec<String> {
        let mut help = vec![
            "Set up starts Ollama if it is installed but not running, then downloads the chosen model (ollama pull); the button shows the download.".into(),
            format!("{} answers on this computer only (127.0.0.1:11434); the selected text never leaves it.", self.local_model),
            "Get Ollama opens ollama.com/download.".into(),
        ];
        if let LocalState::Working(line) = &self.local { help.insert(0, line.clone()); }
        help
    }
    /// Ask Ollama what is missing, off the UI thread.
    fn check_local(&mut self, cx: &mut Context<Self>) {
        let tag = self.local_model.to_string();
        self.local = LocalState::Unknown;
        cx.spawn(async move |this, cx| {
            let step = cx.background_executor().spawn(async move { local_setup::check(&tag) }).await;
            let _ = this.update(cx, |v, cx| { v.local = LocalState::Step(step); cx.notify(); });
        }).detach();
        cx.notify();
    }
    /// Show one demo state and move on to the next after a beat, so the whole cycle can be watched.
    fn step_demo(&mut self, index: usize, cx: &mut Context<Self>) {
        let states = demo_states();
        self.local = states[index % states.len()].clone();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(1600)).await;
            let alive = this.update(cx, |v, cx| {
                let Some(LocalDemo::Cycle(index)) = v.demo.clone() else { return };
                let states = demo_states();
                v.local = states[(index + 1) % states.len()].clone();
                v.demo = Some(LocalDemo::Cycle(index + 1));
                cx.notify();
            });
            if alive.is_err() { break }
        }).detach();
    }
    /// Start Ollama and download the model if either is missing, keeping the row's note on the latest line.
    fn setup_local(&mut self, cx: &mut Context<Self>) {
        if let Some(demo) = self.demo.clone() {
            // a demo window never touches Ollama; clicking walks the states instead
            let index = match demo { LocalDemo::Fixed(_) => 0, LocalDemo::Cycle(index) => index + 1 };
            self.demo = Some(LocalDemo::Cycle(index));
            self.step_demo(index, cx);
            return;
        }
        if matches!(self.local, LocalState::Step(local_setup::Step::Install)) { local_setup::open_download_page(); return }
        let tag = self.local_model.to_string();
        self.local = LocalState::Working("starting".into());
        let (progress, finished, outcome) = (Arc::new(Mutex::new(String::new())), Arc::new(AtomicBool::new(false)), Arc::new(Mutex::new(None)));
        std::thread::spawn({
            let (progress, finished, outcome) = (Arc::clone(&progress), Arc::clone(&finished), Arc::clone(&outcome));
            move || {
                let mut log = |line: &str| *progress.lock().unwrap() = line.to_string();
                *outcome.lock().unwrap() = Some(local_setup::ensure(&tag, &mut log));
                finished.store(true, Ordering::SeqCst);
            }
        });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
                let (line, done, result) = (progress.lock().unwrap().clone(), finished.load(Ordering::SeqCst), outcome.lock().unwrap().clone());
                let alive = this.update(cx, |v, cx| {
                    v.local = match (done, result) {
                        (false, _) => LocalState::Working(if line.is_empty() { "starting".into() } else { line }),
                        (true, Some(Ok(msg))) => LocalState::Done(msg),
                        (true, Some(Err(e))) => LocalState::Failed(e),
                        (true, None) => LocalState::Unknown,
                    };
                    cx.notify();
                });
                if done || alive.is_err() { break }
            }
        }).detach();
        cx.notify();
    }
    /// Pick another Ollama tag: saved at once, then the row re-checks.
    fn set_rewrite_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.rewrite_tab = if index == 1 { RewriteTab::Cloud } else { RewriteTab::Local };
        cx.notify();
    }
    fn set_local_model(&mut self, tag: String, cx: &mut Context<Self>) {
        match self.snap.rewrite.clone().with_local_model(&tag) {
            Ok(c) => {
                self.rewrite_error = None;
                self.local_model = c.local_model.clone().into();
                bridge::push_settings(SettingsCmd::SetRewrite(c));
                self.check_local(cx);
            }
            Err(e) => { self.rewrite_error = Some(e); cx.notify() }
        }
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
            "These words stay as you type them: ok typed on the English layout is not offered as นา. On the Thai layout they are offered back as English, so นา offers ok.".into(),
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
        let delay = { let (read, write) = (view.clone(), view.clone());
            let options = COMPLETE_IDLE_CHOICES_MS.iter().map(|&ms| (delay_label(ms), delay_label(ms))).collect();
            SettingField::dropdown(options,
                move |cx| delay_label(read.read(cx).snap.complete_idle.as_millis() as u64),
                move |label, cx| if let Some(&ms) = COMPLETE_IDLE_CHOICES_MS.iter().find(|&&ms| delay_label(ms) == label) { write.update(cx, |v, cx| v.set_complete_idle(ms, cx)) }) };
        let accept = { let (read, write) = (view.clone(), view.clone());
            let options = ACCEPT.iter().map(|(_, label)| (SharedString::from(*label), SharedString::from(*label))).collect();
            SettingField::dropdown(options,
                move |cx| ACCEPT.iter().find(|(a, _)| *a == read.read(cx).snap.accept).map_or("", |(_, l)| l).into(),
                move |label, cx| if let Some((a, _)) = ACCEPT.iter().find(|(_, l)| *l == label.as_ref()) { write.update(cx, |v, cx| v.set_accept(*a, cx)) }) };
        let layout = { let (read, write) = (view.clone(), view.clone());
            let options = BUBBLE.iter().map(|(_, label)| (SharedString::from(*label), SharedString::from(*label))).collect();
            SettingField::dropdown(options,
                move |cx| BUBBLE.iter().find(|(b, _)| *b == read.read(cx).snap.bubble).map_or("", |(_, l)| l).into(),
                move |label, cx| if let Some((b, _)) = BUBBLE.iter().find(|(_, l)| *l == label.as_ref()) { write.update(cx, |v, cx| v.set_bubble(*b, cx)) }) };
        let pick = { let (read, write) = (view.clone(), view.clone());
            let options = PICK.iter().map(|(_, label)| (SharedString::from(*label), SharedString::from(*label))).collect();
            SettingField::dropdown(options,
                move |cx| { let s = &read.read(cx).snap; PICK.iter().find(|(p, _)| *p == (s.pick_arrows, s.pick_ctrl)).map_or("", |(_, l)| l).into() },
                move |label, cx| if let Some(&(p, _)) = PICK.iter().find(|(_, l)| *l == label.as_ref()) { write.update(cx, |v, cx| v.set_pick(p, cx)) }) };
        let badge = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.input_badge, move |on, cx| write.update(cx, |v, cx| v.set_input_badge(on, cx))) };
        let badge_delay = { let (read, write) = (view.clone(), view.clone());
            let options = std::iter::once((SharedString::from(BADGE_OFF), SharedString::from(BADGE_OFF)))
                .chain(BADGE_IDLE_CHOICES_MS.iter().map(|&ms| (delay_label(ms), delay_label(ms)))).collect();
            SettingField::dropdown(options,
                move |cx| read.read(cx).snap.badge_idle.map_or_else(|| BADGE_OFF.into(), |d| delay_label(d.as_millis() as u64)),
                move |label, cx| {
                    let ms = if label.as_ref() == BADGE_OFF { None } else { BADGE_IDLE_CHOICES_MS.iter().find(|&&ms| delay_label(ms) == label).map(|&ms| ms) };
                    if let Some(ms) = ms.or_else(|| (label.as_ref() == BADGE_OFF).then_some(0)) { write.update(cx, |v, cx| v.set_badge_idle((ms > 0).then(|| std::time::Duration::from_millis(ms)), cx)) }
                }) };
        let next = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.next_word, move |on, cx| write.update(cx, |v, cx| v.set_next_word(on, cx))) };
        let learn = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.learn_words, move |on, cx| write.update(cx, |v, cx| v.set_learn_words(on, cx))) };
        let predict = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.predict, move |on, cx| write.update(cx, |v, cx| v.set_predict(on, cx))) };
        let forget = custom_field(view, |v, view, _, _| Button::new("forget-words").label(if v.confirm_forget { "Click again to forget" } else { "Forget" }).danger()
            .on_click(click(view, |v, _, cx| v.ask_forget(cx))).into_any_element());
        let learned = match self.snap.words_learned { 0 => "No words learned yet.".to_string(), 1 => "1 word learned.".to_string(), n => format!("{n} words learned.") };
        page("Word suggestions", IconName::CaseSensitive, "The bubble can finish the word you are typing, in Thai or English. Tab accepts the highlighted word; hold Tab and press 1–3 to pick another. Space can also accept a word typed in the wrong layout.".into(),
            keywords(&["word suggestions", "completion", "complete", "autocomplete", "pause", "delay", "typing", "learn", "forget", "accept", "space", "tab", "shortcut", "key", "ctrl", "control", "1", "2", "3", "number", "digit", "pick", "arrow", "arrows", "next", "next word", "predict", "prediction", "model", "enter", "bubble", "layout", "horizontal", "vertical", "row", "column", "input", "language", "source", "keyboard", "flag", "badge", "caret", "preview", "idle", "stop", "once", "never", "off", "ไทย"], std::iter::empty()),
            [Some(SettingItem::new("Show suggestions", mode)),
                (self.snap.completion == CompleteMode::Pause).then(|| SettingItem::new("Pause before suggesting", delay).description("How long to stop typing before the bubble offers words. Shorter is quicker but may flash while you type.")),
                (self.snap.completion != CompleteMode::Off).then(|| SettingItem::new("Suggest the next word", next).description("Pause after a word (after a space in English) and the bubble offers the word that usually follows it: built-in word pairs, and your own model's guess with them.")),
                Some(SettingItem::new("Bubble layout", layout).description("Horizontal puts the suggestions side by side. Long suggestions and AI rewrite results are always stacked.")),
                Some(SettingItem::new("Accept with", accept).description("Space accepts a word typed in the wrong layout (l;ylfu → สวัสดี), then types the space. Over other suggestions it just types a space.")),
                Some(SettingItem::new("Show the input language", badge).description("A flag shows by the caret: when the keyboard switches between Thai and English, and while a text field is focused with nothing typed in it yet.")),
                self.snap.input_badge.then(|| SettingItem::new("Show it again after", badge_delay).description("How long a pause in typing brings the flag back while the same text field still has focus. Off: it shows once when focus lands in the field, and a pause never brings it back.")),
                Some(SettingItem::new("Pick with", pick).description("Arrow keys: while the bubble is up, Up moves into it, then Left or Right chooses and Enter or Up applies; Down or Esc leaves. Until Up the arrows move the cursor. Ctrl+1–3: Ctrl+1, Ctrl+2 or Ctrl+3 picks that suggestion; otherwise those keys reach the app.")),
                Some(SettingItem::new("Learn from my typing", learn).description("Words you type often come first. A new word is suggested after you type it three times.")),
                Some(SettingItem::new("Finish words from my typing", predict).description("A model of your own typing finishes the word you are typing, in Thai or English, offers words the built-in lists do not have, and adds its guess to the next-word rows. Learned from the lines you finish; nothing leaves this computer.")),
                Some(SettingItem::new("Forget learned words", forget).description(learned))].into_iter().flatten().collect())
    }
    fn updates_page(&self, view: &Entity<Self>) -> SettingPage {
        let auto = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.check_updates, move |on, cx| write.update(cx, |v, cx| v.set_check_updates(on, cx))) };
        // the same action as the tray item: check, restart into a downloaded update, or open the release page
        let now = custom_field(view, |_, _, _, _| { let (label, enabled) = update::status().menu();
            Button::new("update-now").label(label).disabled(!enabled).on_click(|_, _, _| bridge::REQ_UPDATE_CLICK.store(true, std::sync::atomic::Ordering::SeqCst)).into_any_element() });
        page("Updates", Icon::new(gpui_kit::assets::IconName::Download), format!("You have typeThai {}. Updates come from the typeThai releases on GitHub, are checked against typeThai's signing key, and install only when you choose Restart to update.", update::CURRENT),
            keywords(&["updates", "update", "version", "release", "upgrade", "github", "download", "restart"], std::iter::empty()),
            vec![SettingItem::new("Check automatically", auto).description("Look for a new version at launch and every 6 hours, and download it in the background. Only GitHub is contacted."),
                SettingItem::new("Update", now)])
    }
    fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.check_updates = on;
        bridge::push_settings(SettingsCmd::SetCheckUpdates(on));
        cx.notify();
    }
    fn set_learn_words(&mut self, on: bool, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.learn_words = on;
        bridge::push_settings(SettingsCmd::SetLearnWords(on));
        cx.notify();
    }
    fn set_predict(&mut self, on: bool, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.predict = on;
        bridge::push_settings(SettingsCmd::SetPredict(on));
        cx.notify();
    }
    fn ask_forget(&mut self, cx: &mut Context<Self>) {
        self.confirm_reset = false;
        if !self.confirm_forget { self.confirm_forget = true; cx.notify(); return; }
        self.confirm_forget = false;
        bridge::push_settings(SettingsCmd::ForgetWords);
        cx.notify();
    }
    fn set_accept(&mut self, accept: AcceptKeys, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.accept = accept;
        bridge::push_settings(SettingsCmd::SetAccept(accept));
        cx.notify();
    }
    fn set_bubble(&mut self, bubble: BubbleLayout, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.bubble = bubble;
        bridge::push_settings(SettingsCmd::SetBubbleLayout(bubble));
        cx.notify();
    }
    fn set_badge_idle(&mut self, idle: Option<std::time::Duration>, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.badge_idle = idle;
        bridge::push_settings(SettingsCmd::SetBadgeIdle(self.snap.badge_idle));
        cx.notify();
    }
    fn set_next_word(&mut self, on: bool, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.next_word = on;
        bridge::push_settings(SettingsCmd::SetNextWord(on));
        cx.notify();
    }
    fn set_input_badge(&mut self, on: bool, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.input_badge = on;
        bridge::push_settings(SettingsCmd::SetInputBadge(on));
        cx.notify();
    }
    fn set_pick(&mut self, (arrows, ctrl): (bool, bool), cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        (self.snap.pick_arrows, self.snap.pick_ctrl) = (arrows, ctrl);
        bridge::push_settings(SettingsCmd::SetPickArrows(arrows));
        bridge::push_settings(SettingsCmd::SetPickCtrl(ctrl));
        cx.notify();
    }
    fn set_tab_hold(&mut self, ms: u64, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.tab_hold = std::time::Duration::from_millis(ms);
        bridge::push_settings(SettingsCmd::SetTabHold(self.snap.tab_hold));
        cx.notify();
    }
    fn set_complete_idle(&mut self, ms: u64, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.complete_idle = std::time::Duration::from_millis(ms);
        bridge::push_settings(SettingsCmd::SetCompleteIdle(self.snap.complete_idle));
        cx.notify();
    }
    fn set_completion(&mut self, mode: CompleteMode, cx: &mut Context<Self>) {
        (self.confirm_reset, self.confirm_forget) = (false, false);
        self.snap.completion = mode;
        bridge::push_settings(SettingsCmd::SetCompletion(mode));
        cx.notify();
    }
    fn rewrite_page(&self, view: &Entity<Self>) -> SettingPage {
        let hotkey = if cfg!(windows) { "Ctrl+Alt+R" } else { "Ctrl+Option+R" };
        let hold = { let (read, write) = (view.clone(), view.clone());
            let options = TAB_HOLD_CHOICES_MS.iter().map(|&ms| (delay_label(ms), delay_label(ms))).collect();
            SettingField::dropdown(options,
                move |cx| delay_label(read.read(cx).snap.tab_hold.as_millis() as u64),
                move |label, cx| if let Some(&ms) = TAB_HOLD_CHOICES_MS.iter().find(|&&ms| delay_label(ms) == label) { write.update(cx, |v, cx| v.set_tab_hold(ms, cx)) }) };
        let key_note = match &self.key_state {
            KeyState::Checking => "Checking for a saved key…".to_string(),
            KeyState::Missing => "No key saved yet.".to_string(),
            KeyState::Saved => format!("Key saved in {KEY_STORE}. Save an empty key to remove it."),
            KeyState::Failed(e) => format!("Could not save the key: {e}"),
        };
        // The rows with their explanation behind an ⓘ: the switch, the model picker and the setup button each carry a
        // hover card instead of a paragraph under the row.
        let enabled = element(view, |v, view, _, paint| {
            let switch = Switch::new("rewrite-enabled").checked(v.snap.rewrite.enabled).xsmall()
                .on_click({ let view = view.clone(); move |on, _, cx| view.update(cx, |v, cx| v.set_rewrite_enabled(*on, cx)) });
            help_row("offer-help", "Offer AI rewrite", None, vec![
                "Finish sentence locally runs Ollama on this computer: no key, and only the selected text is sent to it.".into(),
                "Fix typos and Translate TH↔EN use the key and server below; the selected text goes there instead.".into(),
                "A preview is always shown first — generated facts can be wrong.".into(),
            ], paint, switch.into_any_element())
        });
        let local_pick = element(view, |v, view, options, paint| {
            let current = v.local_model.clone();
            let label = rewrite::LOCAL_MODELS.iter().find(|(tag, _)| *tag == current.as_ref())
                .map_or_else(|| current.to_string(), |(_, label)| (*label).to_string());
            let menu = Button::new("local-model").label(label).dropdown_caret(true).outline().with_size(options.size())
                .dropdown_menu_with_anchor(Anchor::TopRight, { let view = view.clone(); let current = current.clone();
                    move |menu, _, _| rewrite::LOCAL_MODELS.iter().fold(menu, |menu, (tag, label)| {
                        let view = view.clone();
                        menu.item(PopupMenuItem::new(*label).checked(*tag == current.as_ref())
                            .on_click({ let view = view.clone(); let tag = tag.to_string(); move |_, _, cx| view.update(cx, |v, cx| v.set_local_model(tag.clone(), cx)) }))
                    }) });
            help_row("local-model-help", "Local model", None, vec![
                "Gemma 4 8B is the measured default.".into(),
                "Typhoon 2.5 4B was about 1.7× faster with a smaller download and comparable output, on the same prompt.".into(),
                "For any other Ollama tag, set prefs.json → \"rewrite\" → \"local_model\".".into(),
            ], paint, menu.into_any_element())
        });
        let local_setup = element(view, |v, view, options, paint| {
            let (label, enabled, busy) = v.local_face();
            let button = Button::new("setup-local").label(label).outline().with_size(options.size()).loading(busy).disabled(!enabled)
                .on_click(click(view, |v, _, cx| v.setup_local(cx)));
            help_row("local-setup-help", "Local model setup", v.local_note().map(|e| rewrite::preview(&e, 90)), v.local_help(), paint, button.into_any_element())
        });
        let key = custom_field(view, |v, view, options, _| field_box(options).child(Input::new(&v.key)
            .suffix(Button::new("save-key").label("Save key").ghost().xsmall().on_click(click(view, |v, window, cx| v.save_key(window, cx))))).into_any_element());
        let instructions = custom_field(view, |v, _, options, _| field_box(options).child(Textarea::new(&v.instructions)).into_any_element());
        let save = element(view, |v, view, _, paint| div().flex().flex_row().items_center().justify_end().gap(px(12.))
            .children(error_text(paint, &v.rewrite_error).map(|e| e.flex_1()))
            .child(Button::new("save-rewrite").label("Save").primary().on_click(click(view, |v, window, cx| v.save_rewrite(window, cx))))
            .into_any_element());
        // Local or cloud first: the two halves of the feature are separate tabs, so neither page is a wall of rows.
        let tabs = element(view, |v, view, _, _| {
            let view = view.clone();
            div().child(TabBar::new("rewrite-tabs").pill()
                .child(Tab::new().label("Local"))
                .child(Tab::new().label("Cloud"))
                .selected_index(if v.rewrite_tab == RewriteTab::Cloud { 1 } else { 0 })
                .on_click(move |index, _, cx| view.update(cx, |v, cx| v.set_rewrite_tab(*index, cx))))
                .into_any_element()
        });
        let mut rows = vec![enabled, SettingItem::new("Hold Tab for", hold).description("How long to hold Tab to open these options.")];
        rows.push(tabs);
        match self.rewrite_tab {
            RewriteTab::Local => {
                rows.push(local_pick);
                rows.push(local_setup);
            }
            RewriteTab::Cloud => {
                rows.push(SettingItem::new("API key", key).description(key_note));
                rows.push(SettingItem::new("Model", draft_field(view, |v| v.model.clone(), |v, s| v.model = s)));
                rows.push(SettingItem::new("Base URL", draft_field(view, |v| v.base_url.clone(), |v, s| v.base_url = s)));
                rows.push(SettingItem::new("Instructions", instructions).layout(Axis::Vertical).description("Cloud prompts only."));
                rows.push(save);
            }
        }
        page("AI rewrite", IconName::Bot,
            format!("Hold Tab, or press {hotkey}, on a selection or the word before the caret: local fixes first, then the AI rows."),
            keywords(&["ai rewrite", "rewrite", "api key", "openrouter", "model", "base url", "translate", "typo", "hotkey", "prompt", "instructions", "selection", "selected", "hold", "tab", "layout", "spelling", "word", "finish sentence", "local", "cloud", "gemma", "ollama", "setup", "download", "typhoon", "4b", hotkey], std::iter::empty()),
            rows)
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let pages = [self.completion_page(&view), self.never_page(&view), self.learn_page(&view), self.rewrite_page(&view), self.updates_page(&view)];
        let t = cx.theme();
        div().size_full().bg(t.background).text_color(t.foreground).font(thai_font())
            .child(Settings::new("settings").sidebar_width(px(200.)).sidebar_size_range(px(160.)..px(280.))
                .default_selected_index(SelectIndex { page_ix: self.first_page, group_ix: None }).pages(pages))
    }
}

#[cfg(test)]
mod tests {
    // no `use super::*`: the glob would bring the kit's own `test` macro in and the attribute below would recurse
    use super::{demo_local, demo_states, local_face, LocalDemo, LocalState};
    use crate::local_setup::{self, Step};

    /// The button face is the state machine the user drives; every branch has to be reachable and honest.
    #[test] fn the_setup_button_says_what_pressing_it_does() {
        let face = local_face;
        assert_eq!(face(&LocalState::Unknown), ("Checking…".to_string(), false, true));
        assert_eq!(face(&LocalState::Step(Step::Install)), ("Get Ollama".to_string(), true, false));
        assert_eq!(face(&LocalState::Step(Step::Start)), ("Start Ollama".to_string(), true, false));
        assert_eq!(face(&LocalState::Step(Step::Pull)), ("Download model".to_string(), true, false));
        assert_eq!(face(&LocalState::Step(Step::Ready)), ("Ready".to_string(), false, false));
        assert_eq!(face(&LocalState::Done("gemma4:latest is ready".into())), ("Ready".to_string(), false, false));
        assert_eq!(face(&LocalState::Failed("ollama pull failed (exit status: 1)".into())), ("Try again".to_string(), true, false));
        assert_eq!(face(&LocalState::Working("Starting Ollama…".into())), ("Starting Ollama…".to_string(), false, true));
        assert_eq!(face(&LocalState::Working("pulling manifest".into())), ("Downloading…".to_string(), false, true));
        let progress = "pulling 6d0b3f1c2a44: 42% ▕█████░░░░░░░░░░░░░░▏ 4.0 GB/9.6 GB";
        assert_eq!(face(&LocalState::Working(progress.into())), ("Downloading 42%".to_string(), false, true));
    }

    /// `--local-state` names reach every state the cycle shows.
    #[test] fn the_demo_names_cover_the_states_and_the_cycle() {
        for name in ["checking", "install", "start", "pull", "downloading", "ready", "failed"] {
            assert!(demo_local(name).is_some(), "{name}");
        }
        assert!(matches!(demo_local("cycle"), Some(LocalDemo::Cycle(0))));
        assert!(matches!(demo_local("not-installed"), Some(LocalDemo::Fixed(LocalState::Step(Step::Install)))));
        assert!(demo_local("nonsense").is_none());
        assert_eq!(demo_states().len(), 10, "the cycle covers every branch the face handles");
    }
}
