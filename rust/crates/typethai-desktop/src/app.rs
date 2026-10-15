//! Shared startup: CLI options, model + personal-state loading/saving, hotkey mapping.
use crate::engine::{Engine, Hotkey, Options};
use crate::prefs::{self, Prefs};
use std::path::PathBuf;
use typethai_core::{personal::{PersonalModel, DEFAULT_PERSONAL}, ModelFiles};

pub struct Cli { pub model: PathBuf, pub state: PathBuf, pub switch_input: bool, pub reverse: bool, pub list_sources: bool, pub eval_live: Option<String>, pub eval_split: String, pub eval_reverse: bool, pub no_typo: bool, pub simulate: bool, pub auto_convert: bool, pub open_settings: bool }
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
        let mut c = Cli { model: default_model(), state: config_dir().join("personal.json"), switch_input: true, reverse: true, list_sources: false, eval_live: None, eval_split: "dev".into(), eval_reverse: false, no_typo: false, simulate: false, auto_convert: false, open_settings: false };
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() { match a.as_str() {
            "--model" => c.model = it.next().expect("--model PATH").into(), "--state" => c.state = it.next().expect("--state PATH").into(),
            "--switch-layout" => c.switch_input = true, "--no-switch-input" => c.switch_input = false, "--no-reverse" => c.reverse = false, "--list-input-sources" => c.list_sources = true,
            "--eval-live" => c.eval_live = it.next(), "--reverse" => c.eval_reverse = true, "--split" => c.eval_split = it.next().expect("--split dev|test"), "--no-typo" => c.no_typo = true, "--simulate" => c.simulate = true, "--auto-convert" => c.auto_convert = true, "--settings" => c.open_settings = true,
            "-h" | "--help" => { println!("typethai-desktop [--model FILE.ttm|DIR] [--state personal.json] [--no-switch-input] [--no-reverse] [--no-typo] [--auto-convert] [--settings] [--simulate] [--list-input-sources]\n       typethai-desktop --demo-bubble [x,y ...] [--secs N]   [--items \"a|b\"] [--select N] [--screenshot PNG]   (show the bubble at screen points, global top-left coords)\n\nNothing is converted automatically: Space/Enter/punctuation pass through and the bubble offers the conversion (also for the word just finished); Tab or Ctrl+Opt/Alt+1..3 applies it. --auto-convert restores the old convert-at-boundary behaviour.\nWhile the bubble is up: Tab applies the selected row, Shift+Up/Down move the selection, Esc dismisses, Ctrl+Opt/Alt+1..3 apply a row.\nAfter applying, the OS input source is switched to Thai (Kedmanee) or ABC/US to match (--no-switch-input to disable).\n--settings opens the Settings window at launch, for when the tray icon is hidden."); std::process::exit(0) }
            x => { eprintln!("unknown arg {x}"); std::process::exit(2) }
        } }
        c
    }
}
pub struct App { pub engine: Engine, pub state_path: PathBuf, pub prefs_path: PathBuf }
impl App {
    pub fn load(cli: &Cli) -> Result<App, String> {
        let m = ModelFiles::load(&cli.model).map_err(|e| format!("model {}: {e}", cli.model.display()))?;
        let personal = match std::fs::read_to_string(&cli.state) {
            Ok(s) => PersonalModel::from_json(m.classifier.clone(), &s).unwrap_or_else(|e| { eprintln!("personal state unreadable ({e}); starting fresh"); PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL) }),
            Err(_) => PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL),
        };
        let corrector = if cli.no_typo { None } else { Some(m.corrector()) };
        let opts = Options { switch_input: cli.switch_input, typo_suggestions: !cli.no_typo, auto_convert: cli.auto_convert, ..Options::default() };
        let mut engine = Engine::new(m.learn_detector(), personal, corrector, opts);
        let prefs_path = prefs::prefs_beside(&cli.state);
        let p = Prefs::load(&prefs_path);
        (engine.never, engine.opts.complete, engine.rewrite) = (p.never, p.completion, p.rewrite);
        if cli.reverse { engine.rev = Some(typethai_core::reverse::Reverse::embedded()); }
        engine.load_completers(&m);
        if !cli.no_typo { engine.speller = crate::platform::spell::system(); }
        Ok(App { engine, state_path: cli.state.clone(), prefs_path })
    }
    pub fn save(&mut self) {
        self.engine.personal.compact(0.05);
        if let Some(d) = self.state_path.parent() { let _ = std::fs::create_dir_all(d); }
        let tmp = self.state_path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.engine.personal.to_json()).is_ok() { let _ = std::fs::rename(&tmp, &self.state_path); }
        self.engine.events_since_save = 0;
    }
    pub fn maybe_save(&mut self) { if self.engine.events_since_save >= 10 { self.save() } }
    pub fn save_prefs(&self) { Prefs { never: self.engine.never.clone(), completion: self.engine.opts.complete, rewrite: self.engine.rewrite.clone() }.save(&self.prefs_path); }
}
/// Hotkeys are Ctrl+Alt (Windows) / Ctrl+Option (macOS) + key char.
pub fn hotkey_for(c: char) -> Option<Hotkey> {
    Some(match c.to_ascii_lowercase() { 'z' => Hotkey::Undo, 'x' => Hotkey::ForceConvert, 'g' => Hotkey::Guess, '1' => Hotkey::Accept(0), '2' => Hotkey::Accept(1), '3' => Hotkey::Accept(2), 'p' => Hotkey::TogglePause, 'r' => Hotkey::Rewrite, _ => return None })
}
