//! Settings window: when word suggestions show, words that stay as typed, the on-device learning list, AI rewrite and
//! updates, one sidebar page each.
use crate::bridge::{self, BubbleLayout};
use crate::engine::{AcceptKeys, CompleteMode, COMPLETE_IDLE_CHOICES_MS, TAB_HOLD_CHOICES_MS};
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::{rewrite, update};
use crate::ui::thai_font;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::setting::{RenderOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{ActiveTheme, Disableable as _, Icon, IconName, Sizable};
use gpui_kit::*;
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
    first_page: usize,
}
enum KeyState { Checking, Missing, Saved, Failed(String) }
const COMPLETION: [(CompleteMode, &str); 3] = [(CompleteMode::Off, "Off"), (CompleteMode::Pause, "After a pause"), (CompleteMode::Typing, "While typing")];
const ACCEPT: [(AcceptKeys, &str); 2] = [(AcceptKeys::Tab, "Tab"), (AcceptKeys::TabOrSpace, "Tab or Space")];
const BUBBLE: [(BubbleLayout, &str); 2] = [(BubbleLayout::Horizontal, "Horizontal"), (BubbleLayout::Vertical, "Vertical")];
/// "Pick with": (pick_arrows, pick_ctrl). Both off (only from a hand-edited prefs.json) shows no choice.
const PICK: [((bool, bool), &str); 3] = [((true, false), "Arrow keys"), ((false, true), "Ctrl+1–3"), ((true, true), "Arrow keys and Ctrl+1–3")];
fn delay_label(ms: u64) -> SharedString { format!("{ms} ms").into() }
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
/// Redraw an open window: the updates page shows the updater's status.
pub fn refresh(slot: &Option<SettingsWin>, cx: &mut App) {
    if let Some(win) = slot { let _ = win.handle.update(cx, |_, _, cx| win.view.update(cx, |_, cx| cx.notify())); }
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
        input, snap: SettingsSnap::default(), error: None, loaded: false, confirm_reset: false, confirm_forget: false, key,
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
        let learn = { let (read, write) = (view.clone(), view.clone());
            SettingField::switch(move |cx| read.read(cx).snap.learn_words, move |on, cx| write.update(cx, |v, cx| v.set_learn_words(on, cx))) };
        let forget = custom_field(view, |v, view, _, _| Button::new("forget-words").label(if v.confirm_forget { "Click again to forget" } else { "Forget" }).danger()
            .on_click(click(view, |v, _, cx| v.ask_forget(cx))).into_any_element());
        let learned = match self.snap.words_learned { 0 => "No words learned yet.".to_string(), 1 => "1 word learned.".to_string(), n => format!("{n} words learned.") };
        page("Word suggestions", IconName::CaseSensitive, "The bubble can finish the word you are typing, in Thai or English. Tab accepts the highlighted word; hold Tab and press 1–3 to pick another. Space can also accept a word typed in the wrong layout.".into(),
            keywords(&["word suggestions", "completion", "complete", "autocomplete", "pause", "delay", "typing", "learn", "forget", "accept", "space", "tab", "shortcut", "key", "ctrl", "control", "1", "2", "3", "number", "digit", "pick", "arrow", "arrows", "enter", "bubble", "layout", "horizontal", "vertical", "row", "column", "input", "language", "source", "keyboard", "flag", "badge", "caret", "ไทย"], std::iter::empty()),
            [Some(SettingItem::new("Show suggestions", mode)),
                (self.snap.completion == CompleteMode::Pause).then(|| SettingItem::new("Pause before suggesting", delay).description("How long to stop typing before the bubble offers words. Shorter is quicker but may flash while you type.")),
                Some(SettingItem::new("Bubble layout", layout).description("Horizontal puts the suggestions side by side. Long suggestions and AI rewrite results are always stacked.")),
                Some(SettingItem::new("Accept with", accept).description("Space accepts a word typed in the wrong layout (l;ylfu → สวัสดี), then types the space. Over other suggestions it just types a space.")),
                Some(SettingItem::new("Show the input language", badge).description("When the keyboard switches between Thai and English, a flag shows by the caret for a moment.")),
                Some(SettingItem::new("Pick with", pick).description("Arrow keys: while the bubble is up, Left or Right chooses and Enter or Up applies; Down or Esc leaves. With no bubble the arrows move the cursor. Ctrl+1–3: Ctrl+1, Ctrl+2 or Ctrl+3 picks that suggestion; otherwise those keys reach the app.")),
                Some(SettingItem::new("Learn from my typing", learn).description("Words you type often come first. A new word is suggested after you type it three times.")),
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
            format!("Select text and hold Tab, or press {hotkey}: the bubble offers it in the other layout and with typos fixed, worked out on this computer. With AI rewrite on and a key saved, Fix typos and Translate TH↔EN follow; only those send the text to the service below. With nothing selected, it offers fixes for the word before the caret."),
            keywords(&["ai rewrite", "rewrite", "api key", "openrouter", "model", "base url", "translate", "typo", "hotkey", "prompt", "instructions", "selection", "selected", "hold", "tab", "layout", "spelling", "word", hotkey], std::iter::empty()),
            vec![SettingItem::new("Offer AI rewrite", enabled),
                SettingItem::new("Hold Tab for", hold).description("How long to hold Tab to open these options. Shorter is quicker; longer leaves more room for a slow Tab."),
                SettingItem::new("API key", key).description(key_note),
                SettingItem::new("Model", draft_field(view, |v| v.model.clone(), |v, s| v.model = s)),
                SettingItem::new("Base URL", draft_field(view, |v| v.base_url.clone(), |v, s| v.base_url = s)),
                SettingItem::new("Instructions", instructions).layout(Axis::Vertical).description("Added to both Fix typos and Translate."), save])
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
