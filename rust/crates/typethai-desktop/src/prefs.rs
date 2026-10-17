//! User preferences: words typeThai should not translate, and when word completions show.
//! Stored beside `personal.json` so a learning reset does not clear them, and so `personal.json`
//! stays compatible with the TypeScript app.
use crate::bridge::BubbleLayout;
use crate::engine::{AcceptKeys, Action, CompleteMode, BADGE_IDLE_CHOICES_MS, BADGE_IDLE_DEFAULT_MS, COMPLETE_IDLE_CHOICES_MS, COMPLETE_IDLE_DEFAULT_MS, TAB_HOLD_CHOICES_MS, TAB_HOLD_DEFAULT_MS};
use crate::rewrite;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;
use typethai_core::keymap::th_to_en;
use typethai_core::personal::Learned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsCmd {
    AddNever(String),
    RemoveNever(String),
    Forget(String),
    ResetLearning,
    SetCompletion(CompleteMode),
    SetCompleteIdle(Duration),
    /// How long a held Tab takes to open the ⌃⌥R rows.
    SetTabHold(Duration),
    /// How long a typing pause brings the input-language preview back (`Options::badge_idle`).
    SetBadgeIdle(Duration),
    SetLearnWords(bool),
    /// Contextual word prediction from the personal word n-gram (`Options::predict`).
    SetPredict(bool),
    SetAccept(AcceptKeys),
    SetPickCtrl(bool),
    /// Arrow keys pick from the bubble (`Options::pick_arrows`).
    SetPickArrows(bool),
    SetInputBadge(bool),
    SetBubbleLayout(BubbleLayout),
    ForgetWords,
    /// Never carries the API key; that goes from the Settings window straight to the keyring.
    SetRewrite(rewrite::Config),
    SetCheckUpdates(bool),
}
impl SettingsCmd {
    pub fn writes_prefs(&self) -> bool {
        matches!(self, SettingsCmd::AddNever(_) | SettingsCmd::RemoveNever(_) | SettingsCmd::SetCompletion(_) | SettingsCmd::SetCompleteIdle(_) | SettingsCmd::SetTabHold(_) | SettingsCmd::SetBadgeIdle(_) | SettingsCmd::SetLearnWords(_) | SettingsCmd::SetPredict(_) | SettingsCmd::SetAccept(_) | SettingsCmd::SetPickCtrl(_) | SettingsCmd::SetPickArrows(_) | SettingsCmd::SetInputBadge(_) | SettingsCmd::SetBubbleLayout(_) | SettingsCmd::SetRewrite(_) | SettingsCmd::SetCheckUpdates(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsSnap {
    pub never: Vec<String>,
    pub learned: Vec<Learned>,
    pub completion: CompleteMode,
    /// How long a pause brings up completions in `CompleteMode::Pause`.
    pub complete_idle: Duration,
    pub tab_hold: Duration,
    pub learn_words: bool,
    /// Contextual word prediction from the personal word n-gram.
    pub predict: bool,
    pub accept: AcceptKeys,
    pub pick_ctrl: bool,
    pub pick_arrows: bool,
    pub input_badge: bool,
    /// How long a typing pause brings the input-language preview back.
    pub badge_idle: Duration,
    pub bubble: BubbleLayout,
    /// Distinct words in the word memory.
    pub words_learned: usize,
    pub rewrite: rewrite::Config,
    pub check_updates: bool,
}

/// One English key token: trimmed, lowercased, no whitespace.
/// Thai Kedmanee text is mapped back to those keys, so `นา` and `OK` both become `ok`.
pub fn normalize_word(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) { return None }
    let mapped = if has_thai(trimmed) { th_to_en(trimmed)? } else { trimmed.to_string() };
    let word = mapped.to_lowercase();
    if word.is_empty() || word.chars().any(char::is_whitespace) { None } else { Some(word) }
}
fn has_thai(s: &str) -> bool { s.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)) }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs { pub never: BTreeSet<String>, pub completion: CompleteMode, pub complete_idle: Duration, pub tab_hold: Duration, pub learn_words: bool, pub predict: bool, pub accept: AcceptKeys, pub pick_ctrl: bool, pub pick_arrows: bool, pub input_badge: bool, pub badge_idle: Duration, pub bubble: BubbleLayout, pub rewrite: rewrite::Config, pub check_updates: bool }
impl Default for Prefs {
    fn default() -> Self { Prefs { never: BTreeSet::new(), completion: CompleteMode::default(), complete_idle: Duration::from_millis(COMPLETE_IDLE_DEFAULT_MS), tab_hold: Duration::from_millis(TAB_HOLD_DEFAULT_MS), learn_words: true, predict: true, accept: AcceptKeys::Tab, pick_ctrl: false, pick_arrows: true, input_badge: true, badge_idle: Duration::from_millis(BADGE_IDLE_DEFAULT_MS), bubble: BubbleLayout::default(), rewrite: rewrite::Config::default(), check_updates: true } }
}
/// `prefs.json` spelling of each completion mode.
const COMPLETION: [(CompleteMode, &str); 3] = [(CompleteMode::Off, "off"), (CompleteMode::Pause, "pause"), (CompleteMode::Typing, "typing")];
/// `prefs.json` spelling of the accept keys.
const ACCEPT: [(AcceptKeys, &str); 2] = [(AcceptKeys::Tab, "tab"), (AcceptKeys::TabOrSpace, "tab_or_space")];
/// `prefs.json` spelling of the bubble layout.
const BUBBLE: [(BubbleLayout, &str); 2] = [(BubbleLayout::Horizontal, "horizontal"), (BubbleLayout::Vertical, "vertical")];
impl Prefs {
    pub fn load(path: &Path) -> Prefs {
        let Ok(text) = std::fs::read_to_string(path) else { return Prefs::default() };
        match parse(&text) {
            Some(p) => p,
            None => {
                eprintln!("typeThai: prefs unreadable ({}); starting with an empty list", path.display());
                Prefs::default()
            }
        }
    }
    pub fn to_json(&self) -> String {
        let completion = COMPLETION.iter().find(|(m, _)| *m == self.completion).map_or("pause", |(_, s)| s);
        let accept = ACCEPT.iter().find(|(a, _)| *a == self.accept).map_or("tab", |(_, s)| s);
        let bubble = BUBBLE.iter().find(|(b, _)| *b == self.bubble).map_or("horizontal", |(_, s)| s);
        serde_json::json!({ "never": self.never.iter().collect::<Vec<_>>(), "completion": completion, "complete_idle_ms": self.complete_idle.as_millis() as u64, "tab_hold_ms": self.tab_hold.as_millis() as u64, "learn_words": self.learn_words, "predict": self.predict, "accept_with": accept, "pick_with_ctrl": self.pick_ctrl, "pick_with_arrows": self.pick_arrows, "input_badge": self.input_badge, "badge_idle_ms": self.badge_idle.as_millis() as u64, "bubble_layout": bubble, "rewrite": self.rewrite.to_json(), "check_updates": self.check_updates }).to_string()
    }
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.to_json()).is_ok() { let _ = std::fs::rename(&tmp, path); }
    }
}
/// None when the file is not JSON or `never` is not a list; an unknown or missing `completion`, `complete_idle_ms`,
/// `learn_words`, `accept_with`, `pick_with_ctrl`, `input_badge`, `badge_idle_ms`, `bubble_layout` or `check_updates`
/// is the default.
fn parse(text: &str) -> Option<Prefs> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let never = match v.get("never") {
        Some(n) => n.as_array()?.iter().filter_map(|x| x.as_str()).filter_map(normalize_word).collect(),
        None => BTreeSet::new(),
    };
    let c = v.get("completion").and_then(|c| c.as_str());
    let completion = COMPLETION.iter().find(|(_, s)| Some(*s) == c).map_or(CompleteMode::default(), |(m, _)| *m);
    let tab_hold = v.get("tab_hold_ms").and_then(serde_json::Value::as_u64).filter(|ms| TAB_HOLD_CHOICES_MS.contains(ms)).unwrap_or(TAB_HOLD_DEFAULT_MS);
    let complete_idle = v.get("complete_idle_ms").and_then(serde_json::Value::as_u64).filter(|ms| COMPLETE_IDLE_CHOICES_MS.contains(ms)).unwrap_or(COMPLETE_IDLE_DEFAULT_MS);
    let learn_words = v.get("learn_words").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let predict = v.get("predict").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let a = v.get("accept_with").and_then(|a| a.as_str());
    let accept = ACCEPT.iter().find(|(_, s)| Some(*s) == a).map_or(AcceptKeys::default(), |(k, _)| *k);
    let pick_ctrl = v.get("pick_with_ctrl").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let pick_arrows = v.get("pick_with_arrows").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let input_badge = v.get("input_badge").and_then(serde_json::Value::as_bool).unwrap_or(true);
    let badge_idle = v.get("badge_idle_ms").and_then(serde_json::Value::as_u64).filter(|ms| BADGE_IDLE_CHOICES_MS.contains(ms)).unwrap_or(BADGE_IDLE_DEFAULT_MS);
    let b = v.get("bubble_layout").and_then(|b| b.as_str());
    let bubble = BUBBLE.iter().find(|(_, s)| Some(*s) == b).map_or(BubbleLayout::default(), |(l, _)| *l);
    let check_updates = v.get("check_updates").and_then(serde_json::Value::as_bool).unwrap_or(true);
    Some(Prefs { never, completion, complete_idle: Duration::from_millis(complete_idle), tab_hold: Duration::from_millis(tab_hold), learn_words, predict, accept, pick_ctrl, pick_arrows, input_badge, badge_idle: Duration::from_millis(badge_idle), bubble, rewrite: rewrite::Config::from_json(v.get("rewrite")), check_updates })
}

/// `prefs.json` next to the personal-state file.
pub fn prefs_beside(state: &Path) -> std::path::PathBuf {
    match state.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join("prefs.json"),
        _ => std::path::PathBuf::from("prefs.json"),
    }
}

/// Apply queued settings commands. Returns the snapshot to show, and a Hide bubble action when the
/// word currently being typed just became blocked.
pub fn apply_commands(app: &mut crate::app::App, cmds: Vec<SettingsCmd>) -> (SettingsSnap, Vec<Action>) {
    let mut personal = false;
    let mut prefs = false;
    for cmd in cmds {
        let prefs_write = cmd.writes_prefs();
        if let SettingsCmd::SetCheckUpdates(on) = cmd { crate::update::set_auto(on) }
        if let SettingsCmd::SetPredict(on) = cmd { app.set_predict(on) }
        if app.engine.apply_settings(cmd) { note_write(prefs_write, &mut personal, &mut prefs) }
    }
    if personal { app.save(); }
    if prefs { app.save_prefs(); }
    let hide = app.engine.hide_if_blocked();
    (app.engine.settings_snapshot(), hide)
}
fn note_write(prefs_write: bool, personal: &mut bool, prefs: &mut bool) {
    if prefs_write { *prefs = true } else { *personal = true }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_accepts_ok_and_its_thai_reading() {
        assert_eq!(normalize_word("  OK "), Some("ok".into()));
        assert_eq!(normalize_word("นา"), Some("ok".into()));
        assert_eq!(normalize_word("two words"), None);
        assert_eq!(normalize_word("  "), None);
        assert_eq!(normalize_word(""), None);
    }
    #[test]
    fn prefs_round_trip_collapses_the_same_word() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"never":[" OK ","นา","two words","ok"]}"#).unwrap();
        let loaded = Prefs::load(&path);
        assert_eq!(loaded.never.into_iter().collect::<Vec<_>>(), vec!["ok".to_string()]);
        loaded_save(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
    fn loaded_save(path: &Path) {
        let mut p = Prefs::default();
        p.never.insert("ok".into());
        p.rewrite = rewrite::Config::new(false, "http://localhost:11434/v1", "llama", "Use formal Thai.").unwrap();
        p.save(path);
        assert_eq!(Prefs::load(path), p);
    }
    #[test]
    fn tab_hold_round_trips_and_an_odd_value_is_the_default() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-h-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let slow = Prefs { tab_hold: Duration::from_millis(1000), ..Prefs::default() };
        slow.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["tab_hold_ms"], 1000);
        assert_eq!(Prefs::load(&path), slow);
        let mid = Prefs { tab_hold: Duration::from_millis(350), ..Prefs::default() };
        mid.save(&path); assert_eq!(Prefs::load(&path), mid, "another choice");
        std::fs::write(&path, r#"{"never":[],"tab_hold_ms":123}"#).unwrap();
        assert_eq!(Prefs::load(&path).tab_hold, Duration::from_millis(TAB_HOLD_DEFAULT_MS), "a value Settings never offers");
        std::fs::write(&path, r#"{"never":[]}"#).unwrap();
        assert_eq!(Prefs::load(&path).tab_hold, Duration::from_millis(TAB_HOLD_DEFAULT_MS), "file from before the setting");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn completion_mode_round_trips_and_defaults_to_pause() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-c-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let typing = Prefs { never: BTreeSet::from(["ok".to_string()]), completion: CompleteMode::Typing, ..Prefs::default() };
        typing.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["completion"], "typing");
        assert_eq!(Prefs::load(&path), typing);
        Prefs { completion: CompleteMode::Off, ..Prefs::default() }.save(&path);
        assert_eq!(Prefs::load(&path).completion, CompleteMode::Off);
        std::fs::write(&path, r#"{"never":["ok"]}"#).unwrap();
        assert_eq!(Prefs::load(&path).completion, CompleteMode::Pause, "missing key");
        std::fs::write(&path, r#"{"never":["ok"],"completion":"sometimes"}"#).unwrap();
        assert_eq!(Prefs::load(&path), Prefs { never: BTreeSet::from(["ok".to_string()]), ..Prefs::default() }, "unknown value");
        assert!(Prefs::load(&path).learn_words, "learning is on unless turned off");
        Prefs { learn_words: false, ..Prefs::default() }.save(&path);
        assert!(!Prefs::load(&path).learn_words);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn accept_keys_round_trip_and_an_old_file_gets_tab() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-a-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let space = Prefs { accept: AcceptKeys::TabOrSpace, ..Prefs::default() };
        space.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["accept_with"], "tab_or_space");
        assert_eq!(Prefs::load(&path), space);
        Prefs::default().save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["accept_with"], "tab");
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","complete_idle_ms":250,"learn_words":true}"#).unwrap();
        assert_eq!(Prefs::load(&path).accept, AcceptKeys::Tab, "file from before the setting");
        std::fs::write(&path, r#"{"never":[],"accept_with":"enter"}"#).unwrap();
        assert_eq!(Prefs::load(&path).accept, AcceptKeys::Tab, "a value Settings never offers");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn pick_with_arrows_round_trips_and_an_old_file_turns_it_on() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-w-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let off = Prefs { pick_arrows: false, ..Prefs::default() };
        off.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["pick_with_arrows"], false);
        assert_eq!(Prefs::load(&path), off);
        std::fs::write(&path, r#"{"never":["ok"],"pick_with_ctrl":true}"#).unwrap();
        assert!(Prefs::load(&path).pick_arrows, "file from before the setting");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn pick_with_ctrl_round_trips_and_an_old_file_leaves_it_off() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-p-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let on = Prefs { pick_ctrl: true, ..Prefs::default() };
        on.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["pick_with_ctrl"], true);
        assert_eq!(Prefs::load(&path), on);
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","complete_idle_ms":250,"learn_words":true,"accept_with":"tab"}"#).unwrap();
        assert!(!Prefs::load(&path).pick_ctrl, "file from before the setting");
        std::fs::write(&path, r#"{"never":[],"pick_with_ctrl":"yes"}"#).unwrap();
        assert!(!Prefs::load(&path).pick_ctrl, "not a boolean");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn input_badge_round_trips_and_an_old_file_leaves_it_on() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-i-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        assert!(Prefs::default().input_badge, "on by default");
        let off = Prefs { input_badge: false, ..Prefs::default() };
        off.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["input_badge"], false);
        assert_eq!(Prefs::load(&path), off);
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","pick_with_ctrl":true}"#).unwrap();
        assert!(Prefs::load(&path).input_badge, "file from before the setting");
        std::fs::write(&path, r#"{"never":[],"input_badge":"no"}"#).unwrap();
        assert!(Prefs::load(&path).input_badge, "not a boolean");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn check_updates_round_trips_and_an_old_file_leaves_it_on() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-u-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let off = Prefs { check_updates: false, ..Prefs::default() };
        off.save(&path);
        assert_eq!(Prefs::load(&path), off);
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause"}"#).unwrap();
        assert!(Prefs::load(&path).check_updates, "file from before the setting");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn bubble_layout_round_trips_and_an_old_file_gets_horizontal() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let rows = Prefs { bubble: BubbleLayout::Vertical, ..Prefs::default() };
        rows.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["bubble_layout"], "vertical");
        assert_eq!(Prefs::load(&path), rows);
        Prefs::default().save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["bubble_layout"], "horizontal");
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","accept_with":"tab","pick_with_ctrl":true}"#).unwrap();
        assert_eq!(Prefs::load(&path).bubble, BubbleLayout::Horizontal, "file from before the setting");
        std::fs::write(&path, r#"{"never":["ok"],"bubble_layout":"vertical"}"#).unwrap();
        assert_eq!(Prefs::load(&path).bubble, BubbleLayout::Vertical, "a file that only sets the layout");
        std::fs::write(&path, r#"{"never":[],"bubble_layout":"diagonal"}"#).unwrap();
        assert_eq!(Prefs::load(&path).bubble, BubbleLayout::Horizontal, "a value Settings never offers");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn completion_delay_round_trips_and_an_old_file_gets_the_default() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-d-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let fast = Prefs { complete_idle: Duration::from_millis(150), ..Prefs::default() };
        fast.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["complete_idle_ms"], 150);
        assert_eq!(Prefs::load(&path), fast);
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","learn_words":true}"#).unwrap();
        assert_eq!(Prefs::load(&path).complete_idle, Duration::from_millis(COMPLETE_IDLE_DEFAULT_MS), "file from before the setting");
        let slow = Prefs { complete_idle: Duration::from_millis(600), ..Prefs::default() };
        slow.save(&path); assert_eq!(Prefs::load(&path), slow, "the slowest choice");
        std::fs::write(&path, r#"{"never":[],"complete_idle_ms":7}"#).unwrap();
        assert_eq!(Prefs::load(&path).complete_idle, Duration::from_millis(COMPLETE_IDLE_DEFAULT_MS), "a value Settings never offers");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn preview_delay_round_trips_and_an_old_file_gets_the_default() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let quick = Prefs { badge_idle: Duration::from_millis(1000), ..Prefs::default() };
        quick.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["badge_idle_ms"], 1000);
        assert_eq!(Prefs::load(&path), quick);
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","learn_words":true}"#).unwrap();
        assert_eq!(Prefs::load(&path).badge_idle, Duration::from_millis(BADGE_IDLE_DEFAULT_MS), "file from before the setting");
        let slow = Prefs { badge_idle: Duration::from_millis(10000), ..Prefs::default() };
        slow.save(&path); assert_eq!(Prefs::load(&path), slow, "the longest choice");
        std::fs::write(&path, r#"{"never":[],"badge_idle_ms":7}"#).unwrap();
        assert_eq!(Prefs::load(&path).badge_idle, Duration::from_millis(BADGE_IDLE_DEFAULT_MS), "a value Settings never offers");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn prediction_round_trips_and_an_old_file_turns_it_on() {
        let dir = std::env::temp_dir().join(format!("typethai-prefs-p-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("prefs.json");
        let off = Prefs { predict: false, ..Prefs::default() };
        off.save(&path);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["predict"], false);
        assert_eq!(Prefs::load(&path), off);
        assert!(Prefs::load(&path).predict == false, "off stays off");
        // a file written before the setting existed gets the default, which is on
        std::fs::write(&path, r#"{"never":["ok"],"completion":"pause","learn_words":true}"#).unwrap();
        assert!(Prefs::load(&path).predict, "file from before the setting");
        let on = Prefs { predict: true, ..Prefs::default() };
        on.save(&path);
        assert_eq!(Prefs::load(&path), on);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
