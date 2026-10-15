//! Words the user asked typeThai not to translate.
//! Stored beside `personal.json` so a learning reset does not clear the list, and so `personal.json`
//! stays compatible with the TypeScript app.
use crate::engine::Action;
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
}
impl SettingsCmd {
    pub fn writes_prefs(&self) -> bool {
        matches!(self, SettingsCmd::AddNever(_) | SettingsCmd::RemoveNever(_))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsSnap {
    pub never: Vec<String>,
    pub learned: Vec<Learned>,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prefs { pub never: BTreeSet<String> }
impl Prefs {
    pub fn load(path: &Path) -> Prefs {
        let Ok(text) = std::fs::read_to_string(path) else { return Prefs::default() };
        match parse_never(&text) {
            Some(never) => Prefs { never },
            None => {
                eprintln!("typeThai: prefs unreadable ({}); starting with an empty list", path.display());
                Prefs::default()
            }
        }
    }
    pub fn to_json(&self) -> String {
        serde_json::json!({ "never": self.never.iter().collect::<Vec<_>>() }).to_string()
    }
    pub fn save(&self, path: &Path) {
        if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.to_json()).is_ok() { let _ = std::fs::rename(&tmp, path); }
    }
}
fn parse_never(text: &str) -> Option<BTreeSet<String>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let Some(n) = v.get("never") else { return Some(BTreeSet::new()) };
    let items = n.as_array()?;
    Some(items.iter().filter_map(|x| x.as_str()).filter_map(normalize_word).collect())
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
        p.save(path);
        assert_eq!(Prefs::load(path).never, p.never);
    }
}
