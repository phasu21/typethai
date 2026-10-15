//! Shared startup: CLI options, model + personal-state loading/saving, hotkey mapping.
use crate::engine::{Engine, Hotkey, Options};
use crate::prefs::{self, Prefs};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::path::Path;
use typethai_core::{completer::WordMemory, personal::{PersonalModel, DEFAULT_PERSONAL}, ModelFiles};

pub struct Cli { pub model: PathBuf, pub state: PathBuf, pub switch_input: bool, pub reverse: bool, pub list_sources: bool, pub eval_live: Option<String>, pub eval_split: String, pub eval_reverse: bool, pub eval_typo: Option<String>, pub slip_rate: Option<f64>, pub seed: u64, pub per_file: usize, pub no_typo: bool, pub simulate: bool, pub auto_convert: bool, pub open_settings: bool }
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
        let mut c = Cli { model: default_model(), state: config_dir().join("personal.json"), switch_input: true, reverse: true, list_sources: false, eval_live: None, eval_split: "dev".into(), eval_reverse: false, eval_typo: None, slip_rate: None, seed: 1, per_file: 1500, no_typo: false, simulate: false, auto_convert: false, open_settings: false };
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() { match a.as_str() {
            "--model" => c.model = it.next().expect("--model PATH").into(), "--state" => c.state = it.next().expect("--state PATH").into(),
            "--switch-layout" => c.switch_input = true, "--no-switch-input" => c.switch_input = false, "--no-reverse" => c.reverse = false, "--list-input-sources" => c.list_sources = true,
            "--eval-live" => c.eval_live = it.next(), "--reverse" => c.eval_reverse = true,
            "--eval-typo" => c.eval_typo = it.next(), "--slip-rate" => c.slip_rate = it.next().and_then(|r| r.parse().ok()), "--seed" => c.seed = it.next().and_then(|r| r.parse().ok()).expect("--seed N"), "--per-file" => c.per_file = it.next().and_then(|r| r.parse().ok()).expect("--per-file N"), "--split" => c.eval_split = it.next().expect("--split dev|test"), "--no-typo" => c.no_typo = true, "--simulate" => c.simulate = true, "--auto-convert" => c.auto_convert = true, "--settings" => c.open_settings = true,
            "-h" | "--help" => { println!("typethai-desktop [--model FILE.ttm|DIR] [--state personal.json] [--no-switch-input] [--no-reverse] [--no-typo] [--auto-convert] [--settings] [--simulate] [--list-input-sources]\n       typethai-desktop --demo-bubble [x,y ...] [--secs N]   [--items \"a|b\"] [--select N] [--vertical] [--light] [--screenshot PNG]   (show the bubble at screen points, global top-left coords)\n\nNothing is converted automatically: Space/Enter/punctuation pass through and the bubble offers the conversion (also for the word just finished); Tab or Ctrl+Opt/Alt+1..3 applies it. --auto-convert restores the old convert-at-boundary behaviour.\nWhile the bubble is up: Tab applies the selected row when released, Tab held + 1..3 applies that row, Shift+Up/Down move the selection, Esc dismisses, Ctrl+Opt/Alt+1..3 apply a row.\nSettings > Word suggestions > Pick with Ctrl+1–3 (prefs.json \"pick_with_ctrl\": true, off by default) makes Ctrl+1..3 apply a row while the bubble is up; otherwise those keys reach the app.\nCtrl+Opt/Alt+R: with text selected, offers it in the other layout and with typos fixed (on this computer), then AI Fix typos / Translate when set up in Settings; with nothing selected, offers the conversion and spelling fixes for the word before the caret.\nSettings > Word suggestions > Accept with: Tab or Space (prefs.json \"accept_with\": \"tab_or_space\") makes Space apply the highlighted row too, only when it is a mid-word conversion, then type the space; over other rows Space is a plain space. With --auto-convert too, a word with a live row up is converted by that row, and other words still convert at the boundary.\nAfter applying, the OS input source is switched to Thai (Kedmanee) or ABC/US to match (--no-switch-input to disable).\n--settings opens the Settings window at launch, for when the tray icon is hidden."); std::process::exit(0) }
            x => { eprintln!("unknown arg {x}"); std::process::exit(2) }
        } }
        c
    }
}
pub struct App { pub engine: Engine, pub state_path: PathBuf, pub prefs_path: PathBuf, pub words_path: PathBuf, saver: Saver }
/// Writes the state files on a thread of its own, in the order asked: serialising a full words.json takes about 1.5 ms,
/// too long for the key tap callback that triggers the save.
struct Saver { tx: mpsc::Sender<Save> }
enum Save { Text(PathBuf, String), Words(PathBuf, Arc<WordMemory>), Flush(mpsc::Sender<()>) }
impl Saver {
    fn spawn() -> Saver {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || for s in rx {
            match s {
                Save::Text(p, t) => write_file(&p, &t),
                Save::Words(p, m) => write_file(&p, &m.to_json()),
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
        (engine.never, engine.opts.complete, engine.opts.complete_idle, engine.opts.learn_words, engine.opts.accept, engine.opts.pick_ctrl, engine.opts.bubble, engine.rewrite) = (p.never, p.completion, p.complete_idle, p.learn_words, p.accept, p.pick_ctrl, p.bubble, p.rewrite);
        let words_path = cli.state.with_file_name("words.json");
        engine.words = Arc::new(load_words(&words_path));
        if cli.reverse { engine.rev = Some(typethai_core::reverse::Reverse::embedded()); }
        engine.load_completers(&mut m);
        if !cli.no_typo { engine.speller = crate::platform::spell::system(); }
        Ok(App { engine, state_path: cli.state.clone(), prefs_path, words_path, saver: Saver::spawn() })
    }
    pub fn save(&mut self) {
        self.engine.personal.compact(0.05);
        self.saver.send(Save::Text(self.state_path.clone(), self.engine.personal.to_json()));
        self.saver.send(Save::Words(self.words_path.clone(), Arc::clone(&self.engine.words)));
        self.engine.events_since_save = 0;
    }
    /// Save, and return once the files are on disk (quitting).
    pub fn save_and_wait(&mut self) { self.save(); self.saver.flush(); }
    pub fn maybe_save(&mut self) { if self.engine.events_since_save >= 10 { self.save() } }
    pub fn save_prefs(&self) { Prefs { never: self.engine.never.clone(), completion: self.engine.opts.complete, complete_idle: self.engine.opts.complete_idle, learn_words: self.engine.opts.learn_words, accept: self.engine.opts.accept, pick_ctrl: self.engine.opts.pick_ctrl, bubble: self.engine.opts.bubble, rewrite: self.engine.rewrite.clone() }.save(&self.prefs_path); }
}
/// Hotkeys are Ctrl+Alt (Windows) / Ctrl+Option (macOS) + key char.
pub fn hotkey_for(c: char) -> Option<Hotkey> {
    Some(match c.to_ascii_lowercase() { 'z' => Hotkey::Undo, 'x' => Hotkey::ForceConvert, 'g' => Hotkey::Guess, '1' => Hotkey::Accept(0), '2' => Hotkey::Accept(1), '3' => Hotkey::Accept(2), 'p' => Hotkey::TogglePause, 'r' => Hotkey::Rewrite, _ => return None })
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
