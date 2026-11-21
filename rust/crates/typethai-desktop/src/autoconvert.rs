//! "Auto-convert when sure": the levels, what each must clear, and the text and apps automatic conversion stays out of.
//! The engine applies the rule at a word boundary (`Engine::auto_rule`); `--eval-auto` measures it (`evalauto.rs`).

/// How sure "Auto-convert when sure" must be (Settings > Word suggestions > How sure; prefs.json `auto_convert_level`).
/// Each level is a rule over the boundary scores (`AutoRule`); the numbers come from `--eval-auto` (README, Accuracy).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum AutoLevel { #[default] Cautious, Balanced, Eager }
/// What a finished word must clear to be converted with no key pressed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoRule {
    /// Forward (English keys meant as Thai): the personal flip probability.
    pub fwd: f64,
    /// Reverse (Thai layout, English meant): the reverse model's probability.
    pub rev: f64,
    /// Fewest keys in the word: short words are where English and Thai readings collide (`wo` ไน, `fu` ดี).
    pub min_keys: usize,
    /// Forward: the Thai reading must be a dictionary word. Off at every level: Thai is written without spaces, so
    /// what ends at a Space is usually a run of words (`สวัสดีครับ`), and `--eval-auto` measured that this rule stops
    /// nearly all of them while length and probability alone already keep false conversions at zero. Reverse always
    /// needs a known English word (whose Thai reading is not a Thai word).
    pub dict_only: bool,
}
/// What the rule reads for one finished word (`Engine::auto_score`): the direction's probability, whether the text is
/// convertible at all, and whether the converted word is a known word.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AutoScore { pub p: f64, pub ok: bool, pub known: bool }
impl AutoRule {
    /// The scores clear this rule (`rev`: English typed on the Thai layout). Length, addresses and the user's lists are
    /// checked by the engine first.
    pub fn admits(&self, s: AutoScore, rev: bool) -> bool { s.ok && if rev { s.p > self.rev && s.known } else { s.p > self.fwd && (s.known || !self.dict_only) } }
}
impl AutoLevel {
    pub const ALL: [AutoLevel; 3] = [AutoLevel::Cautious, AutoLevel::Balanced, AutoLevel::Eager];
    pub fn rule(self) -> AutoRule {
        match self {
            AutoLevel::Cautious => AutoRule { fwd: AUTO_CAUTIOUS.0, rev: AUTO_CAUTIOUS.1, min_keys: AUTO_CAUTIOUS.2, dict_only: false },
            AutoLevel::Balanced => AutoRule { fwd: AUTO_BALANCED.0, rev: AUTO_BALANCED.1, min_keys: AUTO_BALANCED.2, dict_only: false },
            AutoLevel::Eager => AutoRule { fwd: AUTO_EAGER.0, rev: AUTO_EAGER.1, min_keys: AUTO_EAGER.2, dict_only: false },
        }
    }
}
/// (forward probability, reverse probability, fewest keys) per level; see `AutoLevel::rule`. `--eval-auto`, model-v4,
/// fresh personal model, false conversions on 23,408 English-layout words (top 20,000 English words in lines, the top
/// 3,000 alone, and `eval/auto-english.txt`) and 22,658 Thai-layout Thai words; recall on 19,633 top Thai words,
/// 3,000 unspaced two-word runs and the chat lines (as keys), and 19,999 English words on the Thai layout:
/// - Cautious: 0 false | Thai words 82.5%, runs 96.4%, chat 23/25 | reverse 51.7%
/// - Balanced: 1 false (`aviv`, 0.4 per 10k) | 84.6%, 97.9%, 24/25 | reverse 73.6%
/// - Eager: 8 false (`dvd duh lulu ryo siu zhu aviv wfh`, 3.4 per 10k) | 95.2%, 98.7%, 24/25 | reverse 85.7%
/// No level flipped any of the Thai-layout Thai. The full grid is in `docs/research/auto-convert-eval.md`.
pub const AUTO_CAUTIOUS: (f64, f64, usize) = (0.995, 0.995, 4);
pub const AUTO_BALANCED: (f64, f64, usize) = (0.99, 0.98, 4);
pub const AUTO_EAGER: (f64, f64, usize) = (0.98, 0.95, 3);
/// Keys that read as a web address, an e-mail address, a path or a handle are never converted with no key pressed:
/// `.` `/` `@` `:` are Thai letters on Kedmanee (ใ ฝ ๑ ซ), so the scores alone cannot be trusted there.
/// Top-level domains `looks_like_address` recognises.
const TLDS: &[&str] = &["com", "net", "org", "io", "co", "th", "dev", "app", "ai", "rs", "me", "uk", "edu", "gov", "info", "ly", "gg", "us", "de", "jp", "xyz", "tv", "cc"]; // not "sh" or "so": `mt.sh` is ทำให้
pub fn looks_like_address(typed: &str) -> bool {
    let t = typed.to_ascii_lowercase();
    if t.contains("://") || t.starts_with("www.") || t.contains('@') || t.starts_with('/') || t.starts_with("~/") || t.starts_with("./") { return true }
    // name.tld: only common top-level domains, since `.` is ใ and `mt.sh` is ทำให้
    let parts: Vec<&str> = t.trim_end_matches('/').split('/').next().unwrap_or("").split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        && parts.last().is_some_and(|p| TLDS.contains(p))
}
/// Apps automatic conversion stays out of (macOS bundle id prefix / Windows executable name, any case): terminals and
/// code editors, where a word that reads as Thai is far more often a command, an identifier or a path. The bubble still
/// offers conversions there; only the conversion with no key pressed is off.
const EXCLUDED_APPS: &[&str] = &[
    // terminals
    "com.apple.terminal", "com.googlecode.iterm2", "com.mitchellh.ghostty", "dev.warp.", "com.github.wez.wezterm", "org.alacritty",
    "io.alacritty", "net.kovidgoyal.kitty", "co.zeit.hyper", "org.tabby", "com.stablyai.orca",
    // code editors and IDEs
    "com.microsoft.vscode", "com.microsoft.vscodeinsiders", "com.vscodium", "com.todesktop.230313mzl4w4u92", "dev.zed.zed", "com.sublimetext.", "com.jetbrains.",
    "com.google.android.studio", "com.apple.dt.xcode", "com.panic.nova", "org.vim.macvim", "com.qvacua.vimr", "org.gnu.emacs",
    "com.barebones.bbedit", "com.coteditor.coteditor", "com.exafunction.windsurf",
    // Windows
    "windowsterminal.exe", "openconsole.exe", "conhost.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "wezterm-gui.exe", "alacritty.exe",
    "mintty.exe", "code.exe", "code - insiders.exe", "cursor.exe", "windsurf.exe", "zed.exe", "sublime_text.exe", "devenv.exe",
    "idea64.exe", "pycharm64.exe", "webstorm64.exe", "rider64.exe", "clion64.exe", "goland64.exe", "phpstorm64.exe", "studio64.exe",
    "notepad++.exe", "gvim.exe",
];
/// Automatic conversion is off in this app (`EXCLUDED_APPS`). An empty id is not excluded: the field check still applies.
pub fn excluded_app(id: &str) -> bool {
    let id = id.trim().to_ascii_lowercase();
    !id.is_empty() && EXCLUDED_APPS.iter().any(|a| if a.ends_with('.') { id.starts_with(a) } else { id == *a || (!a.ends_with(".exe") && id.starts_with(&format!("{a}."))) })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn addresses_are_recognised() {
        for t in ["https://example.com", "www.google", "me@mail", "/usr/bin", "~/code", "./run", "github.com", "docs.rs", "a.b.io", "example.org/path"] { assert!(looks_like_address(t), "{t}") }
        // Thai typed on the English layout uses . / @ : as letters
        for t in ["l;ylfu", "w,j", "ihk.", "g.or", "k.shf", "e.g.", "g-hk.0", "sy;.0", "mt.sh"] { assert!(!looks_like_address(t), "{t}") }
    }
    #[test] fn terminals_and_editors_are_excluded() {
        for id in ["com.apple.Terminal", "com.microsoft.VSCode", "com.microsoft.VSCodeInsiders", "com.jetbrains.intellij", "dev.zed.Zed", "Code.exe", "WindowsTerminal.exe", "com.todesktop.230313mzl4w4u92"] { assert!(excluded_app(id), "{id}") }
        for id in ["", "com.apple.TextEdit", "com.tinyspeck.slackmacgap", "com.google.Chrome", "notepad.exe", "com.apple.terminalx"] { assert!(!excluded_app(id), "{id}") }
    }
    #[test] fn levels_get_stricter() {
        let [c, b, e] = AutoLevel::ALL.map(AutoLevel::rule);
        assert!(c.fwd >= b.fwd && b.fwd >= e.fwd && c.rev >= b.rev && b.rev >= e.rev && c.min_keys >= b.min_keys && b.min_keys >= e.min_keys);
        assert_eq!(AutoLevel::default(), AutoLevel::Cautious);
    }
}
