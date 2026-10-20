//! Shared startup: CLI options, model + personal-state loading/saving, hotkey mapping.
use crate::engine::{Action, Engine, Hotkey, Options};
use crate::prefs::{self, Prefs};
use crate::rank;
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::path::Path;
use typethai_core::{completer::WordMemory, personal::{PersonalModel, DEFAULT_PERSONAL}, ModelFiles};

pub struct Cli { pub model: PathBuf, pub state: PathBuf, pub switch_input: bool, pub reverse: bool, pub list_sources: bool, pub eval_live: Option<String>, pub eval_split: String, pub eval_reverse: bool, pub eval_typo: Option<String>, pub slip_rate: Option<f64>, pub seed: u64, pub per_file: usize, pub no_typo: bool, pub simulate: bool, pub auto_convert: bool, pub open_settings: bool, pub settings_page: usize, pub rank_report: Option<String>, pub whats_new: bool }
pub fn config_dir() -> PathBuf {
    #[cfg(windows)] { std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into()).join("typethai") }
    #[cfg(target_os = "macos")] { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into()).join("Library/Application Support/typethai") }
    #[cfg(not(any(windows, target_os = "macos")))] { std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| ".".into()).join(".config/typethai") }
}
fn default_model() -> PathBuf {
    if let Some(p) = std::env::var_os("TYPETHAI_MODEL") { return p.into() }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_else(|| ".".into());
    for c in [exe_dir.join("typethai.ttm"), exe_dir.join("../Resources/typethai.ttm"), config_dir().join("typethai.ttm")] { if c.exists() { return c } }
    exe_dir.join("typethai.ttm")
}
impl Cli {
    pub fn parse() -> Cli {
        let mut c = Cli { model: default_model(), state: config_dir().join("personal.json"), switch_input: true, reverse: true, list_sources: false, eval_live: None, eval_split: "dev".into(), eval_reverse: false, eval_typo: None, slip_rate: None, seed: 1, per_file: 1500, no_typo: false, simulate: false, auto_convert: false, open_settings: false, settings_page: 0, rank_report: None, whats_new: false };
        let mut it = std::env::args().skip(1).peekable();
        while let Some(a) = it.next() { match a.as_str() {
            "--model" => c.model = it.next().expect("--model PATH").into(), "--state" => c.state = it.next().expect("--state PATH").into(),
            "--switch-layout" => c.switch_input = true, "--no-switch-input" => c.switch_input = false, "--no-reverse" => c.reverse = false, "--list-input-sources" => c.list_sources = true,
            "--eval-live" => c.eval_live = it.next(), "--reverse" => c.eval_reverse = true,
            "--eval-typo" => c.eval_typo = it.next(), "--slip-rate" => c.slip_rate = it.next().and_then(|r| r.parse().ok()), "--seed" => c.seed = it.next().and_then(|r| r.parse().ok()).expect("--seed N"), "--per-file" => c.per_file = it.next().and_then(|r| r.parse().ok()).expect("--per-file N"), "--split" => c.eval_split = it.next().expect("--split dev|test"), "--no-typo" => c.no_typo = true, "--simulate" => c.simulate = true, "--auto-convert" => c.auto_convert = true, "--settings" => c.open_settings = true, "--whats-new" => c.whats_new = true, "--settings-page" => c.settings_page = it.next().and_then(|s| s.parse().ok()).expect("--settings-page N"),
            // the file is optional: without one the log beside the state file is read
            "--rank-report" => c.rank_report = Some(if it.peek().is_some_and(|s| !s.starts_with("--")) { it.next().unwrap() } else { String::new() }),
            "-h" | "--help" => { println!("typethai-desktop [--model FILE.ttm|DIR] [--state personal.json] [--no-switch-input] [--no-reverse] [--no-typo] [--auto-convert] [--settings [--settings-page N]] [--whats-new] [--simulate] [--list-input-sources] [--check-update]\n       typethai-desktop --demo-bubble [x,y ...] [--secs N]   [--items \"a|b\"] [--select N] [--vertical] [--light] [--backdrop light|dark] [--screenshot PNG]   (show the bubble at screen points, global top-left coords)\n       typethai-desktop --demo-badge [x,y ...] [--secs N] [--en] [--light] [--backdrop light|dark] [--screenshot PNG]   (show the input-language badge at a fake caret)\n       typethai-desktop --demo-hold [x,y ...] [--progress P] [--light] [--backdrop light|dark] [--screenshot PNG]   (show the hold-Tab indicator)\n       typethai-desktop --demo-toast   (post the update notification, and say whether this copy can)\n       typethai-desktop --check-update   (ask GitHub for the latest release and print it; nothing is downloaded)\n\nNothing is converted automatically: Space/Enter/punctuation pass through and the bubble offers the conversion (also for the word just finished); Tab or Ctrl+Opt/Alt+1..3 applies it. --auto-convert restores the old convert-at-boundary behaviour.\nWhile the bubble is up: Tab applies the selected row when released, Tab held + 1..3 applies that row, Shift+Up/Down move the selection, Esc dismisses, Ctrl+Opt/Alt+1..3 apply a row.\nSettings > Word suggestions > Pick with: Ctrl+1–3 (prefs.json \"pick_with_ctrl\": true, off by default) makes Ctrl+1..3 apply a row while the bubble is up; otherwise those keys reach the app.\nCtrl+Opt/Alt+R: with text selected, offers it in the other layout and with typos fixed (on this computer), then AI Fix typos / Translate when set up in Settings; with nothing selected, offers the conversion and spelling fixes for the word before the caret. Tab held + R does the same for the word just typed (Tab after a word is sent on release).\nSettings > Word suggestions > Accept with: Tab or Space (prefs.json \"accept_with\": \"tab_or_space\") makes Space apply the highlighted row too, only when it is a mid-word conversion, then type the space; over other rows Space is a plain space. With --auto-convert too, a word with a live row up is converted by that row, and other words still convert at the boundary.\nAfter applying, the OS input source is switched to Thai (Kedmanee) or ABC/US to match (--no-switch-input to disable).\n--settings opens the Settings window at launch, for when the tray icon is hidden.\n--whats-new shows the What's new window for this version at launch (it also shows once by itself after an update).\n--rank-report [FILE] prints what the bubble offered and which row was picked (default: rank.jsonl beside the state file)."); std::process::exit(0) }
            x => { eprintln!("unknown arg {x}"); std::process::exit(2) }
        } }
        c
    }
}
pub struct App { pub engine: Engine, pub state_path: PathBuf, pub prefs_path: PathBuf, pub words_path: PathBuf, pub rank_path: PathBuf, saver: Saver,
    /// Apps typeThai is turned fully off in (see `prefs::app_key`), keyed by the app id as stored; the value is the name
    /// the tray and Settings show. Mirrors `prefs.json`.
    off_apps: std::collections::BTreeMap<String, String>,
    /// The app keys/focus went to last, other than typeThai itself: (raw id, display name). What `refresh_front_off`
    /// recomputes the off state for when the user turns the app on/off from the tray or Settings.
    front: Option<(String, String)> }
/// Writes the state files on a thread of its own, in the order asked: serialising a full words.json takes about 1.5 ms,
/// too long for the key tap callback that triggers the save.
struct Saver { tx: mpsc::Sender<Save> }
/// The saver's work: whole files rewritten, and the offer log, which is appended to (its flag clears it first).
enum Save { Text(PathBuf, String), Words(PathBuf, Arc<WordMemory>), Rank(PathBuf, Vec<rank::Event>, bool), Flush(mpsc::Sender<()>) }
impl Saver {
    fn spawn() -> Saver {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || for s in rx {
            match s {
                Save::Text(p, t) => write_file(&p, &t),
                Save::Words(p, m) => write_file(&p, &m.to_json()),
                // the log is appended to, not rewritten: a clear is the only time the file goes
                Save::Rank(p, e, clear) => { if clear { rank::remove(&p) } let _ = rank::append(&p, &e); }
                Save::Flush(done) => { let _ = done.send(()); }
            }
        });
        Saver { tx }
    }
    fn send(&self, s: Save) { let _ = self.tx.send(s); }
    /// Block until everything asked before is written.
    fn flush(&self) { let (done, wait) = mpsc::channel(); self.send(Save::Flush(done)); let _ = wait.recv(); }
}
/// The word memory in `path`; a missing or unreadable file starts it empty.
fn load_words(path: &Path) -> WordMemory {
    let Ok(s) = std::fs::read_to_string(path) else { return WordMemory::default() };
    WordMemory::from_json(&s).unwrap_or_else(|e| { eprintln!("typeThai: word memory unreadable ({e}); starting empty"); WordMemory::default() })
}
/// Write through a temp file, so a crash mid-write leaves the old file.
fn write_file(path: &Path, text: &str) {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() { let _ = std::fs::rename(&tmp, path); }
}
impl App {
    pub fn load(cli: &Cli) -> Result<App, String> {
        let mut m = ModelFiles::load(&cli.model).map_err(|e| format!("model {}: {e}", cli.model.display()))?;
        let personal = match std::fs::read_to_string(&cli.state) {
            Ok(s) => PersonalModel::from_json(m.classifier.clone(), &s).unwrap_or_else(|e| { eprintln!("personal state unreadable ({e}); starting fresh"); PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL) }),
            Err(_) => PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL),
        };
        let corrector = if cli.no_typo { None } else { Some(m.corrector()) };
        let opts = Options { switch_input: cli.switch_input, typo_suggestions: !cli.no_typo, auto_convert: cli.auto_convert, ..Options::default() };
        let mut engine = Engine::new(m.learn_detector(), personal, corrector, opts);
        let prefs_path = prefs::prefs_beside(&cli.state);
        let p = Prefs::load(&prefs_path);
        (engine.never, engine.offer_last, engine.opts.complete, engine.opts.complete_idle, engine.opts.tab_hold, engine.opts.learn_words, engine.opts.predict, engine.opts.accept, engine.opts.pick_ctrl, engine.opts.pick_arrows, engine.opts.input_badge, engine.opts.badge_idle, engine.opts.next_word, engine.opts.bubble, engine.rewrite, engine.check_updates) = (p.never, p.offer_last, p.completion, p.complete_idle, p.tab_hold, p.learn_words, p.predict, p.accept, p.pick_ctrl, p.pick_arrows, p.input_badge, p.badge_idle, p.next_word, p.bubble, p.rewrite, p.check_updates);
        let words_path = cli.state.with_file_name("words.json");
        engine.words = Arc::new(load_words(&words_path));
        // a log that has grown past its cap comes down to its last half once, here, not on the typing path
        let rank_path = rank::path_beside(&cli.state);
        rank::trim(&rank_path);
        if cli.reverse { engine.rev = Some(typethai_core::reverse::Reverse::embedded()); }
        engine.load_completers(&mut m);
        if !cli.no_typo { engine.speller = crate::platform::spell::system(); }
        let mut app = App { engine, state_path: cli.state.clone(), prefs_path, words_path, rank_path, off_apps: p.off_apps, front: None, saver: Saver::spawn() };
        app.set_predict(app.engine.opts.predict);
        Ok(app)
    }
    /// Start or stop the contextual predictor with its setting. While it is off no model is read and no state file is
    /// written.
    pub fn set_predict(&mut self, on: bool) {
        if on {
            if self.engine.predict.is_none() {
                let dir = self.words_path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
                let (thai, english) = (Arc::clone(&self.engine.det.th_dict), Arc::clone(&self.engine.det.en_words));
                let oracle = Arc::new(typethai_core::predict::KnownWords::new(thai, english));
                self.engine.predict = Some(crate::predict::Predict::spawn(dir, oracle));
            }
        } else if let Some(p) = self.engine.predict.take() { p.flush() }
    }
    pub fn save(&mut self) {
        self.engine.personal.compact(0.05);
        self.saver.send(Save::Text(self.state_path.clone(), self.engine.personal.to_json()));
        self.saver.send(Save::Words(self.words_path.clone(), Arc::clone(&self.engine.words)));
        self.engine.events_since_save = 0;
        self.save_rank();
    }
    /// Hand the closed offers to the saver, and drop the log file when learning was reset. Nothing is sent when there
    /// is nothing to write.
    fn save_rank(&mut self) {
        let cleared = std::mem::take(&mut self.engine.rank.cleared);
        let events = self.engine.rank.take_pending();
        if cleared || !events.is_empty() { self.saver.send(Save::Rank(self.rank_path.clone(), events, cleared)); }
    }
    /// Save, and return once the files are on disk (quitting).
    pub fn save_and_wait(&mut self) { self.save(); if let Some(p) = &self.engine.predict { p.flush() } self.saver.flush(); }
    /// After a key or a click: the state every few learning events, prefs.json as soon as a rating changed it.
    pub fn maybe_save(&mut self) {
        if self.engine.events_since_save >= 10 { self.save() }
        else if self.engine.rank.pending() >= rank::FLUSH_EVERY || self.engine.rank.cleared { self.save_rank() }
        if std::mem::take(&mut self.engine.prefs_dirty) { self.save_prefs() }
    }
    pub fn save_prefs(&self) { Prefs { never: self.engine.never.clone(), offer_last: self.engine.offer_last.clone(), completion: self.engine.opts.complete, complete_idle: self.engine.opts.complete_idle, tab_hold: self.engine.opts.tab_hold, learn_words: self.engine.opts.learn_words, predict: self.engine.opts.predict, accept: self.engine.opts.accept, pick_ctrl: self.engine.opts.pick_ctrl, pick_arrows: self.engine.opts.pick_arrows, input_badge: self.engine.opts.input_badge, badge_idle: self.engine.opts.badge_idle, next_word: self.engine.opts.next_word, bubble: self.engine.opts.bubble, rewrite: self.engine.rewrite.clone(), check_updates: self.engine.check_updates, off_apps: self.off_apps.clone() }.save(&self.prefs_path); }
    /// Keys or focus just went to this app (not typeThai itself): remember it, recompute whether it is off from prefs,
    /// tell the tray/UI and return the engine's actions (a bubble or preview that was up goes away).
    pub fn set_front_app(&mut self, id: &str, name: &str) -> Vec<Action> {
        self.front = Some((id.to_string(), name.to_string()));
        self.engine.rank.app = prefs::app_key(id);
        self.refresh_front_off()
    }
    /// Recompute the off state for the app in front and publish it. An empty id is never off (`prefs::app_key`).
    fn refresh_front_off(&mut self) -> Vec<Action> {
        let Some((id, name)) = self.front.clone() else { *crate::bridge::FRONT_APP.lock().unwrap() = None; return vec![] };
        let key = prefs::app_key(&id);
        let off = !key.is_empty() && self.off_apps.contains_key(&key);
        *crate::bridge::FRONT_APP.lock().unwrap() = Some(crate::bridge::FrontApp { id, name, off });
        self.engine.set_app_off(off)
    }
    /// Turn typeThai off in an app (from the tray or Settings). A blank id is ignored.
    pub fn app_off(&mut self, id: &str, name: String) -> Vec<Action> {
        let key = prefs::app_key(id);
        if key.is_empty() { return vec![] }
        self.off_apps.insert(key, name);
        self.refresh_front_off()
    }
    /// Turn it back on there.
    pub fn app_on(&mut self, id: &str) -> Vec<Action> {
        self.off_apps.remove(&prefs::app_key(id));
        self.refresh_front_off()
    }
    /// The off apps for the Settings page: (key id, display name), sorted by name case-insensitively.
    pub fn off_apps_sorted(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self.off_apps.iter().map(|(k, n)| (k.clone(), n.clone())).collect();
        v.sort_by_cached_key(|(_, n)| n.to_lowercase());
        v
    }
}
/// Hotkeys are Ctrl+Alt (Windows) / Ctrl+Option (macOS) + key char.
pub fn hotkey_for(c: char) -> Option<Hotkey> {
    Some(match c.to_ascii_lowercase() { 'z' => Hotkey::Undo, 'x' => Hotkey::ForceConvert, 'g' => Hotkey::Guess, '1' => Hotkey::Accept(0), '2' => Hotkey::Accept(1), '3' => Hotkey::Accept(2), 'p' => Hotkey::TogglePause, 'r' => Hotkey::Rewrite, _ => return None })
}
#[cfg(test)]
impl App {
    /// A minimal App around a model-free engine, for tests in other modules that never touch the state files.
    pub(crate) fn for_test(engine: Engine) -> App {
        let dir = std::env::temp_dir().join(format!("typethai-app-test-{}", std::process::id()));
        App { engine, state_path: dir.join("personal.json"), prefs_path: dir.join("prefs.json"), words_path: dir.join("words.json"), rank_path: dir.join("rank.jsonl"), off_apps: Default::default(), front: None, saver: Saver::spawn() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn word_memory_loads_or_starts_empty() {
        let dir = std::env::temp_dir().join(format!("typethai-words-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("words.json");
        assert_eq!(load_words(&path), WordMemory::default(), "missing");
        let mut m = WordMemory::default();
        m.learn(Some("my"), "helmet");
        write_file(&path, &m.to_json());
        assert_eq!(load_words(&path), m);
        std::fs::write(&path, "{\"clock\":").unwrap();
        assert_eq!(load_words(&path), WordMemory::default(), "corrupt");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn the_saver_appends_the_rank_log_and_a_clear_wipes_it() {
        let dir = std::env::temp_dir().join(format!("typethai-rank-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("rank.jsonl");
        let saver = Saver::spawn();
        let offered = rank::Event { t: 1, app: "com.apple.TextEdit".into(), w: "l;y".into(), prev: None, rev: false,
            rows: vec![rank::Row { w: "สวัสดี".into(), r: Some("th".into()), c: None, s: None }], pick: None, ms: 400, suppressed: 0 };
        saver.send(Save::Rank(path.clone(), vec![offered.clone()], false));
        saver.flush();
        assert_eq!(rank::load(&path).0, vec![offered.clone()], "offers are appended as they close");
        saver.send(Save::Rank(path.clone(), vec![], true));
        saver.flush();
        assert!(rank::load(&path).0.is_empty(), "a learning reset leaves no offers behind");
        saver.send(Save::Rank(path.clone(), vec![offered.clone()], false));
        saver.flush();
        assert_eq!(rank::load(&path).0, vec![offered], "and the log starts again after it");
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn the_saver_writes_the_memory_as_it_was_when_asked() {
        let dir = std::env::temp_dir().join(format!("typethai-saver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("words.json");
        let saver = Saver::spawn();
        let mut words = Arc::new(WordMemory::default());
        Arc::make_mut(&mut words).learn(None, "helmet");
        let asked = (*words).clone();
        saver.send(Save::Words(path.clone(), Arc::clone(&words)));
        Arc::make_mut(&mut words).learn(Some("helmet"), "red");
        saver.flush();
        assert_eq!(load_words(&path), asked);
        assert_eq!(words.len(), 2, "learning went on while the save ran");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
