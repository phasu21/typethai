//! User preferences: words typeThai should not translate, and when word completions show.
//! Stored beside `personal.json` so a learning reset does not clear them, and so `personal.json`
//! stays compatible with the TypeScript app.
use crate::engine::{Action, CompleteMode};
use crate::rewrite;
use std::collections::BTreeSet;
use std::path::Path;
use typethai_core::keymap::th_to_en;
use typethai_core::personal::Learned;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsCmd {
    AddNever(String),
    RemoveNever(String),
    Forget(String),
    ResetLearning,
    SetCompletion(CompleteMode),
    SetLearnWords(bool),
    ForgetWords,
    /// Never carries the API key; that goes from the Settings window straight to the keyring.
    SetRewrite(rewrite::Config),
}
impl SettingsCmd {
    pub fn writes_prefs(&self) -> bool {
        matches!(self, SettingsCmd::AddNever(_) | SettingsCmd::RemoveNever(_) | SettingsCmd::SetCompletion(_) | SettingsCmd::SetLearnWords(_) | SettingsCmd::SetRewrite(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsSnap {
    pub never: Vec<String>,
    pub learned: Vec<Learned>,
    pub completion: CompleteMode,
    pub learn_words: bool,
    /// Distinct words in the word memory.
    pub words_learned: usize,
    pub rewrite: rewrite::Config,
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
pub struct Prefs { pub never: BTreeSet<String>, pub completion: CompleteMode, pub learn_words: bool, pub rewrite: rewrite::Config }
impl Default for Prefs {
    fn default() -> Self { Prefs { never: BTreeSet::new(), completion: CompleteMode::default(), learn_words: true, rewrite: rewrite::Config::default() } }
}
/// `prefs.json` spelling of each completion mode.
const COMPLETION: [(CompleteMode, &str); 3] = [(CompleteMode::Off, "off"), (CompleteMode::Pause, "pause"), (CompleteMode::Typing, "typing")];
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
        serde_json::json!({ "never": self.never.iter().collect::<Vec<_>>(), "completion": completion, "learn_words": self.learn_words, "rewrite": self.rewrite.to_json() }).to_string()
    }
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.to_json()).is_ok() { let _ = std::fs::rename(&tmp, path); }
    }
}
/// None when the file is not JSON or `never` is not a list; an unknown or missing `completion` or `learn_words` is the default.
fn parse(text: &str) -> Option<Prefs> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let never = match v.get("never") {
        Some(n) => n.as_array()?.iter().filter_map(|x| x.as_str()).filter_map(normalize_word).collect(),
        None => BTreeSet::new(),
    };
    let c = v.get("completion").and_then(|c| c.as_str());
    let completion = COMPLETION.iter().find(|(_, s)| Some(*s) == c).map_or(CompleteMode::default(), |(m, _)| *m);
    let learn_words = v.get("learn_words").and_then(serde_json::Value::as_bool).unwrap_or(true);
    Some(Prefs { never, completion, learn_words, rewrite: rewrite::Config::from_json(v.get("rewrite")) })
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
}
