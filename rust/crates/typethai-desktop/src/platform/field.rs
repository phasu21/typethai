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
    /// Keys go to the engine: a caret (or a known text app) always opens; unknown opens unless fullscreen.
    pub fn open(&self, caret: bool, fullscreen: impl FnOnce() -> bool) -> bool { caret || match self.field { Some(Field::Text) => true, Some(Field::NotText) => false, _ => !fullscreen() } }
    /// Classify synchronously before this key? Open: at each word start (shared with the password check). Closed:
    /// only after a trigger nothing has answered yet, so a closed gate never queries per key.
    pub fn must_ask(&self, word_start: bool, open: bool) -> bool { self.field.is_none() || if open { word_start } else { self.touched.is_some() && !self.inflight } }
    /// Closed: send an off-thread re-check (marks it in flight). From the timer (`key` false) after a settled trigger;
    /// from a key also when the verdict is older than `REVERIFY`.
    pub fn refresh_due(&mut self, now: Instant, key: bool) -> bool {
        if self.inflight || !matches!(self.field, Some(Field::NotText | Field::Unknown)) { return false }
        let due = self.touched.is_some_and(|t| now.duration_since(t) >= SETTLE) || (key && self.at.is_some_and(|t| now.duration_since(t) >= REVERIFY));
        if due { self.inflight = true; self.touched = None; } due
    }
}

/// UIA ControlType ids (UIA_*ControlTypeId).
pub const UIA_EDIT: i32 = 50004; pub const UIA_DOCUMENT: i32 = 50030; pub const UIA_COMBOBOX: i32 = 50003;
/// UIA focused element: Edit / Document unless read-only, editable ComboBox, else TextPattern (terminals) or a
/// writable ValuePattern. Chromium's page itself is a read-only Document, its inputs are Edit.
pub fn uia(control_type: i32, text: bool, value: bool, read_only: bool) -> Field {
    let writable = !(value && read_only);
    let t = match control_type { UIA_EDIT | UIA_DOCUMENT => writable, UIA_COMBOBOX => value && !read_only, _ => text || (value && !read_only) };
    if t { Field::Text } else { Field::NotText }
}
/// AX focused element (`role` None: nothing answered). No focused element in an app that has a focused window
/// (games, canvases) is not text, unless the app builds its tree on request (`lazy`: Electron, Chromium), where a
/// text view often reports no focused element at all.
pub fn ax(role: Option<&str>, has_range: bool, has_window: bool, lazy: bool) -> Field {
    match role {
        Some("AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField") => Field::Text,
        Some(_) if has_range => Field::Text,
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
    #[test] fn touch_while_closed_asks_until_answered() {
        let t = Instant::now(); let mut g = closed(t); g.touch(t);
        assert!(g.must_ask(true, false));
        g.answer(Some(Field::Text), ms(t, 5)); assert!(g.open(false, || true)); assert!(!g.must_ask(false, true));
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

    #[test] fn uia_rules() {
        assert_eq!(uia(UIA_EDIT, true, true, false), Field::Text);
        assert_eq!(uia(UIA_EDIT, false, false, false), Field::Text);           // Win32 edit without patterns
        assert_eq!(uia(UIA_EDIT, true, true, true), Field::NotText);           // read-only box
        assert_eq!(uia(UIA_DOCUMENT, true, false, false), Field::Text);        // Word / RichEdit
        assert_eq!(uia(UIA_DOCUMENT, true, true, true), Field::NotText);       // Chromium page body
        assert_eq!(uia(UIA_COMBOBOX, false, true, false), Field::Text);        // editable combo
        assert_eq!(uia(UIA_COMBOBOX, false, false, false), Field::NotText);    // drop-down list
        assert_eq!(uia(50020, true, false, false), Field::Text);               // Text + TextPattern: Windows Terminal
        assert_eq!(uia(50033, false, true, false), Field::Text);               // Pane with writable value
        assert_eq!(uia(50033, false, false, false), Field::NotText);           // game window (Pane)
        assert_eq!(uia(50032, false, false, false), Field::NotText);           // Window
        assert_eq!(uia(50000, false, false, false), Field::NotText);           // Button
    }
    #[test] fn ax_rules() {
        for r in ["AXTextField", "AXTextArea", "AXComboBox", "AXSearchField"] { assert_eq!(ax(Some(r), false, true, false), Field::Text, "{r}"); }
        assert_eq!(ax(Some("AXGroup"), true, true, false), Field::Text);              // custom text view with a selection range
        assert_eq!(ax(Some("AXWindow"), false, true, false), Field::NotText);         // game: focus rests on the window
        assert_eq!(ax(Some("AXOutline"), false, true, false), Field::NotText);        // Finder list type-select
        assert_eq!(ax(Some("AXWebArea"), false, true, false), Field::NotText);        // web page, no field focused
        assert_eq!(ax(None, false, true, false), Field::NotText);                     // native app answers, nothing focused
        assert_eq!(ax(None, false, false, false), Field::Unknown);                    // app does not answer AX
        // Electron / Chromium build their tree on request and often report no focused element in a text view
        // (Orca's terminal): not knowing is not "not text"
        assert_eq!(ax(None, false, true, true), Field::Unknown);
        assert_eq!(ax(Some("AXWindow"), false, true, true), Field::NotText, "an element that answers still decides");
    }
    #[test] fn known_apps() {
        assert!(known_text_app("io.alacritty") && known_text_app("net.kovidgoyal.kitty") && !known_text_app("com.valvesoftware.steam"));
        assert!(known_text_class("SunAwtFrame") && !known_text_class("UnrealWindow"));
        assert!(known_text_exe("Alacritty.exe") && !known_text_exe("FortniteClient-Win64-Shipping.exe") && !known_text_exe(""));
    }
}
