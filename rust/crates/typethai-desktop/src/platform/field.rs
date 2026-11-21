//! Text-entry gate: keys reach the engine only while focus is in a text control, so game keys (WASD) and list
//! type-select never bring up the rail. The platform classifies the focused element (accessibility, cross-process),
//! this caches the verdict per focus so a closed gate costs no query per key.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
use std::time::{Duration, Instant};

/// What accessibility says about the focused element.
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Field { Text, NotText, Unknown }

/// After a click / Tab / Enter / Esc / Nav key, how long the app gets to move focus before an off-thread re-check.
pub const SETTLE: Duration = Duration::from_millis(40);
/// A closed verdict is re-checked (off-thread, on a key) at most this often, in case focus moved without a trigger.
pub const REVERIFY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Gate { field: Option<Field>, at: Option<Instant>, touched: Option<Instant>, inflight: bool }
impl Gate {
    /// Just told `f` (the badge's one-off look at focus, outside the per-key cache).
    pub fn told(f: Field) -> Gate { Gate { field: Some(f), ..Gate::default() } }
    /// Focus moved to another window / app: nothing known.
    pub fn reset(&mut self) { *self = Gate::default() }
    /// Click, Tab, Enter, Esc or a Nav key: focus may have moved inside the same window.
    pub fn touch(&mut self, now: Instant) { self.touched = Some(now) }
    /// A classification arrived (`None`: the query was lost / timed out, keep the old verdict).
    pub fn answer(&mut self, f: Option<Field>, now: Instant) { self.inflight = false; if let Some(f) = f { self.field = Some(f); self.at = Some(now); self.touched = None; } }
    /// Accessibility said the focused element is a text field (not just "unknown, but not fullscreen"): the bar for
    /// changing text with no key pressed ("Auto-convert when sure").
    pub fn text(&self) -> bool { self.field == Some(Field::Text) }
    /// Keys go to the engine: a caret (or a known text app) always opens; unknown opens unless fullscreen.
    pub fn open(&self, caret: bool, fullscreen: impl FnOnce() -> bool) -> bool { caret || match self.field { Some(Field::Text) => true, Some(Field::NotText) => false, _ => !fullscreen() } }
    /// Classify synchronously before this key? Unknown focus, or an open gate at each word start (shared with the
    /// password check). Never while closed: the hooks share one thread, and a key waiting on a query there holds up
    /// every mouse move (a game, clicking and pressing keys); a trigger is re-checked off-thread (`refresh_due`).
    pub fn must_ask(&self, word_start: bool, open: bool) -> bool { self.field.is_none() || (open && word_start) }
    /// Closed: send an off-thread re-check (marks it in flight). From the timer (`key` false) after a settled trigger;
    /// from a key also when the verdict is older than `REVERIFY`.
    pub fn refresh_due(&mut self, now: Instant, key: bool) -> bool {
        if self.inflight || !matches!(self.field, Some(Field::NotText | Field::Unknown)) { return false }
        let due = self.touched.is_some_and(|t| now.duration_since(t) >= SETTLE) || (key && self.at.is_some_and(|t| now.duration_since(t) >= REVERIFY));
        if due { self.inflight = true; self.touched = None; } due
    }
}

/// While the bubble is up with no key or click since (a key or a click says where focus is), how long before focus is
/// looked at off-thread: an app can move focus on its own (a window closes, a sheet opens, a send button takes it), and
/// the bubble must not be left over a field the user has gone from.
pub const WATCH: Duration = Duration::from_millis(750);

/// The bubble's focus watch: one off-thread look per `WATCH` of quiet while it is up, none while it is down or the
/// user is typing (each key restarts the wait, and drops a look in flight: its answer predates the key).
#[derive(Default)]
pub struct Watch { since: Option<Instant>, inflight: bool }
impl Watch {
    /// A key or a click: focus is where the user just was.
    pub fn input(&mut self, now: Instant) { self.since = Some(now); self.inflight = false; }
    /// Timer: send a look now? (marks it in flight). `up`: the bubble is showing.
    pub fn due(&mut self, now: Instant, up: bool) -> bool {
        if !up { *self = Watch::default(); return false }
        if self.inflight { return false }
        let since = *self.since.get_or_insert(now);
        self.inflight = now.duration_since(since) >= WATCH; self.inflight
    }
    /// The look answered (or was lost): the next one is `WATCH` from now.
    pub fn answered(&mut self, now: Instant) { self.since = Some(now); self.inflight = false; }
    /// A look is in flight and its answer still counts.
    pub fn waiting(&self) -> bool { self.inflight }
}

/// UIA ControlType ids (UIA_*ControlTypeId).
pub const UIA_EDIT: i32 = 50004; pub const UIA_DOCUMENT: i32 = 50030; pub const UIA_COMBOBOX: i32 = 50003;
/// UIA focused element: an Edit is writable unless ValuePattern says it's read-only (an Edit with neither pattern is
/// still a real edit control, so that defaults open); a Document needs the same proof when it has a ValuePattern
/// (Chromium's page body), else a real insertion caret (Word / RichEdit) — bare TextPattern is not enough, since
/// read-only selectable text (a web page's own text, a PDF viewer, a document in read mode) exposes it too, only
/// without a caret of its own; same for any other control type (Windows Terminal's prompt has one, static text does
/// not). `caret`: TextPattern2's `GetCaretRange` answered with an active range, not merely a selection.
pub fn uia(control_type: i32, caret: bool, value: bool, read_only: bool) -> Field {
    let writable = !(value && read_only);
    let t = match control_type {
        UIA_EDIT => writable,
        UIA_DOCUMENT => if value { writable } else { caret },
        UIA_COMBOBOX => value && !read_only,
        _ => (value && writable) || caret,
    };
    if t { Field::Text } else { Field::NotText }
}
/// AX roles that are text whatever else they report: a combo box's text part and a search field are never read-only.
/// AXTextField / AXTextArea are not here: both also cover read-only text (a disabled field, a display-only field
/// styled as one, Preview's PDF text layer, Console.app's log) and fall to the `editable` check below like any other
/// role with a selection range.
pub const TEXT_ROLES: [&str; 2] = ["AXComboBox", "AXSearchField"];
/// AX focused element (`role` None: nothing answered). Another role is text when it has a selection range and the
/// range or the value can be set (`editable`): a page in Chromium / Electron and the Finder desktop report a range they
/// cannot edit. No focused element in an app that has a focused window (games, canvases) is not text, unless the app
/// builds its tree on request (`lazy`: Electron, Chromium), where a text view often reports no focused element at all.
pub fn ax(role: Option<&str>, has_range: bool, editable: bool, has_window: bool, lazy: bool) -> Field {
    match role {
        Some(r) if TEXT_ROLES.contains(&r) => Field::Text,
        Some(_) if has_range && editable => Field::Text,
        Some(_) => Field::NotText,
        None => if has_window && !lazy { Field::NotText } else { Field::Unknown },
    }
}
/// Apps that take text without exposing a caret or an editable element (GPU terminals).
pub fn known_text_app(bundle: &str) -> bool {
    ["io.alacritty", "org.alacritty", "net.kovidgoyal.kitty", "com.github.wez.wezterm", "com.mitchellh.ghostty"].iter().any(|b| bundle.starts_with(b))
}
/// Windows top-level classes of the same kind (Java/Swing: JetBrains IDEs; WezTerm).
pub fn known_text_class(class: &str) -> bool { ["SunAwtFrame", "SunAwtDialog", "org.wezfurlong.wezterm"].contains(&class) }
/// Windows executables of the same kind (file name, any case).
pub fn known_text_exe(exe: &str) -> bool { ["alacritty.exe", "wezterm-gui.exe"].iter().any(|e| exe.eq_ignore_ascii_case(e)) }

#[cfg(test)]
mod tests {
    use super::*;
    fn ms(t: Instant, n: u64) -> Instant { t + Duration::from_millis(n) }
    fn closed(t: Instant) -> Gate { let mut g = Gate::default(); g.answer(Some(Field::NotText), t); g }

    #[test] fn a_fresh_verdict_opens_as_the_cached_one_would() {
        assert!(Gate::told(Field::Text).open(false, || true));
        assert!(!Gate::told(Field::NotText).open(false, || false));
        assert!(Gate::told(Field::NotText).open(true, || false), "a caret opens");
        assert!(Gate::told(Field::Unknown).open(false, || false) && !Gate::told(Field::Unknown).open(false, || true), "unknown: open unless fullscreen");
    }
    #[test] fn unknown_focus_asks_on_any_key() {
        let g = Gate::default();
        assert!(g.must_ask(false, true) && g.must_ask(true, false));
    }
    #[test] fn caret_opens_whatever_accessibility_says() {
        assert!(closed(Instant::now()).open(true, || true));
    }
    #[test] fn verdicts_open_and_close() {
        let t = Instant::now(); let mut g = Gate::default();
        g.answer(Some(Field::Text), t); assert!(g.open(false, || true));
        g.answer(Some(Field::NotText), t); assert!(!g.open(false, || false));
    }
    #[test] fn unknown_opens_unless_fullscreen() {
        let t = Instant::now(); let mut g = Gate::default(); g.answer(Some(Field::Unknown), t);
        assert!(g.open(false, || false)); assert!(!g.open(false, || true));
    }
    #[test] fn lost_reply_keeps_verdict() {
        let t = Instant::now(); let mut g = closed(t); g.answer(None, t); assert!(!g.open(false, || false));
    }
    #[test] fn closed_gate_never_asks_per_key() {
        // WASD in a game: every key is a "word start" (the engine is reset), none may cost a query
        let g = closed(Instant::now());
        for _ in 0..50 { assert!(!g.must_ask(true, false)); }
    }
    #[test] fn open_gate_asks_at_word_start_only() {
        let t = Instant::now(); let mut g = Gate::default(); g.answer(Some(Field::Text), t);
        assert!(g.must_ask(true, true)); assert!(!g.must_ask(false, true));
    }
    #[test] fn touch_while_closed_rechecks_off_thread_only() {
        // a click then a key in a game: no query on the key, the timer sends one after SETTLE
        let t = Instant::now(); let mut g = closed(t); g.touch(t);
        assert!(!g.must_ask(true, false)); assert!(!g.refresh_due(ms(t, 10), true));
        assert!(g.refresh_due(ms(t, 50), false));
        g.answer(Some(Field::Text), ms(t, 60)); assert!(g.open(false, || true)); assert!(!g.must_ask(false, true));
    }
    #[test] fn timer_refreshes_after_settle_once() {
        let t = Instant::now(); let mut g = closed(t); g.touch(t);
        assert!(!g.refresh_due(ms(t, 10), false));
        assert!(g.refresh_due(ms(t, 50), false));
        assert!(!g.refresh_due(ms(t, 100), false), "in flight");
        assert!(!g.must_ask(true, false), "the in-flight reply answers, not a second query");
        g.answer(None, ms(t, 120)); assert!(!g.refresh_due(ms(t, 200), false), "trigger consumed");
    }
    #[test] fn stale_closed_verdict_refreshes_on_keys_not_timer() {
        let t = Instant::now(); let mut g = closed(t); let later = t + REVERIFY + Duration::from_millis(1);
        assert!(!g.refresh_due(later, false)); assert!(g.refresh_due(later, true));
        g.answer(Some(Field::NotText), later); assert!(!g.refresh_due(ms(later, 10), true));
    }
    #[test] fn open_gate_needs_no_refresh() {
        let t = Instant::now(); let mut g = Gate::default(); g.answer(Some(Field::Text), t); g.touch(t);
        assert!(!g.refresh_due(t + REVERIFY * 2, true));
    }
    #[test] fn reset_forgets() {
        let t = Instant::now(); let mut g = closed(t); g.touch(t); assert!(g.refresh_due(ms(t, 50), false));
        g.reset(); assert!(g.must_ask(false, true)); assert!(!g.refresh_due(ms(t, 100), false));
    }

    #[test] fn watch_looks_after_a_quiet_spell_with_the_bubble_up() {
        let t = Instant::now(); let mut w = Watch::default();
        assert!(!w.due(t, true), "the wait starts when the bubble is first seen up");
        assert!(!w.due(t + WATCH / 2, true));
        assert!(w.due(t + WATCH, true)); assert!(w.waiting());
        assert!(!w.due(t + WATCH * 3, true), "one look in flight at a time");
        w.answered(t + WATCH * 3); assert!(!w.waiting());
        assert!(!w.due(t + WATCH * 3, true)); assert!(w.due(t + WATCH * 4, true), "and again a quiet spell later");
    }
    #[test] fn watch_waits_while_typing_and_drops_a_stale_look() {
        let t = Instant::now(); let mut w = Watch::default();
        w.input(t); assert!(w.due(t + WATCH, true));
        w.input(t + WATCH); assert!(!w.waiting(), "a key: the answer in flight predates it");
        for i in 1..20 { w.input(ms(t, 100 * i)); assert!(!w.due(ms(t, 100 * i + 50), true), "typing proves focus"); }
    }
    #[test] fn watch_sleeps_while_the_bubble_is_down() {
        let t = Instant::now(); let mut w = Watch::default();
        w.input(t); assert!(!w.due(t + WATCH * 10, false));
        assert!(!w.due(t + WATCH * 10, true), "the bubble just came up: wait from here");
        assert!(w.due(t + WATCH * 11, true));
        assert!(!w.due(t + WATCH * 12, false) && !w.waiting(), "down: the look in flight no longer counts");
    }

    #[test] fn uia_rules() {
        assert_eq!(uia(UIA_EDIT, true, true, false), Field::Text);
        assert_eq!(uia(UIA_EDIT, false, false, false), Field::Text);           // Win32 edit without patterns
        assert_eq!(uia(UIA_EDIT, false, true, true), Field::NotText);          // read-only box
        assert_eq!(uia(UIA_DOCUMENT, true, false, false), Field::Text);        // Word / RichEdit: no ValuePattern, a real caret
        assert_eq!(uia(UIA_DOCUMENT, false, false, false), Field::NotText);    // a read-only Document with no ValuePattern either: a PDF viewer, a document in read mode
        assert_eq!(uia(UIA_DOCUMENT, false, true, true), Field::NotText);      // Chromium page body
        assert_eq!(uia(UIA_DOCUMENT, true, true, true), Field::NotText);       // Chromium page body, caret browsing on: ValuePattern still says read-only
        assert_eq!(uia(UIA_COMBOBOX, false, true, false), Field::Text);        // editable combo
        assert_eq!(uia(UIA_COMBOBOX, false, false, false), Field::NotText);    // drop-down list
        assert_eq!(uia(50020, true, false, false), Field::Text);               // Text + a real caret: Windows Terminal's prompt
        assert_eq!(uia(50020, false, false, false), Field::NotText);           // Text, TextPattern, no caret: a web page's own selectable text, a label
        assert_eq!(uia(50033, false, true, false), Field::Text);               // Pane with writable value
        assert_eq!(uia(50033, false, false, false), Field::NotText);           // game window (Pane)
        assert_eq!(uia(50032, false, false, false), Field::NotText);           // Window
        assert_eq!(uia(50000, false, false, false), Field::NotText);           // Button
    }
    #[test] fn ax_rules() {
        for r in TEXT_ROLES { assert_eq!(ax(Some(r), false, false, true, false), Field::Text, "{r}"); }
        // AXTextField / AXTextArea are not fast-pathed: a disabled field, a display-only field styled as one, or a
        // read-only text view (Preview's PDF text layer, Console.app's log) reports the role but isn't editable
        for r in ["AXTextField", "AXTextArea"] {
            assert_eq!(ax(Some(r), true, true, true, false), Field::Text, "{r} editable");
            assert_eq!(ax(Some(r), true, false, true, false), Field::NotText, "{r} read-only");
            assert_eq!(ax(Some(r), false, false, true, false), Field::NotText, "{r} no selection range");
        }
        assert_eq!(ax(Some("AXGroup"), true, true, true, false), Field::Text);        // custom text view, its selection settable
        // the selection range alone is no field: Chromium / Electron pages expose one on the page itself, focused on a
        // settings page with no field (Hermes Agent), and so does the Finder desktop
        assert_eq!(ax(Some("AXWebArea"), true, false, true, true), Field::NotText);
        assert_eq!(ax(Some("AXGroup"), true, false, true, false), Field::NotText);
        assert_eq!(ax(Some("AXSlider"), false, true, true, false), Field::NotText);  // a settable value without text
        assert_eq!(ax(Some("AXWindow"), false, false, true, false), Field::NotText);  // game: focus rests on the window
        assert_eq!(ax(Some("AXOutline"), false, false, true, false), Field::NotText); // Finder list type-select
        assert_eq!(ax(None, false, false, true, false), Field::NotText);              // native app answers, nothing focused
        assert_eq!(ax(None, false, false, false, false), Field::Unknown);             // app does not answer AX
        // Electron / Chromium build their tree on request and often report no focused element in a text view
        // (Orca's terminal): not knowing is not "not text"
        assert_eq!(ax(None, false, false, true, true), Field::Unknown);
        assert_eq!(ax(Some("AXWindow"), false, false, true, true), Field::NotText, "an element that answers still decides");
    }
    #[test] fn known_apps() {
        assert!(known_text_app("io.alacritty") && known_text_app("net.kovidgoyal.kitty") && !known_text_app("com.valvesoftware.steam"));
        assert!(known_text_class("SunAwtFrame") && !known_text_class("UnrealWindow"));
        assert!(known_text_exe("Alacritty.exe") && !known_text_exe("FortniteClient-Win64-Shipping.exe") && !known_text_exe(""));
    }
}
