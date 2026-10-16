//! Platform-independent typing engine: per-word key buffer -> decisions -> actions for the platform layer.
//! The platform layer feeds `Key`s (already mapped to the US-QWERTY character the key would produce), and executes the
//! returned `Action`s (delete N chars + inject text, show/hide the suggestion bubble). Fully unit-testable on Linux.
use crate::backspace::{self, DeleteUnit};
use crate::bridge::BubbleLayout;
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::rewrite::{self, Captured, Job, Preset, Reply, ReqId, Run};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use typethai_core::{completer::{thai_tail, Bigrams, Completer, Completions, Context, WordMemory}, slip::Slips, reverse::Reverse, corrector::Corrector, detector::Detector, keymap::en_to_th, personal::{Feedback, PersonalModel}, reverse::th_text_to_keys, TokenDecision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary { Space, Enter, Tab }
impl Boundary { pub fn text(self) -> &'static str { match self { Boundary::Space => " ", Boundary::Enter => "\n", Boundary::Tab => "\t" } } }
/// English spell checker (the OS one; see `platform::spell`).
pub trait Speller {
    /// None when `word` is spelled right (or cannot be checked); else the guesses, best first.
    fn guesses(&self, word: &str) -> Option<Vec<String>>;
    /// Words that start with `prefix`, best first (empty where the OS has no completion API).
    fn completions(&self, _prefix: &str) -> Vec<String> { vec![] }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey { Undo, ForceConvert, Guess, Accept(usize), Dismiss, TogglePause,
    /// Ctrl+Opt/Alt+R: options for the selected text (other layout, spelling, AI rewrite), or with nothing selected for
    /// the word before the caret.
    Rewrite }
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    /// A printable key, as the US-QWERTY char it produces (e.g. `;` for the ว key).
    Char(char),
    Backspace,
    /// Option+Backspace (macOS) / Ctrl+Backspace (Windows): delete the previous word.
    WordDelete,
    /// Cmd+Backspace (macOS): delete to the start of the line.
    LineDelete,
    Boundary(Boundary),
    /// Arrows, Home/End, PgUp/PgDn, Delete, Esc, or any shortcut with Ctrl/Alt/Cmd: resets the buffer.
    Nav,
    Hotkey(Hotkey),
    /// Tab pressed (no modifiers), auto-repeats included. Over the bubble it starts a `TabChord`, otherwise it is a word
    /// boundary.
    Tab,
    /// Tab released. Ends a `TabChord`: applies the selected row unless a digit already applied one.
    TabUp,
    /// Shift+Up / Shift+Down: move the bubble selection while it is visible, otherwise plain navigation (text selection).
    SelectPrev, SelectNext,
    /// Esc: dismisses a visible bubble (swallowed), otherwise navigation.
    Esc,
    /// Ctrl+1..3 (Control alone): applies that row while the bubble is up and `Options::pick_ctrl` is on, otherwise
    /// navigation.
    PickRow(usize),
}
/// Which kind of OS input source fits the text just inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputLang { Thai, Latin }
pub fn lang_of(s: &str) -> InputLang { if s.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)) { InputLang::Thai } else { InputLang::Latin } }
/// The input source fitting inserted text; None when it has no letters of either script (only a space or punctuation
/// changed), and the current source stays.
fn layout_of(insert: &str) -> Option<InputLang> {
    let l = lang_of(insert);
    (l == InputLang::Thai || insert.chars().any(|c| c.is_ascii_alphabetic())).then_some(l)
}

/// Bubble selection: index into the visible items, wrapping. Pure state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection { pub len: usize, pub idx: usize }
impl Selection {
    pub fn new(len: usize) -> Self { Selection { len, idx: 0 } }
    pub fn next(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + 1) % self.len } self.idx }
    pub fn prev(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + self.len - 1) % self.len } self.idx }
}
/// Tab held over the bubble. Tab alone applies the selected row when released; Tab+1..3 applies that row instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TabChord {
    /// Tab is up, or went down with no bubble (a plain Tab).
    #[default] Up,
    /// Down over the bubble with nothing applied yet: the release applies the selected row.
    Armed,
    /// Still down after a digit picked a row: the release and auto-repeats do nothing.
    Spent,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Send `delete` Backspaces (counted in the engine's `delete_unit`), then type `insert`; then re-send the swallowed
    /// boundary key.
    Replace { delete: usize, insert: String, then: Option<Boundary> },
    ShowBubble(Vec<BubbleItem>),
    HideBubble,
    /// Move the bubble highlight to this row.
    Select(usize),
    /// Switch the OS input source to fit the text just inserted (platform: no-op if already that kind).
    SwitchInput(InputLang),
    /// Hand to the rewrite worker. Answers come back through `Engine::on_reply`.
    Rewrite(Job),
}
#[derive(Clone, Debug, PartialEq)]
pub struct BubbleItem { pub label: String, pub kind: BubbleKind }
#[derive(Clone, Debug, PartialEq)]
pub enum BubbleKind {
    /// Mid-word live suggestion: replace the typed prefix with its Thai conversion (then the OS switches to Thai).
    /// `prev` > 0 (reverse phrase being edited): the `prev` committed words before the buffer are replaced too.
    Live { typed: String, thai: String, prev: usize },
    UndoConversion, Convert { typed: String, thai: String }, Typo { word: String },
    /// Reverse mode, after a boundary: flip the last word plus `k` preceding Thai-looking words back to English.
    ConvertPrev { k: usize },
    /// Guess (double Tab / hotkey): replace the last `k` committed words, and the buffer `typed` when it is Some, with
    /// `targets` (one per word, in order).
    Guess { k: usize, typed: Option<String>, targets: Vec<String> },
    /// Word completion: replace what the buffer `typed` put on screen with `insert` (a Thai run keeps its head).
    Complete { typed: String, insert: String },
    /// While rewrite rows are on the bubble they are the rewrite session's whole state.
    Rewrite(RewriteRow) }
/// Why an option is offered: it was typed on the other keyboard layout and becomes text in this language, or it fixes
/// a typo. The bubble shows options grouped by it, conversions first, under a tab that names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason { Layout(InputLang), Typo }
impl BubbleItem {
    /// None for finishing the word, the AI rows and notices: what the bubble offers anyway, not a fix. Undoing a
    /// conversion converts back; a Guess that also fixes a typo is still mainly a conversion.
    pub fn reason(&self) -> Option<Reason> {
        match &self.kind {
            BubbleKind::Live { .. } | BubbleKind::Convert { .. } | BubbleKind::ConvertPrev { .. } | BubbleKind::Guess { .. } | BubbleKind::UndoConversion =>
                Some(Reason::Layout(lang_of(&self.label))),
            BubbleKind::Typo { .. } => Some(Reason::Typo),
            BubbleKind::Rewrite(RewriteRow::Fixed(why, _)) => Some(*why),
            BubbleKind::Complete { .. } | BubbleKind::Rewrite(RewriteRow::Preset(..) | RewriteRow::Busy(_) | RewriteRow::Result(_) | RewriteRow::Notice) => None,
        }
    }
}
/// The visible rewrite phases; a bubble holds rows of one phase.
#[derive(Clone, Debug, PartialEq)]
pub enum RewriteRow {
    /// Choosing: one row per `Preset::ALL`, each carrying the captured selection.
    Preset(Preset, String),
    /// Pending: "Rewriting…" until `Reply::Rewritten` for this id.
    Busy(ReqId),
    /// Result: Tab pastes this full text (the label may be a preview).
    Result(String),
    /// A local fix of the selection, made on this computer: Tab pastes this full text, the selection in the other
    /// layout or with its typos fixed, as `Reason` says.
    Fixed(Reason, String),
    /// Nothing to offer / no key / failure; the label is the message.
    Notice,
}
/// What the key handler should do with the original key event.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome { pub swallow: bool, pub actions: Vec<Action> }
impl Outcome { fn pass() -> Self { Outcome { swallow: false, actions: vec![] } } }

#[derive(Clone, Debug)]
/// `typed` = the keys (US chars); `orig` = what those keys put on screen (keys themselves, or their Thai reading when the
/// Thai layout was active = `rev`); `converted` = what we replaced it with.
/// `with_prev`: how many preceding words this conversion also flipped (undo restores them too).
/// `boundary` is None for a word converted mid-word (Guess on the buffer): what is typed next follows it directly.
struct Committed { typed: String, orig: String, converted: Option<String>, boundary: Option<Boundary>, rev: bool, with_prev: usize,
    /// Extra boundaries typed after this word (more spaces / Enters) while it is still the last word.
    extra: String,
    /// Already given to the word memory (see `settle`).
    learned: bool }
impl Committed {
    /// The word as it is on screen now.
    fn shown(&self) -> &str { self.converted.as_deref().unwrap_or(&self.orig) }
    /// Everything after the word on screen: its boundary plus any extra spaces/newlines.
    fn after(&self) -> String { format!("{}{}", self.boundary.map_or("", Boundary::text), self.extra) }
}

pub struct Options {
    pub auto_convert: bool, pub switch_input: bool, pub suggest_band: f64, pub typo_suggestions: bool, pub context_words: usize,
    /// Live (mid-word) bubble: show when the current prefix's flip probability > `live_show` and the prefix has at
    /// least `live_min_len` keys. `live_show` matches the post-space suggestion band, and `live_stable` is 1, so the
    /// bubble appears on the keystroke that crosses the band. Space is not required. Two keys are too few: English
    /// prefixes like `wo` / `fu` read as ไน / ดี with p > 0.95, so 2-key words are offered after Space instead.
    pub live: bool, pub live_show: f64, pub live_min_len: usize, pub live_stable: usize,
    /// Reverse direction (English typed on the Thai layout): auto-convert at a boundary above `rev_auto`, live bubble
    /// above `rev_live` (calibrated on dev, see README v11).
    pub rev_auto: f64, pub rev_live: f64,
    /// A word left as Thai is offered together with the next word (in the live bubble, and again after Space)
    /// when that next word is confident and this word's probability with that right context is above `rev_prev`.
    pub rev_prev: f64,
    /// Word completion: `Typing` shows it on a keystroke when the top word's share of the prefix's frequency is at
    /// least `complete_conf`; `Pause` shows it, ungated, once no key came for `complete_idle`.
    pub complete: CompleteMode, pub complete_conf: f64, pub complete_idle: Duration,
    /// Learn the words the user finishes and rank completions by them; off neither learns nor uses what was learned.
    pub learn_words: bool,
    pub accept: AcceptKeys,
    /// Ctrl+1..3 picks a bubble row.
    pub pick_ctrl: bool,
    /// How the keys typed may differ from the word meant, for completion and spelling rows; None reads them as typed.
    pub slips: Option<Slips>,
    /// How the bubble lays out word options; the engine only stores it for Settings and the platform layer.
    pub bubble: BubbleLayout,
}
impl Default for Options { fn default() -> Self { Options { auto_convert: false, switch_input: true, suggest_band: 0.5, typo_suggestions: true, context_words: 8,
    live: true, live_show: LIVE_SHOW, live_min_len: 3, live_stable: 1, rev_auto: REV_AUTO, rev_live: REV_LIVE, rev_prev: REV_PREV,
    complete: CompleteMode::default(), complete_conf: 0.4, complete_idle: Duration::from_millis(COMPLETE_IDLE_DEFAULT_MS), learn_words: true, accept: AcceptKeys::Tab, pick_ctrl: false, slips: Some(Slips::default()), bubble: BubbleLayout::default() } } }
/// Pause lengths Settings offers, shortest first. The default sits at the second step: above the gap between
/// keys in fluent typing, so the bubble does not flash mid-word, and half the old 500 ms.
pub const COMPLETE_IDLE_CHOICES_MS: [u64; 4] = [150, 250, 400, 600];
pub const COMPLETE_IDLE_DEFAULT_MS: u64 = 250;
/// When word completions appear in the bubble.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum CompleteMode { Off, #[default] Pause, Typing }
/// What a key or a pause asks of the completion rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// A key in `CompleteMode::Typing`: rows nobody waited for, so the completed part needs 3 chars and `complete_conf`.
    Typing,
    /// A key while rows from a pause are up: they follow the word in place instead of going until the next pause.
    Refresh,
    /// The pause in `CompleteMode::Pause`: 2 chars, ungated, and the spell checker tops the rows up.
    Pause,
}
impl Ask {
    fn min_len(self) -> usize { if self == Ask::Typing { 3 } else { 2 } }
    fn gated(self) -> bool { self == Ask::Typing }
    /// The spell checker takes ~0.85 ms a call, and a slow tap callback gets the tap disabled: keys never call it.
    fn spells(self) -> bool { self == Ask::Pause }
}
/// Keys that apply the highlighted bubble row. Tab (and Tab held + 1..3) always does. `TabOrSpace`: Space also applies
/// a live conversion row, then types the space; over any other row Space is a plain word boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum AcceptKeys { #[default] Tab, TabOrSpace }
/// Same band as the suggestion shown after Space (`suggest_band`), so a word that would be offered at the boundary
/// is offered while it is still being typed.
pub const LIVE_SHOW: f64 = 0.5;
/// Band for a forward conversion that is not a dictionary word (a prefix, a phrase, or nonsense); the old live
/// threshold tuned on dev with `--eval-live`.
pub const STRICT_BAND: f64 = 0.9;
/// Reverse auto-convert threshold: false flips on dev Thai (wiki, chat, typo'd) <= 0.1%.
pub const REV_AUTO: f64 = 0.98;
/// Reverse live-bubble threshold.
pub const REV_LIVE: f64 = 0.98;
/// Retroactive previous-word flip threshold (only when the next word auto-flips).
pub const REV_PREV: f64 = 0.8;

pub struct Engine {
    /// Reverse-direction model; None disables it (the engine then ignores keys while Thai is active).
    pub rev: Option<Reverse>,
    /// The OS input source is Thai: words are scored in the reverse direction.
    thai_active: bool,
    pub det: Detector, pub personal: PersonalModel, pub corrector: Option<Corrector>, pub opts: Options,
    pub paused: bool,
    word: String,
    /// The last key was a Tab that inserted itself: a second Tab runs Guess.
    tab_pending: bool,
    tab: TabChord,
    /// The bubble was opened by ⌃⌥R and offers something to apply: Enter applies the highlighted row, like Tab.
    fixing: bool,
    line: Vec<Committed>,
    /// Index of the first word of the current line in `line` (after Enter the last word is kept so Tab still applies).
    line_start: usize,
    /// Items currently on the bubble (accept by index).
    bubble: Vec<BubbleItem>,
    sel: Selection,
    live_streak: usize,
    /// The visible bubble is a live (mid-word) suggestion.
    live: bool,
    /// Words in the offered phrase (current buffer or last committed word, plus earlier words); 0 = nothing pending.
    /// Lets Backspace keep offering what is left of a phrase.
    phrase: usize,
    pub events_since_save: usize,
    /// English key tokens the user does not want translated (`ok` stays as typed, not `นา`).
    pub never: BTreeSet<String>,
    /// English spelling suggestions for words left as typed; None disables them.
    pub speller: Option<Box<dyn Speller + Send>>,
    /// prefs.json "rewrite". The API key is not here; only the rewrite worker reads it.
    pub rewrite: rewrite::Config,
    /// prefs.json "check_updates": look for a new release at launch and every few hours. Read by `update`.
    pub check_updates: bool,
    /// The selection read in flight. Only Some while the bubble is empty (the one rewrite phase with no rows).
    capturing: Option<ReqId>,
    last_req: ReqId,
    /// Word completion sources; None disables that language.
    pub thai_comp: Option<Completer>, pub en_comp: Option<Completer>,
    /// Corpus word pairs that rank completions after the previous word; None ranks without them.
    pub thai_bigrams: Option<Bigrams>, pub en_bigrams: Option<Bigrams>,
    /// The words this user finished (words.json). Shared with the save thread, which serialises a snapshot; learning
    /// while a save is still running copies it first.
    pub words: Arc<WordMemory>,
    /// The last Char / Backspace, until `on_idle` has handled the pause after it.
    last_key: Option<Instant>,
    /// Esc / Dismiss: no completions until the next word boundary.
    comp_muted: bool,
    /// What one Backspace removes in the focused app; the platform sets it when another app gets the keys.
    pub delete_unit: DeleteUnit,
}

impl Engine {
    pub fn new(det: Detector, personal: PersonalModel, corrector: Option<Corrector>, opts: Options) -> Self {
        Engine { rev: None, thai_active: false, det, personal, corrector, opts, paused: false, word: String::new(), tab_pending: false, tab: TabChord::Up, line: vec![], line_start: 0, bubble: vec![], sel: Selection::default(), live_streak: 0, live: false, phrase: 0, events_since_save: 0, never: BTreeSet::new(), speller: None, rewrite: rewrite::Config::default(), check_updates: true, capturing: None, last_req: ReqId::default(), thai_comp: None, en_comp: None, thai_bigrams: None, en_bigrams: None, words: Arc::default(), last_key: None, comp_muted: false, delete_unit: DeleteUnit::default(), fixing: false }
    }
    /// Completion sources from the model; English only when it carries English word frequencies (TTM section 11).
    pub fn load_completers(&mut self, m: &mut typethai_core::ModelFiles) {
        self.thai_comp = Some(m.thai_completer());
        self.en_comp = (!m.english_freq.is_empty()).then(|| m.english_completer());
        // taken, not cloned: loaded pairs take tens of MB
        self.thai_bigrams = Some(std::mem::take(&mut m.th_bigrams)).filter(|b| !b.is_empty());
        self.en_bigrams = Some(std::mem::take(&mut m.en_bigrams)).filter(|b| !b.is_empty());
    }
    pub fn buffer(&self) -> &str { &self.word }
    pub fn thai_active(&self) -> bool { self.thai_active }
    /// Platform reports the active input source before each key. A change starts a new word (the line and a visible
    /// bubble are kept, so undo / Tab still work right after a conversion switched the layout).
    pub fn set_thai_active(&mut self, on: bool) { if on != self.thai_active { self.thai_active = on; self.word.clear(); self.live_streak = 0; } }
    /// What `typed` keys put on screen in the current mode, and what a conversion would replace it with.
    fn screen_of(&self, typed: &str) -> String { if self.thai_active { en_to_th(typed) } else { typed.to_string() } }
    /// Replace `old`, the text right before the caret, with `new`; what they share stays on screen.
    fn replace(&self, old: &str, new: &str, then: Option<Boundary>) -> Action {
        let (delete, insert) = backspace::edit(old, new, self.delete_unit);
        Action::Replace { delete, insert: insert.to_string(), then }
    }
    /// Reverse line probabilities for the reverse-mode tail of the line plus `last` (None without a model).
    fn rev_line_probs_with(&self, last: &str) -> Option<Vec<f64>> {
        let r = self.rev.as_ref()?;
        let tail: Vec<&str> = self.line[self.line_start.min(self.line.len())..].iter().rev().take_while(|c| c.rev).take(self.opts.context_words).map(|c| c.typed.as_str()).collect::<Vec<_>>().into_iter().rev().chain([last]).collect();
        Some(r.line_probs(&r.decide_line(&self.det, &tail)))
    }
    /// How many preceding unconverted reverse words belong with `typed` (e.g. "i" once "need" follows it).
    fn rev_prev_count(&self, typed: &str) -> usize {
        let Some(probs) = self.rev_line_probs_with(typed) else { return 0 };
        let mut k = 0;
        while k < 3 && k + 1 < probs.len() && self.line.len() > self.line_start + k {
            let c = &self.line[self.line.len() - 1 - k];
            let key = c.typed.to_lowercase();
            if !c.rev || c.converted.is_some() || self.never.contains(&key) || probs[probs.len() - 2 - k] <= self.opts.rev_prev { break }
            k += 1;
        }
        k
    }
    /// Reverse words to put in front of the live bubble. Forward suggestions stay on the current word.
    fn live_prev(&self) -> usize {
        if !self.thai_active { return 0 }
        let typed = self.word.clone();
        self.rev_prev_count(&typed)
    }
    /// Reverse P(English intended) for `last` given the reverse-mode words before it on the line.
    pub fn rev_prob(&self, last: &str) -> (f64, bool) {
        let Some(r) = &self.rev else { return (0.0, false) };
        let tail: Vec<&str> = self.line[self.line_start.min(self.line.len())..].iter().rev().take_while(|c| c.rev).take(self.opts.context_words).map(|c| c.typed.as_str()).collect::<Vec<_>>().into_iter().rev().chain([last]).collect();
        let ds = r.decide_line(&self.det, &tail); let ok = ds.last().is_some_and(|d| d.convertible);
        (*r.line_probs(&ds).last().unwrap_or(&0.0), ok)
    }
    pub fn bubble_visible(&self) -> bool { !self.bubble.is_empty() }
    pub fn selected(&self) -> usize { self.sel.idx }
    /// Keys the platform must route to the engine even when it would otherwise bypass it (Thai layout active, secure
    /// field check): the bubble controls while the bubble is up, the keys of a held Tab, and Space when it applies a live
    /// row (a layout change clears the buffer first, so a stale row never pulls Space in).
    pub fn is_bubble_key(&self, k: &Key) -> bool {
        (self.bubble_visible() && matches!(k, Key::Tab | Key::SelectPrev | Key::SelectNext | Key::Esc))
            || (self.tab != TabChord::Up && matches!(k, Key::Tab | Key::TabUp | Key::Char('1'..='3')))
            || (*k == Key::Boundary(Boundary::Space) && self.space_accepts())
            || (matches!(k, Key::PickRow(_)) && self.picks_rows())
            || (*k == Key::Boundary(Boundary::Enter) && self.fixing && self.bubble_visible())
    }
    /// Ctrl+1..3 applies a row: the setting is on and there is a bubble to pick from.
    fn picks_rows(&self) -> bool { self.opts.pick_ctrl && self.bubble_visible() }
    /// Space applies the highlighted row: with `AcceptKeys::TabOrSpace`, and only when that row is a live conversion.
    fn space_accepts(&self) -> bool {
        self.opts.accept == AcceptKeys::TabOrSpace
            && matches!(self.bubble.get(self.sel.idx), Some(BubbleItem { kind: BubbleKind::Live { typed, prev, .. }, .. }) if self.live_current(typed, *prev))
    }
    /// A live row still matches the text on screen: the buffer it was offered for, and the `prev` words before it.
    fn live_current(&self, typed: &str, prev: usize) -> bool { self.word == typed && self.line.len() >= prev }
    /// Focus change, mouse click, secure field, layout change, any non-bubble key during a rewrite: forget everything.
    pub fn reset(&mut self) -> Vec<Action> {
        self.settle();
        self.word.clear(); self.tab_pending = false; self.tab = TabChord::Up; self.capturing = None; self.line.clear(); self.line_start = 0; self.live_streak = 0; self.live = false;
        self.last_key = None; self.comp_muted = false;
        if self.bubble.is_empty() { vec![] } else { self.bubble.clear(); vec![Action::HideBubble] }
    }
    fn decisions(&self, last: &str) -> Vec<TokenDecision> {
        let start = self.line.len().saturating_sub(self.opts.context_words).max(self.line_start.min(self.line.len()));
        let mut toks: Vec<&str> = self.line[start..].iter().filter(|c| !c.rev).map(|c| c.typed.as_str()).collect(); toks.push(last);
        self.det.decide_line(&toks)
    }
    /// Personal flip probability for `last` given the line context, plus its decisions (for feedback).
    pub fn prob(&self, last: &str) -> (f64, Vec<TokenDecision>) {
        let ds = self.decisions(last); let p = *self.personal.line_probs(&ds).last().unwrap_or(&0.0); (p, ds)
    }
    /// What completion and spelling rows in `lang` are ranked by: `prev` (the word before, when it is in `lang`), the
    /// corpus pairs, the user's words and the slips.
    pub(crate) fn context<'a>(&'a self, prev: Option<&'a str>, lang: InputLang) -> Context<'a> {
        let bigrams = match lang { InputLang::Thai => self.thai_bigrams.as_ref(), InputLang::Latin => self.en_bigrams.as_ref() };
        let unknown = Some(match lang { InputLang::Thai => &self.det.thai, InputLang::Latin => &self.det.english });
        Context { prev: prev.filter(|p| lang_of(p) == lang), bigrams, memory: self.opts.learn_words.then_some(&*self.words), slips: self.opts.slips, unknown }
    }
    /// English spelling suggestions for a finished word (`ths` -> this) the spell checker flags: its guesses and, with
    /// slips, the words one slip away, ranked by the completion model after `prev`. Punctuation around the word is kept.
    pub(crate) fn en_items(&self, typed: &str, prev: Option<&str>) -> Vec<BubbleItem> {
        let Some(sp) = &self.speller else { return vec![] };
        if !self.opts.typo_suggestions { return vec![] }
        let core = typed.trim_matches(|c: char| !c.is_ascii_alphabetic());
        if core.len() < 2 || !core.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') { return vec![] }
        let at = typed.find(core).unwrap_or(0);
        let (pre, post) = (&typed[..at], &typed[at + core.len()..]);
        let Some(guesses) = sp.guesses(core) else { return vec![] };
        let fixes = match (self.opts.slips, &self.en_comp) {
            (Some(_), Some(comp)) => comp.correct(core, 3, self.context(prev, InputLang::Latin), &guesses),
            _ => guesses.into_iter().filter(|g| g != core).take(3).collect(),
        };
        fixes.into_iter().map(|g| { let word = format!("{pre}{g}{post}"); BubbleItem { label: word.clone(), kind: BubbleKind::Typo { word } } }).collect()
    }
    /// Thai typo fixes for a word the corrector does not know: its suggestions and, with slips, the words one slip
    /// away, ranked by the completion model after `prev`. A word it knows gets at most its normalised spelling.
    pub(crate) fn typo_items(&self, thai: &str, prev: Option<&str>) -> Vec<BubbleItem> {
        let Some(c) = &self.corrector else { return vec![] };
        if !self.opts.typo_suggestions { return vec![] }
        let r = c.suggest(thai, TYPO_GUESSES, &self.det.thai);
        if r.in_dict && r.apply.is_none() { return vec![] }
        let item = |w: String| BubbleItem { label: w.clone(), kind: BubbleKind::Typo { word: w } };
        if let (false, Some(_), Some(comp)) = (r.in_dict, self.opts.slips, &self.thai_comp) {
            let guesses: Vec<String> = r.suggestions.iter().map(|s| s.word.clone()).collect();
            let fixes = comp.correct(&r.normalized, 3, self.context(prev, InputLang::Thai), &guesses);
            if !fixes.is_empty() { return fixes.into_iter().filter(|w| w != thai).map(item).collect() }
        }
        let mut out: Vec<BubbleItem> = r.apply.iter().filter(|a| a.as_str() != thai).cloned().map(item).collect();
        for s in r.suggestions { if out.len() >= 3 { break } if !out.iter().any(|o| o.label == s.word) { out.push(item(s.word)) } }
        out
    }
    pub fn live_visible(&self) -> bool { self.live && self.bubble_visible() }
    fn show(&mut self, mut items: Vec<BubbleItem>, acts: &mut Vec<Action>) {
        self.fixing = false;
        // grouped as drawn, so a row's index is its place on screen for Tab, the digits and clicks
        items.sort_by_key(|i| match i.reason() { Some(Reason::Layout(_)) => 0, Some(Reason::Typo) => 1, None => 2 });
        self.live = matches!(items.first(), Some(BubbleItem { kind: BubbleKind::Live { .. }, .. }));
        let act = if items.is_empty() { self.phrase = 0; if self.bubble.is_empty() { return } self.bubble.clear(); Action::HideBubble }
            else { self.sel = Selection::new(items.len()); self.bubble = items.clone(); Action::ShowBubble(items) };
        // one bubble update per key (the live row and the completions under it are worked out one after the other)
        acts.retain(|a| !matches!(a, Action::ShowBubble(_) | Action::HideBubble));
        acts.push(act);
    }
    fn feedback(&mut self, typed: &str, kind: Feedback) {
        let ds = self.decisions(typed); let i = ds.len() - 1; self.personal.feedback(&ds, i, kind); self.events_since_save += 1;
    }

    pub fn on_key(&mut self, k: Key) -> Outcome {
        let mut o = self.on_key_inner(k);
        // after inserting text, switch the OS input source to match it (Thai after a conversion, Latin after an undo)
        if self.opts.switch_input {
            if let Some(Action::Replace { insert, .. }) = o.actions.iter().rev().find(|a| matches!(a, Action::Replace { .. })) {
                if let Some(l) = layout_of(insert) { o.actions.push(Action::SwitchInput(l)); }
            }
        }
        o
    }
    fn on_key_inner(&mut self, k: Key) -> Outcome {
        if let Key::Hotkey(Hotkey::TogglePause) = k { self.paused = !self.paused; let a = self.reset(); return Outcome { swallow: true, actions: a } }
        if self.paused { return Outcome::pass() }
        if let Some(o) = self.tab_chord(&k) { return o }
        let k = match k { Key::PickRow(i) if self.picks_rows() => return self.accept_held(i), Key::PickRow(_) => Key::Nav, k => k };
        if self.rewriting() && !self.rewrite_keeps(&k) { let mut a = self.reset(); let mut o = self.on_key_inner(k); a.append(&mut o.actions); o.actions = a; return o }
        match k {
            Key::Char(c) => {
                self.tab_pending = false;
                self.resume_glued();
                // a new word starts: the bubble was about the previous word and can no longer be applied
                let mut a = vec![]; if self.word.is_empty() { self.settle(); self.show(vec![], &mut a); self.live_streak = 0; }
                // a Guess or the Ctrl+Opt+R rows were made for the word as it was
                else if !matches!(self.bubble.first(), None | Some(BubbleItem { kind: BubbleKind::Live { .. } | BubbleKind::Complete { .. }, .. })) { self.show(vec![], &mut a); }
                let up = self.completions_up();
                self.word.push(c); self.last_key = Some(Instant::now());
                self.live_update(&mut a);
                self.update_completions(false, &up, &mut a);
                Outcome { swallow: false, actions: a }
            }
            Key::Tab => if let Some(item) = self.tab_pending.then(|| self.guess()).flatten() {
                // second Tab in a row: show the proposal (a third Tab applies it); with nothing to propose it is a tab
                self.tab_pending = false;
                let mut a = vec![]; self.show(vec![item], &mut a);
                Outcome { swallow: true, actions: a }
            } else {
                let o = self.on_boundary(Boundary::Tab);
                self.tab_pending = true;
                o
            },
            Key::TabUp => Outcome::pass(),
            Key::SelectPrev | Key::SelectNext => if self.bubble_visible() {
                let i = if k == Key::SelectNext { self.sel.next() } else { self.sel.prev() };
                Outcome { swallow: true, actions: vec![Action::Select(i)] }
            } else { Outcome { swallow: false, actions: self.reset() } },
            Key::Esc => if self.bubble_visible() { let mut a = vec![]; self.dismiss(&mut a); Outcome { swallow: true, actions: a } }
                else { Outcome { swallow: false, actions: self.reset() } },
            Key::Backspace => { self.tab_pending = false; self.last_key = Some(Instant::now()); self.on_backspace() }
            Key::WordDelete => self.on_word_delete(),
            // Cmd+Backspace deletes the whole line: nothing of it is left to offer
            Key::LineDelete => { self.tab_pending = false; Outcome { swallow: false, actions: self.reset() } }
            Key::Nav | Key::PickRow(_) => { self.tab_pending = false; Outcome { swallow: false, actions: self.reset() } }
            Key::Boundary(Boundary::Space) if self.space_accepts() => self.accept_rolled(k),
            Key::Boundary(Boundary::Enter) if self.fixing && self.bubble_visible() => {
                let mut o = self.on_hotkey(Hotkey::Accept(self.sel.idx)); o.swallow = true; o
            }
            Key::Boundary(b) => self.on_boundary(b),
            Key::Hotkey(h) => self.on_hotkey(h),
        }
    }
    /// The keys of a held Tab (see `TabChord`); None for keys handled as usual.
    fn tab_chord(&mut self, k: &Key) -> Option<Outcome> {
        let swallowed = Outcome { swallow: true, actions: vec![] };
        Some(match (self.tab, k) {
            (TabChord::Up, Key::Tab) if self.bubble_visible() => { self.tab = TabChord::Armed; self.tab_pending = false; swallowed }
            (TabChord::Up, _) => return None,
            (_, Key::Tab) => swallowed,
            (TabChord::Armed, Key::TabUp) => { self.tab = TabChord::Up; self.accept_held(self.sel.idx) }
            (TabChord::Spent, Key::TabUp) => { self.tab = TabChord::Up; swallowed }
            // a digit is never typed while Tab is down; past the last row it does nothing
            (_, Key::Char(d @ '1'..='3')) => { self.tab = TabChord::Spent; self.accept_held(*d as usize - '1' as usize) }
            // bubble controls work as usual: the release applies whatever is then selected, if the bubble is still up
            (_, Key::SelectPrev | Key::SelectNext | Key::Esc | Key::Hotkey(_)) => return None,
            (TabChord::Armed, Key::Char(_) | Key::Boundary(_)) => { self.tab = TabChord::Up; self.accept_rolled(k.clone()) }
            // caret moves and deletions drop the held Tab: the bubble no longer matches the text
            _ => { self.tab = TabChord::Up; return None }
        })
    }
    fn accept_held(&mut self, i: usize) -> Outcome { let mut o = self.on_hotkey(Hotkey::Accept(i)); o.swallow = true; o }
    /// A key typed before the held Tab came up (fast typing rolls Tab into the next key): the Tab counts as released
    /// first. The key is folded into the replacement instead of passing through, which would put it on screen before
    /// the injected text. Space accepting a live row (`AcceptKeys::TabOrSpace`) goes the same way.
    fn accept_rolled(&mut self, k: Key) -> Outcome {
        let mut acts = self.accept_held(self.sel.idx).actions;
        let mut o;
        if let Some(Action::Replace { insert, then, .. }) = acts.iter_mut().rev().find(|a| matches!(a, Action::Replace { .. })) {
            // the key arrives in the layout the replacement switches to
            if let Some(l) = layout_of(insert).filter(|_| self.opts.switch_input) { self.set_thai_active(l == InputLang::Thai) }
            o = self.on_key_inner(k.clone());
            if !o.swallow {
                match k { Key::Boundary(b) => *then = Some(b), Key::Char(c) => insert.push_str(&self.screen_of(&c.to_string())), _ => unreachable!() }
                o.swallow = true;
            }
        } else { o = self.on_key_inner(k) }
        acts.append(&mut o.actions); o.actions = acts; o
    }
    /// Re-score the current word buffer (with line context) after a keystroke; show/update/hide the live bubble.
    /// Is the current buffer confident enough for the live bubble (same thresholds as typing)?
    /// True when `word` is on the don't-translate list (matched as lowercased English keys).
    pub fn blocked(&self, word: &str) -> bool {
        let k = word.to_lowercase();
        !k.is_empty() && self.never.contains(&k)
    }
    pub fn apply_settings(&mut self, cmd: SettingsCmd) -> bool {
        match cmd {
            SettingsCmd::AddNever(w) => self.insert_never(&w),
            SettingsCmd::RemoveNever(w) => self.remove_never(&w),
            SettingsCmd::Forget(w) => self.forget_learned(&w),
            SettingsCmd::ResetLearning => { self.personal.reset(); self.events_since_save += 1; true }
            SettingsCmd::SetCompletion(m) => { let changed = self.opts.complete != m; self.opts.complete = m; changed }
            SettingsCmd::SetCompleteIdle(d) => { let changed = self.opts.complete_idle != d; self.opts.complete_idle = d; changed }
            SettingsCmd::SetLearnWords(on) => { let changed = self.opts.learn_words != on; self.opts.learn_words = on; changed }
            SettingsCmd::SetAccept(a) => { let changed = self.opts.accept != a; self.opts.accept = a; changed }
            SettingsCmd::SetPickCtrl(on) => { let changed = self.opts.pick_ctrl != on; self.opts.pick_ctrl = on; changed }
            SettingsCmd::SetBubbleLayout(l) => { let changed = self.opts.bubble != l; self.opts.bubble = l; changed }
            SettingsCmd::ForgetWords => { self.words = Arc::default(); self.events_since_save += 1; true }
            SettingsCmd::SetRewrite(c) => { let changed = self.rewrite != c; self.rewrite = c; changed }
            SettingsCmd::SetCheckUpdates(on) => { let changed = self.check_updates != on; self.check_updates = on; changed }
        }
    }
    pub fn settings_snapshot(&self) -> SettingsSnap {
        SettingsSnap { never: self.never.iter().cloned().collect(), learned: self.personal.learned(), completion: self.opts.complete, complete_idle: self.opts.complete_idle, learn_words: self.opts.learn_words, accept: self.opts.accept, pick_ctrl: self.opts.pick_ctrl, bubble: self.opts.bubble, words_learned: self.words.len(), rewrite: self.rewrite.clone(), check_updates: self.check_updates }
    }
    /// Hide a bubble whose current word was just blocked.
    pub fn hide_if_blocked(&mut self) -> Vec<Action> {
        if !self.blocked(&self.word) || self.bubble.is_empty() { return vec![] }
        let mut acts = vec![];
        self.show(vec![], &mut acts);
        acts
    }
    fn insert_never(&mut self, raw: &str) -> bool {
        let Some(k) = normalize_word(raw) else { return false };
        self.never.insert(k)
    }
    fn remove_never(&mut self, raw: &str) -> bool {
        let Some(k) = normalize_word(raw) else { return false };
        self.never.remove(&k)
    }
    fn forget_learned(&mut self, raw: &str) -> bool {
        if !self.personal.forget(raw) { return false }
        self.events_since_save += 1;
        true
    }
    fn live_confident(&self) -> bool {
        let typed = &self.word;
        if self.blocked(typed) { return false }
        if typed.chars().count() < self.opts.live_min_len { return false }
        if self.thai_active { let (p, ok) = self.rev_prob(typed); ok && p > self.opts.rev_live }
        else { let (p, ds) = self.prob(typed); ds.last().is_some_and(|d| d.valid) && p > self.band(&en_to_th(typed), self.opts.live_show) }
    }
    /// Confidence a forward conversion must clear: `band` when `thai` is a dictionary word, `STRICT_BAND` otherwise.
    /// Short English prefixes and typos (`wo` -> ไน, `riht` -> พร้ะ) score in the middle band and read as nonsense Thai.
    fn band(&self, thai: &str, band: f64) -> f64 {
        if self.corrector.as_ref().is_some_and(|c| c.has(thai)) { band } else { band.max(STRICT_BAND) }
    }
    /// Show the live bubble for the buffer, replacing also the `prev` committed words before it (reverse phrase).
    fn show_live(&mut self, prev: usize, acts: &mut Vec<Action>) {
        let typed = self.word.clone();
        let text = if self.thai_active { typed.clone() } else { en_to_th(&typed) };
        let prev = prev.min(self.line.len().saturating_sub(self.line_start.min(self.line.len())));
        let mut label = String::new();
        for c in &self.line[self.line.len() - prev..] { label += &c.typed; label += &c.after(); }
        label += &text;
        self.show(vec![BubbleItem { label, kind: BubbleKind::Live { typed, thai: text, prev } }], acts);
        self.phrase = prev + 1;
    }
    fn live_update(&mut self, acts: &mut Vec<Action>) {
        if !self.opts.live { return }
        if self.live_confident() {
            self.live_streak += 1;
            // Reverse (Thai layout -> English): confident means >= rev_live (very high), so show immediately
            // instead of waiting for `live_stable` consecutive keystrokes.
            let need = if self.thai_active { 1 } else { self.opts.live_stable };
            if self.live_streak >= need { let prev = self.live_prev(); self.show_live(prev, acts); }
        } else { self.live_streak = 0; if self.live_visible() { self.show(vec![], acts); } }
    }
    /// After an edit that left the buffer (possibly) non-empty: re-offer what is still pending, without re-stabilising.
    fn reoffer(&mut self, was_shown: bool, phrase: usize, up: &[BubbleItem], acts: &mut Vec<Action>) {
        if !self.word.is_empty() {
            if phrase > 1 { self.show_live(phrase - 1, acts) }                    // rest of a reverse phrase: keep it
            else if was_shown && self.live_confident() { self.live_streak = self.opts.live_stable; let prev = self.live_prev(); self.show_live(prev, acts) }
            else if was_shown { self.live_streak = 0; self.show(vec![], acts) }
            else { self.live_update(acts) }                                      // nothing shown: normal stability rules
            self.update_completions(false, up, acts);
        } else if phrase > 0 && was_shown {
            // buffer gone: the words still left on the line (reverse phrase) stay offered
            self.offer_committed(phrase, acts);
        } else { self.show(vec![], acts) }
    }
    /// Esc / Dismiss: hide the bubble; completions stay away until the word ends.
    fn dismiss(&mut self, acts: &mut Vec<Action>) { self.comp_muted = !self.word.is_empty(); self.show(vec![], acts) }
    /// Pause mode: the first timer tick at least `complete_idle` after a key in a word offers its completions.
    pub fn on_idle(&mut self, now: Instant) -> Vec<Action> {
        let mut acts = vec![];
        if self.paused || self.opts.complete != CompleteMode::Pause { return acts }
        if self.last_key.is_none_or(|t| now.saturating_duration_since(t) < self.opts.complete_idle) { return acts }
        self.last_key = None;
        self.update_completions(true, &[], &mut acts);
        acts
    }
    /// The completion rows on the bubble now.
    fn completions_up(&self) -> Vec<BubbleItem> { self.bubble.iter().filter(|b| matches!(b.kind, BubbleKind::Complete { .. })).cloned().collect() }
    /// Re-derive the completion rows under the live row (if it is up); stale ones go. `idle`: the pause has elapsed.
    /// `up`: the rows that were on the bubble before this key.
    fn update_completions(&mut self, idle: bool, up: &[BubbleItem], acts: &mut Vec<Action>) {
        let ask = match (self.opts.complete, idle) {
            (CompleteMode::Typing, false) => Some(Ask::Typing),
            (CompleteMode::Pause, false) if !up.is_empty() => Some(Ask::Refresh),
            (CompleteMode::Pause, true) => Some(Ask::Pause),
            _ => None,
        };
        let rows = ask.map_or_else(Vec::new, |ask| self.completion_rows(ask, if ask == Ask::Refresh { up } else { &[] }));
        if rows.is_empty() && !self.bubble.iter().any(|b| matches!(b.kind, BubbleKind::Complete { .. })) { return }
        let live = self.bubble.first().filter(|b| matches!(&b.kind, BubbleKind::Live { typed, .. } if *typed == self.word)).cloned();
        let items: Vec<BubbleItem> = live.into_iter().chain(rows).collect();
        if items != self.bubble { self.show(items, acts) }
    }
    /// Completions of the buffer, at most 3 rows with the live row. Thai (a Thai layout run, or a wrong-layout word the
    /// live row converts): the run's last word is completed and the rest kept, labelled with that word only. Otherwise
    /// an English word. `ask` sets the length, the gate and the spell checker; rows in `carry` that still extend the
    /// buffer fill what is left.
    fn completion_rows(&self, ask: Ask, carry: &[BubbleItem]) -> Vec<BubbleItem> {
        if self.comp_muted || self.word.is_empty() || self.blocked(&self.word) { return vec![] }
        let live = self.bubble.first().and_then(|b| match &b.kind { BubbleKind::Live { typed, thai, .. } if *typed == self.word => Some(thai.as_str()), _ => None });
        // English typed on the Thai layout: the reverse live row is what was meant
        if self.thai_active && live.is_some() { return vec![] }
        let (min, k) = (ask.min_len(), 3 - usize::from(live.is_some()));
        let gate = |c: &Completions| !ask.gated() || c.confidence >= self.opts.complete_conf;
        // `reading`: the buffer as the rows read it, `head` its part kept in front of each completed word
        let (head, reading, found) = if self.thai_active || live.is_some() {
            let Some(comp) = &self.thai_comp else { return vec![] };
            let run = en_to_th(&self.word);
            let Some(at) = thai_tail(&run, &self.det.th_dict, comp, 1, self.opts.slips.as_ref()).filter(|&at| run.chars().count() - at >= min) else { return vec![] };
            let (head, tail) = run.split_at(run.char_indices().nth(at).map_or(run.len(), |(b, _)| b));
            // the word before the tail in the run, else the one the line shows before the run
            let d = &self.det.th_dict;
            let prev = match d.segment(head).last() { Some(w) => d.has(w).then(|| w.to_string()), None => self.word_before(self.line.len()) };
            (head.to_string(), run.clone(), comp.complete(tail, k, self.context(prev.as_deref(), InputLang::Thai)))
        } else {
            let w = self.word.as_str();
            if w.len() < min || !w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') { return vec![] }
            let Some(comp) = &self.en_comp else { return vec![] };
            let prev = self.word_before(self.line.len());
            let mut found = comp.complete(w, k, self.context(prev.as_deref(), InputLang::Latin));
            if ask.spells() && found.words.len() < k { if let Some(sp) = &self.speller {
                for g in sp.completions(w) {
                    if found.words.len() >= k { break }
                    // keep the typed prefix's case: the checker may capitalise
                    let Some(rest) = g.get(w.len()..).filter(|r| !r.is_empty() && g[..w.len()].eq_ignore_ascii_case(w)) else { continue };
                    let g = format!("{w}{rest}");
                    if !found.words.iter().any(|f| f.eq_ignore_ascii_case(&g)) { found.words.push(g) }
                }
            } }
            (String::new(), w.to_string(), found)
        };
        if !gate(&found) { return vec![] }
        let carried = carry.iter().filter_map(|b| match &b.kind {
            BubbleKind::Complete { insert, .. } if insert.len() > reading.len() && insert.starts_with(&reading) => Some((b.label.clone(), insert.clone())),
            _ => None,
        });
        let mut rows: Vec<BubbleItem> = vec![];
        for (label, insert) in found.words.into_iter().map(|w| (w.clone(), format!("{head}{w}"))).chain(carried) {
            if rows.len() == k { break }
            if Some(insert.as_str()) == live || rows.iter().any(|r| matches!(&r.kind, BubbleKind::Complete { insert: i, .. } if *i == insert)) { continue }
            rows.push(BubbleItem { kind: BubbleKind::Complete { typed: self.word.clone(), insert }, label });
        }
        rows
    }
    /// Offer the last `n` committed words again (word buffer empty, caret right after them).
    fn offer_committed(&mut self, n: usize, acts: &mut Vec<Action>) {
        let avail = self.line.len().saturating_sub(self.line_start.min(self.line.len()));
        let Some(c) = self.line.last().cloned() else { return self.show(vec![], acts) };
        if n == 0 || avail == 0 || c.converted.is_some() || c.rev != self.thai_active { return self.show(vec![], acts) }
        if c.rev {
            let k = (n - 1).min(avail - 1);
            let start = self.line.len() - 1 - k;
            if self.line[start..].iter().any(|w| !w.rev || w.converted.is_some()) { return self.show(vec![], acts) }
            let mut label = String::new(); for w in &self.line[start..] { label += &w.typed; label += &w.after(); }
            let label = label.trim_end().to_string();
            self.show(vec![BubbleItem { label, kind: BubbleKind::ConvertPrev { k } }], acts); self.phrase = k + 1;
        } else {
            let t = en_to_th(&c.typed);
            self.show(vec![BubbleItem { label: t.clone(), kind: BubbleKind::Convert { typed: c.typed.clone(), thai: t } }], acts); self.phrase = 1;
        }
    }
    /// The caret is right after a word inserted mid-word (an accepted completion or live row) that is in the active
    /// layout: a letter or Backspace edits it as the buffer again, as if it had been typed. Its Undo is given up.
    fn resume_glued(&mut self) {
        if !self.word.is_empty() { return }
        let Some(text) = self.line.last().filter(|c| c.boundary.is_none() && c.extra.is_empty()).and_then(|c| c.converted.as_deref()) else { return };
        let keys = if self.thai_active { th_text_to_keys(text) } else { text.is_ascii().then(|| text.to_string()) };
        let Some(keys) = keys else { return };
        self.line.pop();
        if self.line_start > self.line.len() { self.line_start = self.line.len(); }
        self.word = keys;
    }
    /// The words `text` shows: an English word without the punctuation around it, or a Thai run split into dictionary
    /// words. None is anything else (digits, symbols, Thai the dictionary does not cover); it ends a chain of words.
    fn screen_words(&self, text: &str) -> Vec<Option<String>> {
        if lang_of(text) == InputLang::Thai {
            let d = &self.det.th_dict;
            return d.segment(text).into_iter().map(|w| (d.has(w) && !w.chars().any(char::is_numeric)).then(|| w.to_string())).collect()
        }
        let core = text.trim_matches(|c: char| !c.is_alphanumeric());
        vec![(!core.is_empty() && core.chars().all(|c| c.is_ascii_alphabetic())).then(|| core.to_ascii_lowercase())]
    }
    /// The last word `line[j - 1]` shows, unless a line break follows it.
    fn word_before(&self, j: usize) -> Option<String> {
        let c = self.line.get(j.checked_sub(1)?)?;
        if c.after().contains('\n') { return None }
        self.screen_words(c.shown()).pop().flatten()
    }
    /// Give the committed words not learned yet to the word memory, as they are on screen now: what a conversion
    /// replaced is learned as its result. Runs when the next word starts (no bubble can change them any more) and
    /// before the line is forgotten.
    fn settle(&mut self) {
        let from = self.line.iter().rposition(|c| c.learned).map_or(0, |i| i + 1);
        for j in from..self.line.len() {
            self.line[j].learned = true;
            if !self.opts.learn_words || self.blocked(&self.line[j].typed) { continue }
            let mut prev = self.word_before(j);
            for w in self.screen_words(self.line[j].shown()) {
                if let Some(w) = &w {
                    Arc::make_mut(&mut self.words).learn(prev.as_deref().filter(|p| lang_of(p) == lang_of(w)), w);
                    self.events_since_save += 1;
                }
                prev = w;
            }
        }
    }
    /// Backspace: edit the buffer; at a word's end (only its boundary/extra spaces left) step back into that word.
    fn on_backspace(&mut self) -> Outcome {
        let mut a = vec![]; let shown = self.bubble_visible(); let phrase = self.phrase; let up = self.completions_up();
        self.resume_glued();
        if self.word.pop().is_some() {
            // word fully deleted: one fewer word left in the phrase
            let phrase = if self.word.is_empty() { phrase.saturating_sub(1) } else { phrase };
            self.reoffer(shown, phrase, &up, &mut a);
            return Outcome { swallow: false, actions: a }
        }
        match self.line.last_mut() {
            Some(c) if !c.extra.is_empty() => {
                // an extra space / newline after the word: still right after it, bubble unchanged
                c.extra.pop();
                if c.extra.is_empty() && c.boundary != Some(Boundary::Enter) { /* only the word's own boundary left */ }
            }
            Some(c) if c.converted.is_none() && c.rev == self.thai_active => {
                // the boundary itself is deleted: the caret is back inside that word -> it becomes the buffer again
                let c = self.line.pop().unwrap();
                if c.boundary == Some(Boundary::Enter) { self.line_start = 0; }
                if self.line_start > self.line.len() { self.line_start = self.line.len(); }
                self.word = c.typed;
                self.reoffer(shown, phrase, &up, &mut a);
            }
            _ => { a = self.reset(); self.phrase = 0; }   // text we can't track (converted / other layout / unknown)
        }
        Outcome { swallow: false, actions: a }
    }
    /// Option/Ctrl+Backspace: the previous word (and the spaces after it) is deleted.
    fn on_word_delete(&mut self) -> Outcome {
        let mut a = vec![]; let shown = self.bubble_visible(); let phrase = self.phrase;
        if !self.word.is_empty() { self.word.clear(); }
        else if let Some(c) = self.line.last() {
            if c.after().contains('\n') { a = self.reset(); return Outcome { swallow: false, actions: a } } // crosses a line: untracked
            self.line.pop();
        } else { a = self.reset(); return Outcome { swallow: false, actions: a } }
        self.live_streak = 0;
        let left = phrase.saturating_sub(1);
        if shown && left > 0 { self.offer_committed(left, &mut a) } else { self.show(vec![], &mut a) }
        Outcome { swallow: false, actions: a }
    }
    fn on_boundary(&mut self, b: Boundary) -> Outcome {
        self.tab_pending = false;
        self.live_streak = 0; self.comp_muted = false; self.last_key = None;
        let typed = std::mem::take(&mut self.word);
        let mut acts = vec![]; let mut swallow = false;
        if typed.is_empty() {
            // extra spaces / Enters after a word: pass through; the bubble for the last word stays applicable
            if let Some(c) = self.line.last_mut() { c.extra.push_str(b.text()); }
            if b == Boundary::Enter { self.new_line(); }
            return Outcome::pass()
        }
        if self.thai_active { return self.on_boundary_rev(typed, b) }
        let prev = self.word_before(self.line.len());
        let (p, ds) = self.prob(&typed);
        let d = ds.last().unwrap();
        let thai = d.candidate.clone();
        let mut converted = None;
        if self.blocked(&typed) {
            self.show(vec![], &mut acts);
            self.line.push(Committed { orig: typed.clone(), typed, converted: None, boundary: Some(b), rev: false, with_prev: 0, extra: String::new(), learned: false });
            if b == Boundary::Enter { self.new_line(); }
            return Outcome { swallow: false, actions: acts };
        }
        if self.opts.auto_convert && p > self.personal.base.threshold && d.valid {
            // legacy (--auto-convert): replace at the boundary
            swallow = true; converted = Some(thai.clone());
            acts.push(self.replace(&typed, &thai, Some(b)));
            let mut items = vec![BubbleItem { label: typed.clone(), kind: BubbleKind::UndoConversion }];
            items.extend(self.typo_items(&thai, prev.as_deref()));
            self.show(items, &mut acts);
        } else if d.valid && p > self.band(&thai, self.opts.suggest_band) {
            // default: the boundary passes through untouched; the bubble offers the conversion (Tab / hotkey applies)
            let mut items = vec![BubbleItem { label: thai.clone(), kind: BubbleKind::Convert { typed: typed.clone(), thai: thai.clone() } }];
            // the best English spelling stays in reach right under the Thai reading
            let en = self.en_items(&typed, prev.as_deref());
            items.extend(en.first().cloned());
            for t in self.typo_items(&thai, prev.as_deref()) { if t.label != thai { items.push(t) } }
            items.extend(en.into_iter().skip(1));
            items.truncate(3);
            self.show(items, &mut acts); self.phrase = 1;
        } else {
            // left as English: offer spelling corrections
            let en = self.en_items(&typed, prev.as_deref());
            self.show(en, &mut acts); self.phrase = 0;
        }
        self.line.push(Committed { orig: typed.clone(), typed, converted, boundary: Some(b), rev: false, with_prev: 0, extra: String::new(), learned: false });
        if b == Boundary::Enter { self.new_line(); }
        Outcome { swallow, actions: acts }
    }
    /// Reverse direction at a word boundary: flip the Thai reading back to the typed English only at very high confidence.
    fn on_boundary_rev(&mut self, typed: String, b: Boundary) -> Outcome {
        let mut acts = vec![]; let mut swallow = false; let mut converted = None;
        let thai = en_to_th(&typed);
        if self.blocked(&typed) {
            self.show(vec![], &mut acts);
            self.line.push(Committed { typed, orig: thai, converted: None, boundary: Some(b), rev: true, with_prev: 0, extra: String::new(), learned: false });
            if b == Boundary::Enter { self.new_line(); }
            return Outcome { swallow: false, actions: acts };
        }
        let (p, ok) = self.rev_prob(&typed);
        let mut with_prev = 0usize;
        if ok && p > self.opts.rev_auto {
            // right context: up to 3 preceding words left as Thai (e.g. "i" = ร) are re-scored now that
            // this word follows them, and offered together with it if they clear `rev_prev`
            let k = self.rev_prev_count(&typed);
            if self.opts.auto_convert {
                // legacy (--auto-convert): flip at the boundary
                swallow = true; converted = Some(typed.clone()); with_prev = k;
                let start = self.line.len() - k;
                let (mut ins, mut was) = (String::new(), String::new());
                for c in &self.line[start..] { ins += &c.typed; ins += &c.after(); was += &c.orig; was += &c.after(); }
                for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
                acts.push(self.replace(&format!("{was}{thai}"), &format!("{ins}{typed}"), Some(b)));
                self.show(vec![BubbleItem { label: format!("{was}{thai}"), kind: BubbleKind::UndoConversion }], &mut acts);
            } else {
                let start = self.line.len() - k;
                let mut label = String::new(); for c in &self.line[start..] { label += &c.typed; label += &c.after(); }
                label += &typed;
                self.show(vec![BubbleItem { label, kind: BubbleKind::ConvertPrev { k } }], &mut acts); self.phrase = k + 1;
            }
        } else { self.show(vec![], &mut acts); }
        self.line.push(Committed { typed, orig: thai, converted, boundary: Some(b), rev: true, with_prev, extra: String::new(), learned: false });
        if b == Boundary::Enter { self.new_line(); }
        Outcome { swallow, actions: acts }
    }
    /// Enter: a new line starts. Only the last word is kept (so the bubble / Tab can still apply to it); contexts and
    /// the reverse lookback start after it.
    fn new_line(&mut self) {
        // keep the finished line's last few words (a pending ConvertPrev may span up to 4); contexts start after them
        let n = self.line.len().saturating_sub(4); self.line.drain(..n); self.line_start = self.line.len();
    }
    /// Reverse: flip the last committed word and the `k` before it (as they are on screen) to the typed English.
    fn flip_prev(&mut self, k: usize, acts: &mut Vec<Action>) {
        if !self.word.is_empty() || self.line.len() < k + 1 { return }
        let start = self.line.len() - 1 - k;
        let (mut old, mut ins) = (String::new(), String::new());
        for c in &self.line[start..] { if c.converted.is_some() || !c.rev { return } old += &c.orig; old += &c.after(); ins += &c.typed; ins += &c.after(); }
        acts.push(self.replace(&old, &ins, None));
        for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
        if let Some(c) = self.line.last_mut() { c.with_prev = k; }
    }
    /// The last committed word as it is on screen, plus its boundary.
    fn last_on_screen(&self) -> Option<(String, &Committed)> {
        let c = self.line.last()?; if !self.word.is_empty() { return None }
        Some((format!("{}{}", c.shown(), c.after()), c))
    }
    /// The buffer `typed` was just replaced mid-word by `converted`: record it with no boundary, so the next Tab /
    /// Undo / Guess measure from it instead of from the word before it.
    fn commit_glued(&mut self, typed: String, converted: String, with_prev: usize) {
        let orig = self.screen_of(&typed);
        self.line.push(Committed { orig, typed, converted: Some(converted), boundary: None, rev: self.thai_active, with_prev, extra: String::new(), learned: false });
        self.word.clear(); self.live_streak = 0;
    }
    /// What Guess turns `typed` into: the English keys (reverse), else the Thai reading or its top typo correction.
    fn guess_target(&self, typed: &str, rev: bool) -> String {
        if rev { return typed.to_string() }
        let base = en_to_th(typed);
        match self.corrector.as_ref().map(|c| c.suggest(&base, 3, &self.det.thai)) {
            Some(cor) if !cor.in_dict && !cor.suggestions.is_empty() => cor.suggestions[0].word.clone(),
            _ => base,
        }
    }
    /// Forward words before the anchor join a Guess only when their Thai reading is a dictionary word.
    fn guess_joins(&self, c: &Committed, rev: bool) -> bool {
        c.converted.is_none() && c.rev == rev && !self.blocked(&c.typed)
            && (rev || self.corrector.as_ref().is_some_and(|cc| cc.has(&en_to_th(&c.typed))))
    }
    /// The anchor (the buffer, else the last committed word) must look convertible; plain English proposes nothing.
    fn guess_anchor_ok(&self, typed: &str, rev: bool) -> bool {
        if self.blocked(typed) { return false }
        if rev { return true }
        let (p, ds) = self.prob(typed);
        ds.last().is_some_and(|d| d.valid) && (p > self.opts.suggest_band || self.corrector.as_ref().is_some_and(|c| c.has(&en_to_th(typed))))
    }
    /// Propose converting the buffer (or the last word) plus the consecutive same-layout words before it on the line.
    fn guess(&self) -> Option<BubbleItem> {
        let rev = self.thai_active;
        let typed = (!self.word.is_empty()).then(|| self.word.clone());
        let anchor_ok = match &typed {
            Some(t) => self.guess_anchor_ok(t, rev),
            None => self.line.last().is_some_and(|c| c.converted.is_none() && c.rev == rev && self.guess_anchor_ok(&c.typed, rev)),
        };
        if !anchor_ok { return None }
        // committed words included; without a buffer the last committed word is the anchor
        let mut k = usize::from(typed.is_none());
        while k < 8 && k < self.line.len() {
            let c = &self.line[self.line.len() - 1 - k];
            // the phrase stays on one line
            if c.after().contains('\n') || !self.guess_joins(c, rev) { break }
            k += 1;
        }
        let start = self.line.len() - k;
        let mut targets: Vec<String> = self.line[start..].iter().map(|c| self.guess_target(&c.typed, rev)).collect();
        let mut label = String::new();
        for (c, t) in self.line[start..].iter().zip(&targets) { label += t; label += &c.after(); }
        if let Some(t) = &typed { let g = self.guess_target(t, rev); label += &g; targets.push(g); }
        let label = label.trim_end().to_string();
        (!label.is_empty()).then(|| BubbleItem { label, kind: BubbleKind::Guess { k, typed, targets } })
    }
    /// Apply a Guess proposal, if the text it was made for is still what is on screen. The words are committed as
    /// converted to what was inserted, so Undo restores the whole phrase.
    fn apply_guess(&mut self, k: usize, typed: Option<String>, targets: Vec<String>, acts: &mut Vec<Action>) {
        if self.word != typed.as_deref().unwrap_or("") || self.line.len() < k { return }
        let start = self.line.len() - k;
        if self.line[start..].iter().any(|c| c.converted.is_some()) { return }
        let rev = self.thai_active;
        if !rev { let a = typed.clone().unwrap_or_else(|| self.line[self.line.len() - 1].typed.clone()); self.feedback(&a, Feedback::Retype); } // forward only
        let (mut old, mut ins) = (String::new(), String::new());
        for (c, t) in self.line[start..].iter().zip(&targets) { old += &c.orig; old += &c.after(); ins += t; ins += &c.after(); }
        if let Some(t) = &typed { old += &self.screen_of(t); ins += &targets[k]; }
        acts.push(self.replace(&old, &ins, None));
        for (c, t) in self.line[start..].iter_mut().zip(&targets) { c.converted = Some(t.clone()); }
        if let Some(t) = typed { self.commit_glued(t, targets[k].clone(), k); }
        else if let Some(c) = self.line.last_mut() { c.with_prev = k - 1; }
    }
    fn on_hotkey(&mut self, h: Hotkey) -> Outcome {
        self.tab_pending = false;
        let mut acts = vec![];
        match h {
            Hotkey::Undo => {
                let Some((shown, c)) = self.last_on_screen() else { return Outcome { swallow: true, actions: acts } };
                let c = c.clone(); if c.converted.is_none() { return Outcome { swallow: true, actions: acts } }
                if c.with_prev > 0 && self.line.len() > c.with_prev {
                    let start = self.line.len() - 1 - c.with_prev;
                    let (mut old, mut ins) = (String::new(), String::new());
                    for pc in &self.line[start..self.line.len() - 1] { old += pc.shown(); old += &pc.after(); ins += &pc.orig; ins += &pc.after(); }
                    acts.push(self.replace(&format!("{old}{shown}"), &format!("{ins}{}{}", c.orig, c.after()), None));
                    let end = self.line.len() - 1; for pc in &mut self.line[start..end] { pc.converted = None; }
                } else {
                    acts.push(self.replace(&shown, &format!("{}{}", c.orig, c.after()), None));
                }
                self.line.pop(); if !c.rev { self.feedback(&c.typed, Feedback::Undo); }
                // a word converted mid-word (Guess on the buffer) is still being typed: it is the buffer again
                if c.boundary.is_none() && c.extra.is_empty() { self.word = c.typed; }
                else { self.line.push(Committed { converted: None, with_prev: 0, ..c }); }
                self.show(vec![], &mut acts);
            }
            Hotkey::ForceConvert => {
                if !self.word.is_empty() {
                    let typed = std::mem::take(&mut self.word);
                    let (shown, target) = if self.thai_active { (en_to_th(&typed), typed.clone()) } else { (typed.clone(), en_to_th(&typed)) };
                    acts.push(self.replace(&shown, &target, None));
                    if !self.thai_active { self.feedback(&typed, Feedback::Retype); } // personal model: forward only
                    self.commit_glued(typed, target, 0);
                } else if let Some((shown, c)) = self.last_on_screen() {
                    let c = c.clone(); if c.converted.is_some() { return Outcome { swallow: true, actions: acts } }
                    let target = if c.rev { c.typed.clone() } else { en_to_th(&c.typed) };
                    acts.push(self.replace(&shown, &format!("{target}{}", c.after()), None));
                    self.line.pop(); if !c.rev { self.feedback(&c.typed, Feedback::Retype); }
                    self.line.push(Committed { converted: Some(target), ..c });
                }
                self.show(vec![], &mut acts);
            }
            Hotkey::Guess => {
                let item = self.guess();
                self.show(item.into_iter().collect(), &mut acts);
            }
            Hotkey::Accept(i) => {
                let Some(item) = self.bubble.get(i).cloned() else { return Outcome::pass() };
                match item.kind {
                    BubbleKind::Live { typed, thai, prev } => {
                        if !self.live_current(&typed, prev) { return Outcome::pass() }
                        // replace the typed prefix (and, for a reverse phrase, the words before it); the rest of the
                        // word arrives in the new layout (input source switched)
                        let start = self.line.len() - prev;
                        let (mut old, mut ins) = (String::new(), String::new());
                        for c in &self.line[start..] { old += c.shown(); old += &c.after(); ins += &c.typed; ins += &c.after(); }
                        for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
                        acts.push(self.replace(&format!("{old}{}", self.screen_of(&typed)), &format!("{ins}{thai}"), None));
                        self.commit_glued(typed, thai, prev);
                    }
                    BubbleKind::UndoConversion => return self.on_hotkey(Hotkey::Undo),
                    BubbleKind::Convert { .. } => return self.on_hotkey(Hotkey::ForceConvert),
                    BubbleKind::ConvertPrev { k } => self.flip_prev(k, &mut acts),
                    BubbleKind::Guess { k, typed, targets } => self.apply_guess(k, typed, targets, &mut acts),
                    BubbleKind::Rewrite(row) => return self.accept_rewrite(row),
                    BubbleKind::Complete { typed, insert } => {
                        if self.word != typed { return Outcome::pass() }
                        acts.push(self.replace(&self.screen_of(&typed), &insert, None));
                        self.commit_glued(typed, insert, 0);
                    }
                    BubbleKind::Typo { word } => if !self.word.is_empty() {
                        let typed = self.word.clone();
                        acts.push(self.replace(&self.screen_of(&typed), &word, None));
                        self.commit_glued(typed, word, 0);
                    } else if let Some((shown, c)) = self.last_on_screen() {
                        let c = c.clone();
                        acts.push(self.replace(&shown, &format!("{word}{}", c.after()), None));
                        self.line.pop(); self.line.push(Committed { converted: Some(word), ..c });
                    },
                }
                self.show(vec![], &mut acts);
            }
            Hotkey::Dismiss => { self.dismiss(&mut acts); return Outcome { swallow: false, actions: acts } }
            Hotkey::Rewrite => return self.start_rewrite(),
            Hotkey::TogglePause => unreachable!(),
        }
        Outcome { swallow: true, actions: acts }
    }
    /// Some row on the bubble applies a fix (a notice only closes it).
    fn offers_fix(&self) -> bool { self.bubble.iter().any(|b| !matches!(b.kind, BubbleKind::Rewrite(RewriteRow::Notice))) }
    fn rewriting(&self) -> bool { self.capturing.is_some() || matches!(self.bubble.first(), Some(BubbleItem { kind: BubbleKind::Rewrite(_), .. })) }
    /// Keys a rewrite session survives. Anything else ends it first; unlike suggestions, Space/Enter/Backspace too,
    /// because the selection the rows refer to has just been edited.
    fn rewrite_keeps(&self, k: &Key) -> bool {
        match k {
            // holding the hotkey autorepeats: while capturing that is a no-op, not a restart
            Key::Hotkey(Hotkey::Rewrite) => self.capturing.is_some(),
            Key::SelectPrev | Key::SelectNext | Key::Esc | Key::Hotkey(Hotkey::Accept(_) | Hotkey::Dismiss) => self.bubble_visible(),
            Key::Boundary(Boundary::Enter) => self.fixing && self.bubble_visible(),
            _ => false,
        }
    }
    fn mint(&mut self) -> ReqId { self.last_req = self.last_req.next(); self.last_req }
    /// Read the selection first: what is offered depends on it. The line is kept, as the word before the caret is what
    /// gets offered when nothing is selected; any key while capturing still resets it.
    fn start_rewrite(&mut self) -> Outcome {
        if self.capturing.is_some() { return Outcome { swallow: true, actions: vec![] } }
        let mut a = vec![]; self.show(vec![], &mut a); self.tab = TabChord::Up;
        let id = self.mint(); self.capturing = Some(id);
        a.push(Action::Rewrite(Job::Capture(id)));
        Outcome { swallow: true, actions: a }
    }
    fn accept_rewrite(&mut self, r: RewriteRow) -> Outcome {
        let mut a = vec![];
        match r {
            RewriteRow::Preset(preset, text) => {
                // Enter while it runs is the user confirming, not a newline
                let id = self.mint(); self.show(vec![row(rewrite::REWRITING, RewriteRow::Busy(id))], &mut a); self.fixing = true;
                a.push(Action::Rewrite(Job::Run(Run { req: id, preset, text, cfg: self.rewrite.clone() })));
            }
            RewriteRow::Busy(_) => {}
            RewriteRow::Result(text) | RewriteRow::Fixed(_, text) => { self.show(vec![], &mut a); a.push(Action::Rewrite(Job::Paste(text))) }
            RewriteRow::Notice => self.show(vec![], &mut a),
        }
        Outcome { swallow: true, actions: a }
    }
    /// ⌃⌥R with nothing selected: the rows for the word before the caret, the buffer or else the last word on the
    /// line. None when there is no such word.
    fn word_options(&self) -> Option<Vec<BubbleItem>> {
        if !self.word.is_empty() { return Some(self.word_rows(&self.word, self.thai_active, self.word_before(self.line.len()))) }
        let c = self.line.last().filter(|_| self.line.len() > self.line_start)?;
        // a converted word's other layout is its undo
        if c.converted.is_some() { return Some(vec![BubbleItem { label: c.orig.clone(), kind: BubbleKind::UndoConversion }]) }
        Some(self.word_rows(&c.typed, c.rev, self.word_before(self.line.len() - 1)))
    }
    /// The conversion of `typed` (keys typed in the `rev` direction) when it looks meant, and the spelling fixes of the
    /// word as it is and as converted, in the order the boundary bubble uses. A reverse conversion clears a high bar,
    /// so the Thai on screen is not offered fixed next to it; a forward one keeps the English spelling in reach.
    fn word_rows(&self, typed: &str, rev: bool, prev: Option<String>) -> Vec<BubbleItem> {
        let (screen, target) = if rev { (en_to_th(typed), typed.to_string()) } else { (typed.to_string(), en_to_th(typed)) };
        let fixes = |text: &str, thai: bool| if thai { self.typo_items(text, prev.as_deref()) } else { self.en_items(text, prev.as_deref()) };
        let converts = if rev { !self.blocked(typed) && { let (p, ok) = self.rev_prob(typed); ok && p > self.opts.rev_prev } } else { self.guess_anchor_ok(typed, false) };
        let here = if rev && converts { vec![] } else { fixes(&screen, rev) };
        if !converts { return here.into_iter().take(3).collect() }
        let mut rows = vec![BubbleItem { label: target.clone(), kind: BubbleKind::Convert { typed: typed.to_string(), thai: target.clone() } }];
        rows.extend(here.first().cloned());
        rows.extend(fixes(&target, !rev).into_iter().filter(|t| t.label != target));
        rows.extend(here.into_iter().skip(1));
        rows.truncate(3);
        rows
    }
    /// Rows for selected text: the local fixes, then the AI presets when they can run. A notice when there is nothing.
    fn selection_offer(&self, text: &str, has_key: bool) -> Vec<BubbleItem> {
        let mut rows = self.selection_rows(text);
        if self.rewrite.enabled && has_key { rows.extend(Preset::ALL.map(|p| row(p.label(), RewriteRow::Preset(p, text.to_string())))) }
        if rows.is_empty() { rows.push(row(if self.rewrite.enabled { rewrite::NO_KEY } else { rewrite::NO_SUGGESTIONS }, RewriteRow::Notice)) }
        rows
    }
    /// The selection in the other layout, then with its spelling fixed, each when it changes something. Each row
    /// pastes its whole text over the selection.
    fn selection_rows(&self, text: &str) -> Vec<BubbleItem> {
        if text.chars().count() > SELECTION_LOCAL_MAX { return vec![] }
        let spans = word_spans(text);
        let flips = self.selection_flips(text, &spans);
        let converted = splice(text, &spans, flips.iter().map(|f| f.as_ref().map(|(w, _)| w.clone())).collect());
        // the language the flipped words become: Thai unless every flip is the reverse one, back to English keys
        let lang = if flips.iter().flatten().all(|(_, rev)| *rev) { InputLang::Latin } else { InputLang::Thai };
        let mut out: Vec<(Reason, String)> = vec![];
        for (why, t) in [(Reason::Layout(lang), converted), (Reason::Typo, self.fix_selection(text, &spans, &flips))] {
            if let Some(t) = t.filter(|t| !out.iter().any(|(_, o)| o == t)) { out.push((why, t)) }
        }
        out.into_iter().map(|(why, t)| row(&rewrite::preview(&t, rewrite::PREVIEW_CHARS), RewriteRow::Fixed(why, t))).collect()
    }
    /// Each word typed on the wrong layout, flipped, and whether that is the reverse direction: English keys that read
    /// as Thai, and Thai that reads as English keys. Each direction scores its words as one line.
    fn selection_flips(&self, text: &str, spans: &[(usize, usize)]) -> Vec<Option<(String, bool)>> {
        let words: Vec<&str> = spans.iter().map(|&(a, b)| &text[a..b]).collect();
        let mut to: Vec<Option<(String, bool)>> = vec![None; words.len()];
        let fwd: Vec<usize> = (0..words.len()).filter(|&i| words[i].is_ascii() && words[i].chars().any(|c| c.is_ascii_alphabetic())).collect();
        if !fwd.is_empty() {
            let ds = self.det.decide_line(&fwd.iter().map(|&i| words[i]).collect::<Vec<_>>());
            for ((&i, d), p) in fwd.iter().zip(&ds).zip(self.personal.line_probs(&ds)) {
                if d.valid && !self.blocked(words[i]) && p > self.band(&d.candidate, self.opts.suggest_band) { to[i] = Some((d.candidate.clone(), false)) }
            }
        }
        let rev: Vec<(usize, String)> = (0..words.len()).filter(|&i| lang_of(words[i]) == InputLang::Thai).filter_map(|i| Some((i, th_text_to_keys(words[i])?))).collect();
        if let (Some(r), false) = (&self.rev, rev.is_empty()) {
            let ds = r.decide_line(&self.det, &rev.iter().map(|(_, k)| k.as_str()).collect::<Vec<_>>());
            for (((i, keys), d), p) in rev.iter().zip(&ds).zip(r.line_probs(&ds)) {
                if d.convertible && !self.blocked(keys) && p > self.opts.rev_prev { to[*i] = Some((keys.clone(), true)) }
            }
        }
        to
    }
    /// Spelling fixes, word by word: the Thai corrector for Thai runs, the spell checker for English words until
    /// `SPELL_BUDGET` is spent. Words that flip in reverse are English, so their Thai reading is left alone (see
    /// `word_rows`).
    fn fix_selection(&self, text: &str, spans: &[(usize, usize)], flips: &[Option<(String, bool)>]) -> Option<String> {
        if !self.opts.typo_suggestions { return None }
        let deadline = Instant::now() + SPELL_BUDGET;
        let to = spans.iter().zip(flips).map(|(&(a, b), f)| { let w = &text[a..b];
            if matches!(f, Some((_, true))) { None }
            else if lang_of(w) == InputLang::Thai { self.fix_thai(w) }
            else if Instant::now() < deadline { self.en_items(w, None).into_iter().next().map(|i| i.label) }
            else { None } }).collect();
        splice(text, spans, to)
    }
    /// A Thai run with the typos the corrector is sure of fixed. It judges about a word at a time, so when the whole run
    /// is not fixed, windows of up to three pieces that hold a piece the dictionary lacks are, widest first; windows of
    /// known words only are left alone, as the corrector "fixes" correct text there.
    fn fix_thai(&self, run: &str) -> Option<String> {
        let c = self.corrector.as_ref()?;
        let fix = |w: &str| { let r = c.suggest(w, 1, &self.det.thai); (!r.in_dict && r.keep_confidence < TYPO_KEEP).then(|| r.suggestions.into_iter().next().map(|s| s.word)).flatten() };
        if let Some(w) = fix(run) { return Some(w) }
        let segs = self.det.th_dict.segment(run);
        let (mut out, mut i, mut changed) = (String::new(), 0, false);
        while i < segs.len() {
            let unknown = |n: usize| segs[i..i + n].iter().any(|w| !self.det.th_dict.has(w));
            match (1..=3).rev().filter(|&n| i + n <= segs.len() && n < segs.len() && unknown(n)).find_map(|n| Some((n, fix(&segs[i..i + n].concat())?))) {
                Some((n, w)) => { out += &w; i += n; changed = true }
                None => { out += segs[i]; i += 1 }
            }
        }
        changed.then_some(out)
    }
    /// A rewrite worker answer. Stale answers (cancelled or superseded) change nothing.
    pub fn on_reply(&mut self, r: Reply) -> Vec<Action> {
        let mut a = vec![];
        match r {
            Reply::Captured { id, sel, has_key } if self.capturing == Some(id) => {
                self.capturing = None;
                let rows = match sel {
                    // pasting over the selection edits text the line does not track
                    Captured::Text(t) if !t.trim().is_empty() => { self.reset(); self.selection_offer(&t, has_key) }
                    Captured::Text(_) | Captured::NoSelection => match self.word_options() {
                        Some(rows) if !rows.is_empty() => rows,
                        Some(_) => vec![row(rewrite::NO_SUGGESTIONS, RewriteRow::Notice)],
                        None => vec![row(rewrite::NO_SELECTION, RewriteRow::Notice)],
                    },
                    Captured::Secure => vec![],
                };
                self.show(rows, &mut a);
                self.fixing = self.offers_fix();
            }
            Reply::Rewritten(id, res) if matches!(self.bubble.first(), Some(BubbleItem { kind: BubbleKind::Rewrite(RewriteRow::Busy(b)), .. }) if *b == id) => {
                let r = match res { Ok(t) => row(&rewrite::preview(&t, rewrite::PREVIEW_CHARS), RewriteRow::Result(t)), Err(f) => row(&f.message(), RewriteRow::Notice) };
                self.show(vec![r], &mut a);
                self.fixing = self.offers_fix();
            }
            _ => {}
        }
        a
    }
}
fn row(label: &str, r: RewriteRow) -> BubbleItem { BubbleItem { label: label.to_string(), kind: BubbleKind::Rewrite(r) } }
/// Longest selection the local rows look at, on the thread that also takes the keys. The Thai corrector needs about
/// 2 ms for this much text.
const SELECTION_LOCAL_MAX: usize = 300;
/// Time the spell checker gets for a selection: about 0.15 ms per right word, 2 to 8 ms per misspelled one (macOS,
/// measured in `completion_cost`), so a typo-heavy selection is fixed only up to here.
const SPELL_BUDGET: Duration = Duration::from_millis(25);
/// Corrector suggestions the Thai typo rows are ranked from.
const TYPO_GUESSES: usize = 10;
/// The corrector replaces a Thai word when it would keep it with less confidence than this. In samples, correct runs
/// score 0.99 and up and runs with a typo 0.35 to 0.71.
const TYPO_KEEP: f64 = 0.9;
/// Byte spans of the non-whitespace runs in `text`.
fn word_spans(text: &str) -> Vec<(usize, usize)> {
    let (mut v, mut start) = (vec![], None);
    for (i, c) in text.char_indices() { match (c.is_whitespace(), start) { (false, None) => start = Some(i), (true, Some(s)) => { v.push((s, i)); start = None } _ => {} } }
    if let Some(s) = start { v.push((s, text.len())) }
    v
}
/// `text` with the words at `spans` replaced where `to` has a replacement; None when that changes nothing.
fn splice(text: &str, spans: &[(usize, usize)], to: Vec<Option<String>>) -> Option<String> {
    let (mut out, mut at) = (String::new(), 0);
    for (&(a, b), t) in spans.iter().zip(to) { out += &text[at..a]; out += t.as_deref().unwrap_or(&text[a..b]); at = b; }
    out += &text[at..];
    (out != text).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use typethai_core::{personal::DEFAULT_PERSONAL, ModelFiles};
    /// The unpacked model dir in the tree, else the `.ttm` in `TYPETHAI_MODEL`.
    fn model() -> Option<ModelFiles> {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/model");
        if p.join("model.json").exists() { return Some(ModelFiles::load_json_dir(&p).unwrap()) }
        if let Some(t) = std::env::var_os("TYPETHAI_MODEL") { return Some(ModelFiles::load(std::path::Path::new(&t)).unwrap()) }
        eprintln!("SKIP: no model files"); None
    }
    fn engine() -> Option<Engine> {
        let m = model()?;
        let mut e = Engine::new(m.learn_detector(), PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL), None, Options::default());
        e.rev = Some(Reverse::embedded()); Some(e)
    }
    /// `engine()` with the typo corrector (Guess needs its dictionary).
    fn engine_typo() -> Option<Engine> {
        let m = model()?;
        let mut e = Engine::new(m.learn_detector(), PersonalModel::new(m.classifier.clone(), None, DEFAULT_PERSONAL), Some(m.corrector()), Options::default());
        e.rev = Some(Reverse::embedded()); e.opts.live = false; Some(e)
    }
    /// Tab pressed and released, as one outcome (the press decides whether a tab is typed).
    fn tab(e: &mut Engine) -> Outcome { let mut o = e.on_key(Key::Tab); o.actions.extend(e.on_key(Key::TabUp).actions); o }
    fn typ(e: &mut Engine, s: &str) -> Vec<Outcome> {
        s.chars().map(|c| e.on_key(match c { ' ' => Key::Boundary(Boundary::Space), '\n' => Key::Boundary(Boundary::Enter), c => Key::Char(c) })).collect()
    }
    #[test] fn legacy_converts_thai_and_keeps_english() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        let o = typ(&mut e, "l;ylfu ");
        let last = o.last().unwrap(); assert!(last.swallow);
        assert_eq!(last.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: Some(Boundary::Space) });
        e.reset();
        let o = typ(&mut e, "hello world ");
        assert!(o.iter().all(|x| !x.swallow && !x.actions.iter().any(|a| matches!(a, Action::Replace { .. }))));
    }
    #[test] fn legacy_undo_restores_and_learns() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        typ(&mut e, "l;ylfu ");
        let o = e.on_key(Key::Hotkey(Hotkey::Undo));
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "l;ylfu ".into(), then: None });
        assert_eq!(e.personal.state.events, 1.0);
        // undo with nothing converted is a no-op
        let o = e.on_key(Key::Hotkey(Hotkey::Undo)); assert!(o.actions.is_empty());
    }
    #[test] fn reset_and_pause() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "l;yl"); e.on_key(Key::Nav); assert_eq!(e.buffer(), "");
        e.on_key(Key::Hotkey(Hotkey::TogglePause)); let o = typ(&mut e, "l;ylfu "); assert!(o.iter().all(|x| !x.swallow));
    }
    #[test] fn selection_wraps() {
        let mut s = Selection::new(3); assert_eq!(s.idx, 0);
        assert_eq!(s.next(), 1); assert_eq!(s.next(), 2); assert_eq!(s.next(), 0);
        assert_eq!(s.prev(), 2); assert_eq!(s.prev(), 1);
        let mut one = Selection::new(1); assert_eq!(one.next(), 0); assert_eq!(one.prev(), 0);
        let mut none = Selection::new(0); assert_eq!(none.next(), 0);
    }
    #[test] fn lang_detection() { assert_eq!(lang_of("สวัสดี "), InputLang::Thai); assert_eq!(lang_of("l;ylfu "), InputLang::Latin); }
    fn bubble_items(o: &Outcome) -> Option<&Vec<BubbleItem>> { o.actions.iter().find_map(|a| if let Action::ShowBubble(v) = a { Some(v) } else { None }) }
    #[test] fn bubble_keys_pass_through_without_bubble() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "hello");
        for k in [Key::SelectPrev, Key::SelectNext, Key::Esc] { let o = e.on_key(k); assert!(!o.swallow); }
        assert_eq!(e.buffer(), ""); // shift+arrows / esc reset the word like other navigation
        // Tab without a bubble is a normal (passed-through) boundary
        typ(&mut e, "hello"); let o = tab(&mut e); assert!(!o.swallow); assert!(!e.bubble_visible());
    }
    #[test] fn legacy_tab_converts_as_boundary_when_no_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        typ(&mut e, "l;ylfu"); let o = tab(&mut e);
        assert!(o.swallow); assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: Some(Boundary::Tab) });
        assert!(o.actions.contains(&Action::SwitchInput(InputLang::Thai)));
    }
    #[test] fn legacy_bubble_navigation_and_tab_apply() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        let o = typ(&mut e, "l;ylfu "); let last = o.last().unwrap();
        assert!(last.actions.contains(&Action::SwitchInput(InputLang::Thai)));
        let n = bubble_items(last).expect("bubble").len(); assert!(e.bubble_visible()); assert_eq!(e.selected(), 0);
        // shift+down moves (swallowed), wraps; shift+up back
        let o = e.on_key(Key::SelectNext); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Select(1 % n)]);
        let o = e.on_key(Key::SelectPrev); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Select(0)]);
        let o = e.on_key(Key::SelectPrev); assert_eq!(o.actions, vec![Action::Select(n - 1)]);
        for _ in 0..(n - 1) { e.on_key(Key::SelectPrev); } assert_eq!(e.selected(), 0);
        // Tab applies the selected (row 0 = undo): swallowed, restores the Latin text, switches to Latin, hides bubble
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "l;ylfu ".into(), then: None });
        assert!(o.actions.contains(&Action::HideBubble)); assert!(o.actions.contains(&Action::SwitchInput(InputLang::Latin)));
        assert!(!e.bubble_visible());
        // with the bubble gone, Tab is an ordinary key again
        let o = tab(&mut e); assert!(!o.swallow);
    }
    #[test] fn legacy_esc_dismisses_and_typing_hides() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        assert!(e.is_bubble_key(&Key::Tab)); assert!(!e.is_bubble_key(&Key::Char('a')));
        let o = e.on_key(Key::Esc); assert!(o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
        assert!(!e.on_key(Key::Esc).swallow); assert!(!e.is_bubble_key(&Key::Tab));
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        let o = e.on_key(Key::Char('h')); assert!(!o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
    }
    #[test] fn live_bubble_mid_word_and_tab_apply() {
        let Some(mut e) = engine() else { return };
        e.opts.live_show = 0.5; e.opts.live_stable = 2; e.opts.live_min_len = 3;
        // "l;ylfu" = สวัสดี: no bubble before 3 keys, then it needs 2 stable keystrokes
        let o = typ(&mut e, "l;"); assert!(o.iter().all(|x| x.actions.is_empty() && !x.swallow));
        let mut first = None;
        for (i, c) in "ylfu".chars().enumerate() { let o = e.on_key(Key::Char(c)); assert!(!o.swallow);
            if first.is_none() && bubble_items(&o).is_some() { first = Some(i + 3) } }
        let k = first.expect("live bubble shows mid-word"); assert!(k >= 4, "stability: not before keystroke 4, got {k}");
        assert!(e.live_visible());
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: None });
        assert!(o.actions.contains(&Action::SwitchInput(InputLang::Thai))); assert!(!e.bubble_visible()); assert_eq!(e.buffer(), "");
    }
    #[test] fn suggestion_shows_before_space() {
        let Some(mut e) = engine() else { return };
        let mut first = None;
        for (i, c) in "l;ylfu".chars().enumerate() {
            let o = e.on_key(Key::Char(c));
            if first.is_none() && bubble_items(&o).is_some() { first = Some(i + 1); }
        }
        let k = first.expect("bubble before space");
        assert!(k < 6, "showed on key {k}; space would be the next key");
        assert!(e.live_visible());
        e.reset();
        let o = typ(&mut e, ";b");
        assert!(o.iter().all(|x| bubble_items(x).is_none()), "2 keys are too few for the live bubble");
        let o = e.on_key(Key::Boundary(Boundary::Space));
        assert!(bubble_items(&o).is_some(), "a 2-character word is offered after Space");
    }
    #[test] fn live_bubble_stays_off_for_english_and_hides_when_confidence_drops() {
        let Some(mut e) = engine() else { return };
        e.opts.live_show = 0.5;
        let o = typ(&mut e, "hello world function return ");
        assert!(o.iter().all(|x| bubble_items(x).map_or(true, |v| !matches!(v[0].kind, BubbleKind::Live { .. }))));
        e.reset(); e.opts.live_stable = 1; e.opts.live_show = 0.0; e.opts.live_min_len = 1;
        typ(&mut e, "l;y"); assert!(e.live_visible());
        e.opts.live_show = 1.1; let o = e.on_key(Key::Char('l')); assert!(o.actions.contains(&Action::HideBubble)); assert!(!e.bubble_visible());
    }
    /// Type a Thai string the way the platform reports it while Kedmanee is active: as the US keys that produced it.
    fn typ_th(e: &mut Engine, th: &str) -> Vec<Outcome> {
        e.set_thai_active(true);
        typ(e, &typethai_core::reverse::th_text_to_keys(th).expect("typeable on Kedmanee"))
    }
    fn replaced(o: &[Outcome]) -> Vec<String> { o.iter().flat_map(|x| x.actions.iter()).filter_map(|a| if let Action::Replace { insert, .. } = a { Some(insert.clone()) } else { None }).collect() }
    #[test] fn legacy_reverse_i_dont_know() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        // exactly what the user typed: 'i dont know' on the Thai layout, then Enter
        let mut o = typ_th(&mut e, "ร กนืะ าืนไ"); o.push(e.on_key(Key::Boundary(Boundary::Enter)));
        let r = replaced(&o);
        assert_eq!(r, vec!["i dont", "know"], "{r:?}"); // "i" (ร) is flipped together with "dont" once it has right context
        // every flip deletes the Thai reading and re-sends the boundary; then switches to ABC
        assert!(o.iter().flat_map(|x| x.actions.iter()).any(|a| *a == Action::SwitchInput(InputLang::Latin)));
        let all: Vec<&Action> = o.iter().flat_map(|x| x.actions.iter()).collect();
        assert!(all.contains(&&Action::Replace { delete: 6, insert: "i dont".into(), then: Some(Boundary::Space) })); // ร + space + กนืะ
    }
    #[test] fn legacy_reverse_variants() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        for (th, en) in [("กนืะ าืนไ", "dont know"), ("ร กนืงะ าืนไ", "i don't know"), ("ร กนืะ", "i dont"), ("ณ กนืงะ าืนไ", "I don't know")] {
            e.reset();
            let mut o = typ_th(&mut e, th); o.push(e.on_key(Key::Boundary(Boundary::Enter)));
            let got = replaced(&o).join(" "); // a replace of "i dont" counts as both words
            eprintln!("{th} -> {got:?}");
            if !en.is_empty() { assert_eq!(got, en, "{th}"); }
        }
    }
    #[test] fn reverse_keeps_real_thai() {
        let Some(mut e) = engine() else { return };
        for th in ["สวัสดีครับ วันนี้อากาศดี", "ไม่รู้ ไม่ชี้", "555 ขำมาก", "กิน ข้าว ยัง", "โอเค ได้ เลย", "ครับ", "คะ", "นะ"] {
            e.reset();
            let Some(keys) = typethai_core::reverse::th_text_to_keys(th) else { continue };
            e.set_thai_active(true);
            let mut o = typ(&mut e, &keys); o.push(e.on_key(Key::Boundary(Boundary::Enter)));
            assert!(replaced(&o).is_empty(), "{th}: {:?}", replaced(&o));
            assert!(o.iter().all(|x| !x.swallow), "{th}");
        }
    }
    #[test] fn legacy_reverse_undo_switches_back_to_thai() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        typ_th(&mut e, "กนืะ าืนไ ");
        // the OS is on ABC now; Tab (bubble key) applies the undo row
        e.set_thai_active(false); assert!(e.bubble_visible());
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 5, insert: "าืนไ ".into(), then: None });
        assert!(o.actions.contains(&Action::SwitchInput(InputLang::Thai)));
    }
    #[test] fn reverse_live_bubble_and_tab() {
        let Some(mut e) = engine() else { return };
        e.opts.rev_live = 0.5;
        let o = typ_th(&mut e, "าืนไ"); // "know"
        assert!(o.iter().any(|x| bubble_items(x).is_some_and(|v| matches!(v[0].kind, BubbleKind::Live { .. }))), "live bubble mid-word");
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 4, insert: "know".into(), then: None });
        assert!(o.actions.contains(&Action::SwitchInput(InputLang::Latin)));
    }
    #[test] fn legacy_switch_input_can_be_disabled() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.switch_input = false;
        let o = typ(&mut e, "l;ylfu "); assert!(!o.last().unwrap().actions.iter().any(|a| matches!(a, Action::SwitchInput(_))));
    }
    #[test] fn force_convert_current_word() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "hello ");
        let o = e.on_key(Key::Hotkey(Hotkey::ForceConvert));
        assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "้ำสสน ".into(), then: None });
        assert_eq!(e.personal.state.events, 1.0);
    }
    // ---------- default mode: nothing converts on a boundary; Tab / hotkeys apply the bubble ----------
    fn screen(o: &[(Option<char>, Outcome)]) -> String { screen_in(DeleteUnit::CodePoint, o) }
    /// The text an app that deletes `unit`s per Backspace ends up with.
    fn screen_in(unit: DeleteUnit, o: &[(Option<char>, Outcome)]) -> String {
        let mut sc = vec![]; for (t, x) in o { crate::platform::sim::apply(&mut sc, &x.actions, x.swallow, *t, unit); } sc.into_iter().collect()
    }
    fn key_of(c: char) -> Key { match c { ' ' => Key::Boundary(Boundary::Space), '\n' => Key::Boundary(Boundary::Enter), c => Key::Char(c) } }
    #[test] fn boundaries_never_convert_by_default() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        let o = typ(&mut e, "l;ylfu ");
        assert!(o.iter().all(|x| !x.swallow && !x.actions.iter().any(|a| matches!(a, Action::Replace { .. }))), "space passes through");
        let items = bubble_items(o.last().unwrap()).expect("bubble offers the conversion");
        assert_eq!(items[0].label, "สวัสดี"); assert!(matches!(items[0].kind, BubbleKind::Convert { .. }));
        // Enter also passes through, and the bubble for the finished word stays
        e.reset(); let o = typ(&mut e, "l;ylfu\n"); assert!(!o.last().unwrap().swallow); assert!(e.bubble_visible());
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "สวัสดี\n".into(), then: None });
        // reverse mode too
        e.reset(); let mut o = typ_th(&mut e, "กนืะ าืนไ"); o.push(e.on_key(Key::Boundary(Boundary::Enter))); assert!(replaced(&o).is_empty()); assert!(e.bubble_visible()); assert!(o.iter().all(|x| !x.swallow));
    }
    #[test] fn tab_after_space_converts_previous_word() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        let mut log: Vec<(Option<char>, Outcome)> = "l;ylfu  ".chars().map(|c| (Some(c), e.on_key(key_of(c)))).collect(); // two spaces
        assert!(e.bubble_visible(), "extra space keeps the bubble");
        log.push((Some('\t'), tab(&mut e)));
        assert!(log.last().unwrap().1.swallow);
        assert_eq!(screen(&log), "สวัสดี  ");
        assert!(log.last().unwrap().1.actions.contains(&Action::SwitchInput(InputLang::Thai)));
        // ^Z undoes it
        let o = e.on_key(Key::Hotkey(Hotkey::Undo)); log.push((None, o)); assert_eq!(screen(&log), "l;ylfu  ");
    }
    #[test] fn next_word_or_caret_move_hides_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        let o = e.on_key(Key::Char('h')); assert_eq!(o.actions, vec![Action::HideBubble]); assert!(!e.bubble_visible());
        typ(&mut e, "ello l;ylfu "); assert!(e.bubble_visible());
        e.on_key(Key::Nav); assert!(!e.bubble_visible());
        // Tab with no bubble is a plain Tab
        typ(&mut e, "hello"); let o = tab(&mut e); assert!(!o.swallow); assert!(o.actions.iter().all(|a| !matches!(a, Action::Replace { .. })));
    }
    #[test] fn hotkey_applies_after_boundary() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ(&mut e, "l;ylfu ");
        let o = e.on_key(Key::Hotkey(Hotkey::Accept(0))); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "สวัสดี ".into(), then: None });
    }
    #[test] fn reverse_i_dont_know_with_tab() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        // 'i dont know' typed on the Thai layout, then Enter (passes through); the bubble offers the whole phrase
        let keys = typethai_core::reverse::th_text_to_keys("ร กนืะ าืนไ").unwrap();
        e.set_thai_active(true);
        let mut log: Vec<(Option<char>, Outcome)> = vec![];
        for c in keys.chars() { let shown = if c == ' ' { ' ' } else { en_to_th(&c.to_string()).chars().next().unwrap() }; log.push((Some(shown), e.on_key(key_of(c)))); }
        log.push((Some('\n'), e.on_key(Key::Boundary(Boundary::Enter))));
        assert_eq!(screen(&log), "ร กนืะ าืนไ\n", "nothing replaced automatically");
        let items = e.bubble.clone(); assert_eq!(items.len(), 1); assert_eq!(items[0].label, "i dont know");
        log.push((Some('\t'), tab(&mut e)));
        assert_eq!(screen(&log), "i dont know\n");
        assert!(log.last().unwrap().1.actions.contains(&Action::SwitchInput(InputLang::Latin)));
        // undo restores all three words
        log.push((None, e.on_key(Key::Hotkey(Hotkey::Undo)))); assert_eq!(screen(&log), "ร กนืะ าืนไ\n");
    }
    #[test] fn reverse_real_thai_gets_no_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        for th in ["สวัสดีครับ วันนี้อากาศดี", "ไม่รู้ ไม่ชี้", "กิน ข้าว ยัง", "ครับ"] {
            e.reset(); let mut o = typ_th(&mut e, th); o.push(e.on_key(Key::Boundary(Boundary::Enter)));
            assert!(o.iter().all(|x| bubble_items(x).is_none()), "{th}");
        }
    }
    // ---------- Backspace / word delete / line delete keep (or close) the bubble ----------
    fn run(e: &mut Engine, log: &mut Vec<(Option<char>, Outcome)>, keys: &[Key]) {
        for k in keys { let t = match k { Key::Char(c) => Some(*c), Key::Boundary(b) => b.text().chars().next(), Key::Backspace => Some('\u{8}'), Key::WordDelete => Some('\u{17}'), Key::LineDelete => Some('\u{15}'), _ => None };
            let o = e.on_key(k.clone()); log.push((t, o)); }
    }
    fn label0(e: &Engine) -> Option<String> { e.bubble.first().map(|b| b.label.clone()) }
    #[test] fn backspace_mid_word_keeps_live_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.live_show = 0.5;
        typ(&mut e, "l;ylfu"); assert!(e.live_visible());
        let o = e.on_key(Key::Backspace);
        assert!(!o.swallow); assert!(!o.actions.contains(&Action::HideBubble), "no close/flicker: {:?}", o.actions);
        assert!(e.live_visible()); assert_eq!(e.buffer(), "l;ylf"); assert_eq!(label0(&e).unwrap(), en_to_th("l;ylf"));
        // below min length / empty: closes
        e.on_key(Key::Backspace); e.on_key(Key::Backspace); let o = e.on_key(Key::Backspace);
        assert_eq!(e.buffer(), "l;"); assert!(o.actions.contains(&Action::HideBubble)); assert!(!e.bubble_visible());
        // confidence drop closes too
        e.reset(); typ(&mut e, "l;ylfu"); e.opts.live_show = 1.1; let o = e.on_key(Key::Backspace); assert!(o.actions.contains(&Action::HideBubble));
    }
    #[test] fn backspace_after_space_reenters_word() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        let mut log = vec![];
        run(&mut e, &mut log, &[Key::Char('l'), Key::Char(';'), Key::Char('y'), Key::Char('l'), Key::Char('f'), Key::Char('u'), Key::Boundary(Boundary::Space), Key::Boundary(Boundary::Space)]);
        assert!(e.bubble_visible());
        run(&mut e, &mut log, &[Key::Backspace]); assert!(e.bubble_visible()); assert_eq!(e.buffer(), ""); // extra space
        run(&mut e, &mut log, &[Key::Backspace]); assert!(e.bubble_visible(), "back inside the word"); assert_eq!(e.buffer(), "l;ylfu");
        assert_eq!(screen(&log), "l;ylfu");
        run(&mut e, &mut log, &[Key::Backspace]); assert_eq!(e.buffer(), "l;ylf"); assert!(e.bubble_visible());
        // Tab converts what's there now
        log.push((Some('\t'), tab(&mut e))); assert_eq!(screen(&log), en_to_th("l;ylf"));
        // delete a whole word with Backspace: closes when empty
        e.reset(); let mut log = vec![]; run(&mut e, &mut log, &"l;ylfu ".chars().map(key_of).collect::<Vec<_>>());
        run(&mut e, &mut log, &[Key::Backspace]); assert!(e.bubble_visible()); assert_eq!(e.buffer(), "l;ylfu");
        // shorter prefixes: shown while confident, closed once below the threshold / min length (and stays closed)
        let mut closed = false;
        for _ in 0..5 { run(&mut e, &mut log, &[Key::Backspace]); if !e.bubble_visible() { closed = true } else { assert!(!closed, "no re-open flicker") } }
        assert!(closed); assert_eq!(screen(&log), "l");
        let o = e.on_key(Key::Backspace); assert!(!e.bubble_visible(), "{:?}", o.actions); assert_eq!(e.buffer(), "");
    }
    #[test] fn reverse_live_offers_the_phrase() {
        let Some(mut e) = engine() else { return };
        e.set_thai_active(true);
        let mut label = None;
        for c in "i need".chars() {
            let o = e.on_key(if c == ' ' { Key::Boundary(Boundary::Space) } else { Key::Char(c) });
            if let Some(items) = bubble_items(&o) { label = Some(items[0].label.clone()); }
        }
        assert_eq!(label.as_deref(), Some("i need"), "the word before a confident word stays in the live bubble");
        let o = tab(&mut e);
        assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "i need".into(), then: None });
        e.reset();
        for c in "i need".chars() { e.on_key(if c == ' ' { Key::Boundary(Boundary::Space) } else { Key::Char(c) }); }
        e.on_key(Key::Backspace);
        assert_eq!(label0(&e).as_deref(), Some("i nee"));
        // a real Thai word is not pulled into the English phrase (it may keep the bubble off entirely)
        e.reset();
        typ_th(&mut e, "สวัสดี ");
        typ(&mut e, "need");
        assert!(label0(&e).is_none_or(|l| l == "need"), "{:?}", label0(&e));
    }
    #[test] fn reverse_phrase_backspace_keeps_remaining_words() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ_th(&mut e, "ร กนืะ าืนไ "); assert_eq!(label0(&e).unwrap(), "i dont know");
        e.on_key(Key::Backspace); assert_eq!(e.buffer(), "know"); assert_eq!(label0(&e).unwrap(), "i dont know");
        e.on_key(Key::Backspace); assert_eq!(label0(&e).unwrap(), "i dont kno");
        for _ in 0..3 { e.on_key(Key::Backspace); }
        assert_eq!(e.buffer(), ""); assert_eq!(label0(&e).unwrap(), "i dont", "remaining words still offered");
        // Tab flips the remaining two words
        let o = tab(&mut e); assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "i dont ".into(), then: None });
        // phrase edited down to nothing closes
        e.reset(); typ_th(&mut e, "ร กนืะ "); assert_eq!(label0(&e).unwrap(), "i dont");
        for _ in 0..7 { e.on_key(Key::Backspace); }
        assert!(!e.bubble_visible()); assert_eq!(e.buffer(), "");
    }
    #[test] fn word_and_line_delete() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        // reverse phrase: Option+Backspace removes "know " -> "i dont" still offered, again -> "i", again -> closed
        typ_th(&mut e, "ร กนืะ าืนไ ");
        e.on_key(Key::WordDelete); assert_eq!(label0(&e).unwrap(), "i dont");
        e.on_key(Key::WordDelete); assert!(e.bubble_visible()); assert_eq!(label0(&e).unwrap(), "i");
        let o = e.on_key(Key::WordDelete); assert!(o.actions.contains(&Action::HideBubble)); assert!(!e.bubble_visible());
        // forward: word delete of the only offered word closes
        e.reset(); e.set_thai_active(false); typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        e.on_key(Key::WordDelete); assert!(!e.bubble_visible());
        // mid-word word delete clears the buffer
        e.reset(); typ(&mut e, "hello l;yl"); e.on_key(Key::WordDelete); assert_eq!(e.buffer(), "");
        // Cmd+Backspace: everything gone, closed
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        let o = e.on_key(Key::LineDelete); assert!(!o.swallow); assert!(!e.bubble_visible()); assert_eq!(e.buffer(), "");
        // untracked text (Backspace with an empty history) resets
        e.reset(); let o = e.on_key(Key::Backspace); assert!(!o.swallow); assert!(!e.bubble_visible());
    }
    fn word_offered(e: &mut Engine, thai: bool) -> bool {
        e.reset();
        e.set_thai_active(thai);
        let mut shown = false;
        for c in [Key::Char('o'), Key::Char('k'), Key::Boundary(Boundary::Space)] {
            let o = e.on_key(c);
            shown |= e.bubble_visible() || bubble_items(&o).is_some();
        }
        shown
    }
    #[test]
    fn blocked_ok_is_not_offered_as_na() {
        let Some(mut e) = engine() else { return };
        assert!(word_offered(&mut e, false), "ok is offered as นา before it is blocked");
        e.never.insert("ok".into());
        assert!(!word_offered(&mut e, false), "forward ok stays as typed");
        assert!(!word_offered(&mut e, true), "reverse ok stays as typed");
        e.reset();
        e.set_thai_active(false);
        typ(&mut e, "l;ylfu");
        assert!(e.bubble_visible(), "other words still suggest");
        // a blocked earlier word is left out of a reverse phrase
        e.never.clear();
        e.never.insert("i".into());
        e.set_thai_active(true);
        let mut label = None;
        for c in "i need".chars() {
            let o = e.on_key(if c == ' ' { Key::Boundary(Boundary::Space) } else { Key::Char(c) });
            if let Some(items) = bubble_items(&o) { label = Some(items[0].label.clone()); }
        }
        assert_ne!(label.as_deref(), Some("i need"));
    }
    #[test]
    fn settings_forget_and_reset_keep_the_never_list() {
        let Some(mut e) = engine() else { return };
        assert!(e.apply_settings(SettingsCmd::AddNever("นา".into())));
        assert!(e.blocked("OK"));
        assert!(!e.apply_settings(SettingsCmd::AddNever("two words".into())));
        assert!(!e.apply_settings(SettingsCmd::AddNever("นา".into())), "duplicate is not a change");
        e.personal.state.mem.insert("hi".into(), typethai_core::personal::Mem { kept: 2.0, conv: 1.0, t: 4.0 });
        assert_eq!(e.settings_snapshot().learned[0].word, "hi");
        assert_eq!(e.settings_snapshot().never, vec!["ok".to_string()]);
        assert!(e.apply_settings(SettingsCmd::Forget("HI".into())));
        assert!(e.settings_snapshot().learned.is_empty());
        e.personal.state.mem.insert("hi".into(), typethai_core::personal::Mem { kept: 1.0, conv: 0.0, t: 1.0 });
        assert!(e.apply_settings(SettingsCmd::ResetLearning));
        assert!(e.settings_snapshot().learned.is_empty());
        assert!(e.blocked("ok"), "reset clears learning and keeps the don't-translate list");
        assert!(e.apply_settings(SettingsCmd::SetCompletion(CompleteMode::Typing)));
        assert!(!e.apply_settings(SettingsCmd::SetCompletion(CompleteMode::Typing)), "same mode is not a change");
        assert_eq!(e.settings_snapshot().completion, CompleteMode::Typing);
        e.reset();
        e.set_thai_active(false);
        e.never.clear();
        typ(&mut e, "l;ylfu");
        assert!(e.bubble_visible());
        e.never.insert("l;ylfu".into());
        let hide = e.hide_if_blocked();
        assert!(hide.iter().any(|a| matches!(a, Action::HideBubble)));
        assert!(!e.bubble_visible());
    }
    fn feed(e: &mut Engine, log: &mut Vec<(Option<char>, Outcome)>, text: &str) {
        for c in text.chars() { log.push((Some(c), if c == '\t' { tab(e) } else { e.on_key(key_of(c)) })); }
    }
    fn hot(e: &mut Engine, log: &mut Vec<(Option<char>, Outcome)>, h: Hotkey) { log.push((None, e.on_key(Key::Hotkey(h)))); }
    #[test] fn guess_typo_correction_undoes_cleanly() {
        let Some(mut e) = engine_typo() else { return };
        let mut log = vec![];
        feed(&mut e, &mut log, "riht");
        hot(&mut e, &mut log, Hotkey::Guess);
        let label = label0(&e).expect("guess offered");
        assert_ne!(label, en_to_th("riht"), "the top typo correction is proposed");
        feed(&mut e, &mut log, "\t");
        assert_eq!(screen(&log), label);
        assert_eq!(e.buffer(), "");
        feed(&mut e, &mut log, " ");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "riht ", "undo deletes the corrected word, not its raw Thai reading");
        // undo straight after a mid-word guess gives the buffer back
        e.reset(); log.clear();
        feed(&mut e, &mut log, "riht");
        hot(&mut e, &mut log, Hotkey::Guess);
        feed(&mut e, &mut log, "\t");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "riht");
        assert_eq!(e.buffer(), "riht");
    }
    #[test] fn guess_phrase_with_buffer_undoes_as_one() {
        let Some(mut e) = engine_typo() else { return };
        let mut log = vec![];
        feed(&mut e, &mut log, "w,j l;ylfu");
        hot(&mut e, &mut log, Hotkey::Guess);
        assert_eq!(label0(&e).as_deref(), Some("ไม่ สวัสดี"));
        feed(&mut e, &mut log, "\t");
        assert_eq!(screen(&log), "ไม่ สวัสดี");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "w,j l;ylfu");
        assert_eq!(e.buffer(), "l;ylfu");
    }
    #[test] fn double_tab_guesses_the_last_word() {
        let Some(mut e) = engine_typo() else { return };
        e.opts.suggest_band = 1.1; // no bubble at the first Tab
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu\t");
        assert!(!e.bubble_visible());
        feed(&mut e, &mut log, "\t");
        assert_eq!(label0(&e).as_deref(), Some("สวัสดี"));
        assert_eq!(screen(&log), "l;ylfu\t", "the second Tab only shows the proposal");
        feed(&mut e, &mut log, "\t");
        assert_eq!(screen(&log), "สวัสดี\t");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "l;ylfu\t");
    }
    #[test] fn guess_skips_english_and_goes_stale() {
        let Some(mut e) = engine_typo() else { return };
        typ(&mut e, "hello");
        e.on_key(Key::Hotkey(Hotkey::Guess));
        assert!(!e.bubble_visible(), "plain English proposes nothing");
        e.reset();
        typ(&mut e, "riht");
        e.on_key(Key::Hotkey(Hotkey::Guess));
        assert!(e.bubble_visible());
        e.on_key(Key::Char('x'));
        assert!(!e.bubble_visible(), "a typed key makes the proposal stale");
        let o = tab(&mut e);
        assert!(!o.actions.iter().any(|a| matches!(a, Action::Replace { .. })));
    }
    #[test] fn guess_after_live_accept_measures_from_the_converted_word() {
        let Some(mut e) = engine_typo() else { return };
        e.opts.live = true;
        let mut log = vec![];
        feed(&mut e, &mut log, "w,j l;ylfu");
        assert!(e.live_visible());
        feed(&mut e, &mut log, "\t");
        assert_eq!(screen(&log), "w,j สวัสดี");
        // the next Tab is a plain tab; the one after it must not touch "w,j " through the converted word
        feed(&mut e, &mut log, "\t\t");
        assert_eq!(screen(&log), "w,j สวัสดี\t\t", "nothing left to guess: both Tabs are typed");
        log.clear(); e.reset();
        feed(&mut e, &mut log, "l;ylfu");
        feed(&mut e, &mut log, "\t");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "l;ylfu", "a live conversion undoes back to the typed keys");
    }
    /// A speller that knows a few misspellings.
    struct FakeSpeller;
    impl Speller for FakeSpeller {
        fn guesses(&self, word: &str) -> Option<Vec<String>> {
            let g: &[&str] = match word { "ths" => &["this", "the"], "thsi" => &["this"], "mu" => &["my"], _ => return None };
            Some(g.iter().map(|s| s.to_string()).collect())
        }
    }
    fn labels(e: &Engine) -> Vec<String> { e.bubble.iter().map(|b| b.label.clone()).collect() }
    #[test] fn english_typo_is_offered_and_undone() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        let mut log = vec![];
        feed(&mut e, &mut log, "is ths ");
        assert_eq!(labels(&e), ["this", "the"]);
        feed(&mut e, &mut log, "\t");
        assert_eq!(screen(&log), "is this ");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "is ths ");
        // punctuation around the word stays
        e.reset();
        typ(&mut e, "thsi. ");
        assert_eq!(labels(&e), ["this."]);
        // spelled right: nothing
        e.reset();
        typ(&mut e, "hello ");
        assert!(!e.bubble_visible());
        e.opts.typo_suggestions = false;
        e.reset();
        typ(&mut e, "ths ");
        assert!(!e.bubble_visible(), "typo suggestions off");
    }
    #[test] fn english_spelling_sits_under_the_thai_reading() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        typ(&mut e, "mu ");
        assert_eq!(labels(&e), ["ที", "my"]);
    }
    use crate::rewrite::{Captured, Failure, Job, Preset, Reply, ReqId, Run};
    /// The worker's answer to a capture, with an API key stored.
    fn captured(id: ReqId, sel: Captured) -> Reply { Reply::Captured { id, sel, has_key: true } }
    /// Ctrl+Opt+R: swallowed, hands out a capture job.
    fn start(e: &mut Engine) -> ReqId {
        let o = e.on_key(Key::Hotkey(Hotkey::Rewrite)); assert!(o.swallow);
        match o.actions.last() { Some(Action::Rewrite(Job::Capture(id))) => *id, a => panic!("expected a capture job, got {a:?}") }
    }
    fn job_run(o: &Outcome) -> Run { match o.actions.last() { Some(Action::Rewrite(Job::Run(r))) => r.clone(), a => panic!("expected a run job, got {a:?}") } }
    /// Start, capture `text` and pick the selected preset with Tab.
    fn pending(e: &mut Engine, text: &str) -> Run {
        let id = start(e); e.on_reply(captured(id, Captured::Text(text.into())));
        let o = tab(e); assert!(o.swallow); job_run(&o)
    }
    #[test] fn rewrite_fix_then_paste() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e);
        e.on_reply(captured(id, Captured::Text("helo wrld".into())));
        assert_eq!(labels(&e), ["Fix typos", "Translate TH↔EN"]);
        let o = tab(&mut e); let run = job_run(&o);
        assert_eq!((run.preset, run.text.as_str(), run.cfg.model.as_str()), (Preset::Fix, "helo wrld", "google/gemma-4-26b-a4b-it"));
        assert_eq!(labels(&e), ["Rewriting…"]);
        e.on_reply(Reply::Rewritten(run.req, Ok("hello world".into())));
        assert_eq!(labels(&e), ["hello world"]);
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions, vec![Action::HideBubble, Action::Rewrite(Job::Paste("hello world".into()))]);
        assert!(!e.bubble_visible());
        assert!(!tab(&mut e).swallow, "the session is over: Tab is a tab again");
    }
    #[test] fn rewrite_select_next_or_click_picks_translate() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e); e.on_reply(captured(id, Captured::Text("สวัสดี".into())));
        assert_eq!(e.on_key(Key::SelectNext).actions, vec![Action::Select(1)]);
        assert_eq!(job_run(&tab(&mut e)).preset, Preset::Translate);
        e.on_key(Key::Esc);
        let id = start(&mut e); e.on_reply(captured(id, Captured::Text("สวัสดี".into())));
        let run = job_run(&e.on_key(Key::Hotkey(Hotkey::Accept(1))));
        assert_eq!((run.preset, run.text.as_str()), (Preset::Translate, "สวัสดี"));
    }
    #[test] fn rewrite_typing_cancels_pending_and_late_result_is_ignored() {
        let Some(mut e) = engine() else { return };
        let run = pending(&mut e, "helo");
        let o = e.on_key(Key::Char('a'));
        assert!(!o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
        assert_eq!(e.on_reply(Reply::Rewritten(run.req, Ok("hello".into()))), vec![]);
        assert!(!e.bubble_visible());
    }
    #[test] fn rewrite_space_cancels_choosing() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e); e.on_reply(captured(id, Captured::Text("helo".into())));
        let o = e.on_key(Key::Boundary(Boundary::Space));
        assert!(!o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
    }
    #[test] fn rewrite_esc_while_capturing_drops_capture() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e);
        let o = e.on_key(Key::Esc); assert!(!o.swallow, "nothing on screen to dismiss: Esc reaches the app");
        assert_eq!(e.on_reply(captured(id, Captured::Text("helo".into()))), vec![]);
        assert!(!e.bubble_visible());
    }
    #[test] fn rewrite_newer_request_wins() {
        let Some(mut e) = engine() else { return };
        let old = pending(&mut e, "one");
        e.on_key(Key::Esc);
        let id = start(&mut e);
        e.on_reply(captured(id, Captured::Text("two".into())));
        assert_eq!(e.on_reply(Reply::Rewritten(old.req, Ok("ONE".into()))), vec![]);
        assert_eq!(labels(&e), ["Fix typos", "Translate TH↔EN"]);
        let new = job_run(&tab(&mut e));
        assert_eq!(e.on_reply(Reply::Rewritten(old.req, Ok("ONE".into()))), vec![], "a reply for the earlier run never matches the newer one");
        e.on_reply(Reply::Rewritten(new.req, Ok("TWO".into())));
        assert_eq!(labels(&e), ["TWO"]);
    }
    #[test] fn rewrite_failure_shows_notice_and_tab_dismisses() {
        let Some(mut e) = engine() else { return };
        let run = pending(&mut e, "helo");
        e.on_reply(Reply::Rewritten(run.req, Err(Failure::Rejected)));
        assert_eq!(labels(&e), ["The API key was rejected"]);
        let o = tab(&mut e); assert!(o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
    }
    /// Ctrl+Opt+R and the worker's answer: the labels that come up.
    fn options(e: &mut Engine, sel: Captured) -> Vec<String> { let id = start(e); e.on_reply(captured(id, sel)); labels(e) }
    const AI: [&str; 2] = ["Fix typos", "Translate TH↔EN"];
    #[test] fn nothing_selected_offers_the_word_being_typed() {
        let Some(mut e) = engine_typo() else { return };
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu");
        assert_eq!(options(&mut e, Captured::NoSelection), ["สวัสดี"]);
        log.push((None, tab(&mut e)));
        assert_eq!(screen(&log), "สวัสดี");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "l;ylfu", "applied as a mid-word conversion, so it undoes back to the keys");
    }
    #[test] fn the_word_rows_go_when_the_word_changes() {
        let Some(mut e) = engine_typo() else { return };
        typ(&mut e, "l;ylfu");
        assert_eq!(options(&mut e, Captured::NoSelection), ["สวัสดี"]);
        assert_eq!(e.on_key(Key::Char('j')), Outcome { swallow: false, actions: vec![Action::HideBubble] });
    }
    #[test] fn nothing_selected_brings_back_a_dismissed_word() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        let mut log = vec![];
        feed(&mut e, &mut log, "is ths ");
        e.on_key(Key::Esc); assert!(!e.bubble_visible());
        assert_eq!(options(&mut e, Captured::NoSelection), ["this", "the"]);
        log.push((None, tab(&mut e)));
        assert_eq!(screen(&log), "is this ");
    }
    #[test] fn nothing_selected_fixes_the_spelling_of_the_buffer() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        let mut log = vec![];
        feed(&mut e, &mut log, "is ths");
        assert_eq!(options(&mut e, Captured::NoSelection), ["this", "the"]);
        log.push((None, e.on_key(Key::Hotkey(Hotkey::Accept(1)))));
        assert_eq!(screen(&log), "is the");
        feed(&mut e, &mut log, "x");
        assert_eq!((screen(&log).as_str(), e.buffer()), ("is thex", "thex"), "the fixed word is still the word being typed");
    }
    #[test] fn nothing_selected_offers_english_typed_on_the_thai_layout() {
        let Some(mut e) = engine_typo() else { return };
        typ(&mut e, "i dont "); e.set_thai_active(true); e.reset(); e.set_thai_active(true);
        let mut log = vec![];
        for c in "know".chars() { log.push((en_to_th(&c.to_string()).chars().next(), e.on_key(Key::Char(c)))); }
        assert_eq!(screen(&log), "าืนไ");
        assert_eq!(options(&mut e, Captured::NoSelection), ["know"], "no Thai spelling for what is clearly English");
        log.push((None, tab(&mut e)));
        assert_eq!(screen(&log), "know");
    }
    #[test] fn nothing_selected_after_a_conversion_offers_the_undo() {
        let Some(mut e) = engine_typo() else { return };
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu \t");
        assert_eq!(screen(&log), "สวัสดี ");
        assert_eq!(options(&mut e, Captured::NoSelection), ["l;ylfu"]);
        log.push((None, tab(&mut e)));
        assert_eq!(screen(&log), "l;ylfu ");
    }
    #[test] fn nothing_selected_and_nothing_to_offer_says_so() {
        let Some(mut e) = engine() else { return };
        assert_eq!(options(&mut e, Captured::NoSelection), ["Select text or type a word first"]);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text(" \n ".into())), ["Select text or type a word first"], "a blank selection is no selection");
        e.on_key(Key::Esc);
        typ(&mut e, "hello ");
        assert_eq!(options(&mut e, Captured::NoSelection), ["No suggestions"]);
        e.on_key(Key::Esc);
        typ(&mut e, "hello\n");
        assert_eq!(options(&mut e, Captured::NoSelection), ["Select text or type a word first"], "the word is on the line above");
    }
    #[test] fn a_selection_typed_on_the_wrong_layout_is_offered_converted() {
        let Some(mut e) = engine_typo() else { return };
        assert_eq!(options(&mut e, Captured::Text("l;ylfu".into())), ["สวัสดี", AI[0], AI[1]]);
        let o = tab(&mut e);
        assert_eq!(o, Outcome { swallow: true, actions: vec![Action::HideBubble, Action::Rewrite(Job::Paste("สวัสดี".into()))] });
        assert_eq!(options(&mut e, Captured::Text(en_to_th("i dont know"))), ["i dont know", AI[0], AI[1]]);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("l;ylfu world\n".into())), ["สวัสดี world\n", AI[0], AI[1]], "only the wrong words flip, spacing stays");
    }
    #[test] fn a_selection_with_typos_is_offered_fixed() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        assert_eq!(options(&mut e, Captured::Text("is ths".into())), ["is this", AI[0], AI[1]]);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("รานอาหาร".into())), ["ร้านอาหาร", AI[0], AI[1]]);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("ขอบคุนมาก ประเทษไทย\n".into())), ["ขอบคุณมาก ประเทศไทย\n", AI[0], AI[1]], "a typo inside a run");
    }
    #[test] fn each_selection_row_says_what_it_fixes() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        let reasons = |e: &Engine| e.bubble.iter().map(BubbleItem::reason).collect::<Vec<_>>();
        options(&mut e, Captured::Text("l;ylfu".into()));
        assert_eq!(reasons(&e), [Some(Reason::Layout(InputLang::Thai)), None, None], "to Thai, then the AI rows");
        e.on_key(Key::Esc);
        options(&mut e, Captured::Text(en_to_th("i dont know")));
        assert_eq!(reasons(&e), [Some(Reason::Layout(InputLang::Latin)), None, None], "back to the English keys");
        e.on_key(Key::Esc);
        options(&mut e, Captured::Text("is ths".into()));
        assert_eq!(reasons(&e), [Some(Reason::Typo), None, None]);
    }
    #[test] fn the_bubble_lists_conversions_then_typo_fixes_then_completions() {
        let Some(mut e) = engine() else { return };
        let item = |label: &str, kind| BubbleItem { label: label.into(), kind };
        let (done, fix, flip) = (item("hello", BubbleKind::Complete { typed: "hel".into(), insert: "hello".into() }),
            item("help", BubbleKind::Typo { word: "help".into() }), item("สวัสดี", BubbleKind::Convert { typed: "l;ylfu".into(), thai: "สวัสดี".into() }));
        let mut a = vec![];
        e.show(vec![done.clone(), fix.clone(), flip.clone()], &mut a);
        assert_eq!(a, vec![Action::ShowBubble(vec![flip, fix, done])], "Tab, the digits and clicks count in this order");
    }
    #[test] fn a_correct_selection_gets_only_the_ai_rows_or_a_notice() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        assert_eq!(options(&mut e, Captured::Text("hello world".into())), AI);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("สวัสดีครับ ยินดีที่ได้รู้จัก".into())), AI);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("วันนี้อากาศดีมากเลยนะ".into())), AI);
        e.on_key(Key::Esc);
        let id = start(&mut e); e.on_reply(Reply::Captured { id, sel: Captured::Text("hello world".into()), has_key: false });
        assert_eq!(labels(&e), ["Add an API key in Settings"]);
        e.on_key(Key::Esc);
        let id = start(&mut e); e.on_reply(Reply::Captured { id, sel: Captured::Text("is ths".into()), has_key: false });
        assert_eq!(labels(&e), ["is this"], "no key: the local fix alone, no notice");
    }
    #[test] fn enter_applies_a_fix_offered_by_ctrl_opt_r() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "l;ylfu ");
        let id = start(&mut e); e.on_reply(captured(id, Captured::NoSelection));
        assert_eq!(labels(&e)[0], "สวัสดี");
        let o = e.on_key(Key::Boundary(Boundary::Enter));
        assert!(o.swallow, "Enter confirms instead of typing a newline");
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "สวัสดี ".into(), then: None });
        assert!(!e.bubble_visible());
        e.reset();
        let id = start(&mut e); e.on_reply(captured(id, Captured::Text("l;ylfu".into())));
        assert_eq!(labels(&e)[0], "สวัสดี");
        let o = e.on_key(Key::Boundary(Boundary::Enter));
        assert!(o.swallow); assert!(o.actions.contains(&Action::Rewrite(Job::Paste("สวัสดี".into()))), "{:?}", o.actions);
    }
    #[test] fn enter_is_a_newline_over_other_bubbles_and_notices() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); assert!(e.bubble_visible());
        let o = e.on_key(Key::Boundary(Boundary::Enter));
        assert!(!o.swallow, "a completion bubble: Enter is the user's newline");
        e.reset();
        let id = start(&mut e); e.on_reply(captured(id, Captured::NoSelection));
        assert_eq!(labels(&e), ["Select text or type a word first"]);
        let o = e.on_key(Key::Boundary(Boundary::Enter));
        assert!(!o.swallow, "a notice offers nothing to confirm");
    }
    #[test] fn with_rewrite_off_a_selection_gets_the_local_rows() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller)); e.rewrite.enabled = false;
        assert_eq!(options(&mut e, Captured::Text("is ths".into())), ["is this"]);
        e.on_key(Key::Esc);
        assert_eq!(options(&mut e, Captured::Text("hello world".into())), ["No suggestions"]);
    }
    #[test] fn a_long_selection_skips_the_local_rows() {
        let Some(mut e) = engine() else { return };
        let long = "l;ylfu ".repeat(50);
        assert_eq!(options(&mut e, Captured::Text(long)), AI);
    }
    #[test] fn rewrite_secure_refuses_silently() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e);
        assert_eq!(e.on_reply(captured(id, Captured::Secure)), vec![]);
        assert!(!tab(&mut e).swallow, "no session left");
    }
    #[test] fn rewrite_off_still_captures_and_paused_passes_hotkey() {
        let Some(mut e) = engine() else { return };
        e.rewrite.enabled = false;
        start(&mut e);
        e.on_key(Key::Esc); e.on_key(Key::Hotkey(Hotkey::TogglePause));
        assert_eq!(e.on_key(Key::Hotkey(Hotkey::Rewrite)), Outcome { swallow: false, actions: vec![] });
    }
    #[test] fn rewrite_autorepeat_does_not_restart() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e);
        assert_eq!(e.on_key(Key::Hotkey(Hotkey::Rewrite)), Outcome { swallow: true, actions: vec![] });
        e.on_reply(captured(id, Captured::Text("helo".into())));
        assert_eq!(labels(&e), ["Fix typos", "Translate TH↔EN"]);
    }
    #[test] fn rewrite_tab_while_pending_keeps_waiting() {
        let Some(mut e) = engine() else { return };
        let run = pending(&mut e, "helo");
        assert_eq!(tab(&mut e), Outcome { swallow: true, actions: vec![] });
        assert_eq!(labels(&e), ["Rewriting…"]);
        e.on_reply(Reply::Rewritten(run.req, Ok("hello".into())));
        assert_eq!(labels(&e), ["hello"]);
    }
    #[test] fn rewrite_starts_over_a_suggestion_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        typ(&mut e, "ths "); assert_eq!(labels(&e), ["this", "the"]);
        let o = e.on_key(Key::Hotkey(Hotkey::Rewrite));
        assert!(matches!(o.actions.as_slice(), [Action::HideBubble, Action::Rewrite(Job::Capture(_))]), "{:?}", o.actions);
    }
    // ---------- word completion ----------
    fn completers(e: &mut Engine, mode: CompleteMode) {
        let c = |ws: &[(&str, f64)]| Completer::new(ws.iter().map(|(w, f)| (w.to_string(), *f)));
        e.thai_comp = Some(c(&[("ภาษา", 10.0), ("ภาษี", 5.0), ("ภาษาไทย", 3.0)]));
        e.en_comp = Some(c(&[("help", 30.0), ("hello", 10.0), ("helmet", 5.0)]));
        e.opts.complete = mode;
        // keys read as typed: in three words, any key one slip from them reads as one of them for sure
        e.opts.slips = None;
    }
    fn keys(th: &str) -> String { typethai_core::reverse::th_text_to_keys(th).unwrap() }
    fn idle(e: &mut Engine, ms: u64) -> Vec<Action> { let t = e.last_key.expect("a key is pending"); e.on_idle(t + Duration::from_millis(ms)) }
    #[test] fn completes_a_wrong_layout_thai_prefix_while_typing() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        let mut log = vec![];
        feed(&mut e, &mut log, &keys("ภาษ"));
        assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"]);
        assert_eq!(log[2].1.actions.len(), 1, "one bubble update for the live row and its completions: {:?}", log[2].1.actions);
        assert!(matches!(e.bubble[0].kind, BubbleKind::Live { .. }));
        hot(&mut e, &mut log, Hotkey::Accept(1));
        assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: 3, insert: "ภาษา".into(), then: None });
        assert!(log.last().unwrap().1.actions.contains(&Action::SwitchInput(InputLang::Thai)));
        assert_eq!(e.buffer(), ""); assert_eq!(screen(&log), "ภาษา");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), keys("ภาษ")); assert_eq!(e.buffer(), keys("ภาษ"));
    }
    fn applied(delete: usize, insert: &str, then: Option<Boundary>, lang: InputLang) -> Outcome {
        Outcome { swallow: true, actions: vec![Action::Replace { delete, insert: insert.into(), then }, Action::HideBubble, Action::SwitchInput(lang)] }
    }
    const SWALLOWED: Outcome = Outcome { swallow: true, actions: vec![] };
    #[test] fn tab_held_with_a_digit_applies_that_row() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "nothing is applied while Tab is down");
        assert_eq!(e.on_key(Key::Char('2')), applied(0, "lo", None, InputLang::Latin), "the digit is swallowed");
        assert_eq!(e.on_key(Key::TabUp), SWALLOWED, "the release applies nothing more");
        assert_eq!(e.on_key(Key::Char('2')), Outcome::pass(), "a digit after the release is typed");
        assert_eq!(e.buffer(), "hello2", "glued to the applied word, as after any applied completion");
    }
    #[test] fn tab_with_one_is_tab_alone() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); e.on_key(Key::SelectNext);
        e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Char('1')), applied(0, "p", None, InputLang::Latin), "Tab+1 is row 1");
        e.on_key(Key::TabUp);
        e.reset(); typ(&mut e, "hel");
        e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::TabUp), applied(0, "p", None, InputLang::Latin));
    }
    #[test] fn tab_alone_applies_the_selected_row_once_on_release() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); e.on_key(Key::SelectNext);
        for _ in 0..4 { assert_eq!(e.on_key(Key::Tab), SWALLOWED, "auto-repeat neither applies nor types a tab"); }
        assert_eq!(e.on_key(Key::TabUp), applied(0, "lo", None, InputLang::Latin));
        assert_eq!(e.on_key(Key::Tab), Outcome::pass(), "with the bubble gone, Tab is a tab again");
        assert_eq!(e.on_key(Key::TabUp), Outcome::pass());
    }
    #[test] fn tab_held_with_a_digit_past_the_last_row_does_nothing() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "helm"); assert_eq!(labels(&e), ["helmet"]);
        e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Char('3')), SWALLOWED);
        assert_eq!(e.on_key(Key::TabUp), SWALLOWED, "the digit used up the Tab");
        assert_eq!(labels(&e), ["helmet"]); assert_eq!(e.buffer(), "helm");
    }
    #[test] fn ctrl_digit_picks_a_row_when_the_setting_is_on() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        assert!(e.apply_settings(SettingsCmd::SetPickCtrl(true)));
        assert!(!e.apply_settings(SettingsCmd::SetPickCtrl(true)), "same value is not a change");
        assert!(e.settings_snapshot().pick_ctrl);
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        assert!(e.is_bubble_key(&Key::PickRow(1)));
        assert_eq!(e.on_key(Key::PickRow(1)), applied(0, "lo", None, InputLang::Latin));
        assert_eq!(e.buffer(), "");
        typ(&mut e, " helm"); assert_eq!(labels(&e), ["helmet"]);
        assert_eq!(e.on_key(Key::PickRow(2)), SWALLOWED, "past the last row: swallowed, nothing applied");
        assert_eq!((labels(&e), e.buffer()), (vec!["helmet".to_string()], "helm"));
    }
    #[test] fn ctrl_digit_passes_through_when_off_or_without_a_bubble() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        assert!(!e.is_bubble_key(&Key::PickRow(0)));
        assert_eq!(e.on_key(Key::PickRow(0)), Outcome { swallow: false, actions: vec![Action::HideBubble] }, "off: a shortcut for the app, like any Ctrl key");
        assert_eq!(e.buffer(), "");
        e.apply_settings(SettingsCmd::SetPickCtrl(true));
        typ(&mut e, "hello"); assert!(!e.bubble_visible());
        assert!(!e.is_bubble_key(&Key::PickRow(0)));
        assert_eq!(e.on_key(Key::PickRow(0)), Outcome::pass(), "no bubble: the app gets Ctrl+1");
        assert_eq!(e.buffer(), "");
    }
    #[test] fn digits_are_typed_while_tab_is_held_without_a_bubble() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "hello"); assert!(!e.bubble_visible());
        assert!(!e.on_key(Key::Tab).swallow, "a plain tab");
        assert_eq!(e.on_key(Key::Char('2')), Outcome::pass());
        assert_eq!(e.on_key(Key::TabUp), Outcome::pass());
        assert_eq!(e.buffer(), "2");
    }
    #[test] fn a_key_rolled_over_a_held_tab_lands_after_the_applied_row() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Char('s')), applied(0, "ps", None, InputLang::Latin));
        assert_eq!(e.on_key(Key::TabUp), Outcome::pass());
        assert_eq!(e.buffer(), "helps");
        e.reset(); typ(&mut e, "hel"); e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Boundary(Boundary::Space)), applied(0, "p", Some(Boundary::Space), InputLang::Latin));
        // a Thai row switches the layout, so the rolled key is its Thai character
        e.reset(); typ(&mut e, &keys("ภาษ")); e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Char('k')), applied(3, "ภาษา", None, InputLang::Thai));
        assert!(e.thai_active()); assert_eq!(e.buffer(), keys("ภาษา"));
    }
    fn space_accepts(e: &mut Engine) { assert!(e.apply_settings(SettingsCmd::SetAccept(AcceptKeys::TabOrSpace))); }
    fn plain_space(o: &Outcome) -> bool { !o.swallow && !o.actions.iter().any(|a| matches!(a, Action::Replace { .. })) }
    #[test] fn space_applies_a_live_row_and_types_the_space_once() {
        let Some(mut e) = engine() else { return };
        space_accepts(&mut e);
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu");
        assert_eq!(labels(&e), ["สวัสดี"]); assert!(e.is_bubble_key(&Key::Boundary(Boundary::Space)));
        feed(&mut e, &mut log, " ");
        assert_eq!(log.last().unwrap().1, applied(6, "สวัสดี", Some(Boundary::Space), InputLang::Thai));
        assert_eq!(screen(&log), "สวัสดี ");
        hot(&mut e, &mut log, Hotkey::Undo);
        assert_eq!(screen(&log), "l;ylfu ", "undo restores the word and keeps the space");
    }
    #[test] fn space_over_a_live_row_is_a_boundary_with_tab_only() {
        let Some(mut e) = engine() else { return };
        assert_eq!(e.settings_snapshot().accept, AcceptKeys::Tab);
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu");
        assert_eq!(labels(&e), ["สวัสดี"]); assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Space)));
        feed(&mut e, &mut log, " ");
        assert!(plain_space(&log.last().unwrap().1));
        assert_eq!(screen(&log), "l;ylfu ");
        assert_eq!(e.bubble[0].kind, BubbleKind::Convert { typed: "l;ylfu".into(), thai: "สวัสดี".into() }, "offered after Space, as before");
    }
    #[test] fn space_over_completions_is_a_boundary() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing); space_accepts(&mut e);
        let mut log = vec![];
        feed(&mut e, &mut log, "hel");
        assert_eq!(labels(&e), ["help", "hello", "helmet"]); assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Space)));
        feed(&mut e, &mut log, " ");
        assert!(plain_space(&log.last().unwrap().1));
        assert_eq!(screen(&log), "hel ");
    }
    #[test] fn space_applies_the_live_row_only_while_it_is_highlighted() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing); space_accepts(&mut e);
        typ(&mut e, &keys("ภาษ")); assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"]);
        e.on_key(Key::SelectNext);
        assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Space)));
        assert!(plain_space(&e.on_key(Key::Boundary(Boundary::Space))), "a completion is highlighted");
        e.reset(); typ(&mut e, &keys("ภาษ"));
        e.on_key(Key::SelectNext); e.on_key(Key::SelectPrev);
        assert_eq!(e.on_key(Key::Boundary(Boundary::Space)), applied(3, "ภาษ", Some(Boundary::Space), InputLang::Thai));
    }
    #[test] fn space_applies_a_reverse_live_row() {
        let Some(mut e) = engine() else { return };
        space_accepts(&mut e); e.opts.rev_live = 0.5;
        typ_th(&mut e, "าืนไ"); assert_eq!(labels(&e), ["know"]);
        assert_eq!(e.on_key(Key::Boundary(Boundary::Space)), applied(4, "know", Some(Boundary::Space), InputLang::Latin));
        assert!(!e.thai_active(), "the next word is typed on the English layout");
    }
    #[test] fn tab_still_applies_with_space_accept_on() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing); space_accepts(&mut e);
        typ(&mut e, "l;ylfu");
        assert_eq!(tab(&mut e), applied(6, "สวัสดี", None, InputLang::Thai));
        e.reset(); typ(&mut e, "hel"); e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Char('2')), applied(0, "lo", None, InputLang::Latin), "Tab held + 2");
        e.on_key(Key::TabUp);
        e.reset(); typ(&mut e, "hel"); e.on_key(Key::Tab);
        assert_eq!(e.on_key(Key::Boundary(Boundary::Space)), applied(0, "p", Some(Boundary::Space), InputLang::Latin), "Space rolled over a held Tab applies any row");
    }
    #[test] fn space_accept_with_auto_convert_converts_once() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true; space_accepts(&mut e);
        let mut log = vec![];
        feed(&mut e, &mut log, "l;ylfu ");
        assert_eq!(log.last().unwrap().1, applied(6, "สวัสดี", Some(Boundary::Space), InputLang::Thai));
        assert_eq!(screen(&log), "สวัสดี ");
    }
    #[test] fn completes_the_last_word_of_a_thai_run() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        let mut log: Vec<(Option<char>, Outcome)> = vec![];
        e.set_thai_active(true);
        for c in keys("ฉันชอบภาษ").chars() { log.push((en_to_th(&c.to_string()).chars().next(), e.on_key(Key::Char(c)))); }
        assert_eq!(screen(&log), "ฉันชอบภาษ");
        assert_eq!(labels(&e), ["ภาษา", "ภาษี", "ภาษาไทย"], "labelled with the completed word only");
        assert_eq!(e.bubble[0].kind, BubbleKind::Complete { typed: keys("ฉันชอบภาษ"), insert: "ฉันชอบภาษา".into() });
        log.push((None, tab(&mut e)));
        assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: 0, insert: "า".into(), then: None });
        assert_eq!(screen(&log), "ฉันชอบภาษา");
        // English typed on the Thai layout keeps only its reverse live row
        e.reset(); e.opts.rev_live = 0.5; e.thai_comp = Some(Completer::new([("าืนไก".to_string(), 9.0)]));
        typ_th(&mut e, "าืนไ");
        assert_eq!(labels(&e), ["know"]);
        e.reset(); e.opts.rev_live = 1.1; typ_th(&mut e, "าืนไ");
        assert_eq!(labels(&e), ["าืนไก"], "the same run completes as Thai when it is not read as English");
    }
    #[test] fn completes_english_while_typing_above_the_confidence_gate() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "he"); assert!(!e.bubble_visible(), "2 letters are too few while typing");
        typ(&mut e, "l"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        let mut log = vec![]; feed(&mut e, &mut log, "\t");
        assert_eq!(log[0].1.actions, vec![Action::Replace { delete: 0, insert: "p".into(), then: None }, Action::HideBubble, Action::SwitchInput(InputLang::Latin)]);
        e.reset(); e.opts.complete_conf = 0.99;
        let o = typ(&mut e, "hel"); assert!(o.iter().all(|x| bubble_items(x).is_none())); assert!(!e.bubble_visible());
        // a finished word is not a confident prefix of its extensions
        e.reset(); e.opts.complete_conf = 0.4; e.en_comp = Some(Completer::new([("you".to_string(), 100.0), ("your".to_string(), 50.0)]));
        typ(&mut e, "you"); assert!(!e.bubble_visible());
        e.reset(); e.en_comp = Some(Completer::new([("you".to_string(), 10.0), ("your".to_string(), 50.0)]));
        typ(&mut e, "you"); assert_eq!(labels(&e), ["your"]);
    }
    #[test] fn pause_mode_offers_completions_once_after_the_idle_time() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        let o = typ(&mut e, "hel"); assert!(o.iter().all(|x| x.actions.is_empty()));
        assert_eq!(idle(&mut e, 100), vec![]);
        let o = idle(&mut e, 600);
        assert_eq!(o.len(), 1); assert_eq!(bubble_items(&Outcome { swallow: false, actions: o }).unwrap().iter().map(|b| b.label.as_str()).collect::<Vec<_>>(), ["help", "hello", "helmet"]);
        assert_eq!(e.on_idle(Instant::now() + Duration::from_secs(5)), vec![], "shown once per pause");
        // the next key updates the rows in place; the pause after it has nothing to add
        let o = e.on_key(Key::Char('m')); assert_eq!(o.actions, vec![Action::ShowBubble(vec![complete("helm", "helmet")])]);
        assert_eq!(idle(&mut e, 600), vec![]);
        e.on_key(Key::Backspace); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        assert_eq!(idle(&mut e, 600), vec![]);
        // a boundary takes them away and offers nothing for the finished word
        let o = e.on_key(Key::Boundary(Boundary::Space)); assert_eq!(o.actions, vec![Action::HideBubble]);
        assert_eq!(e.on_idle(Instant::now() + Duration::from_secs(5)), vec![]);
    }
    fn complete(typed: &str, insert: &str) -> BubbleItem { BubbleItem { label: insert.into(), kind: BubbleKind::Complete { typed: typed.into(), insert: insert.into() } } }
    #[test] fn a_key_with_pause_rows_up_updates_them_in_place() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        assert_eq!(e.on_key(Key::Char('m')), Outcome { swallow: false, actions: vec![Action::ShowBubble(vec![complete("helm", "helmet")])] }, "one update, no hide");
        assert_eq!(e.on_key(Key::Char('e')), Outcome { swallow: false, actions: vec![Action::ShowBubble(vec![complete("helme", "helmet")])] });
        assert_eq!(e.on_key(Key::Char('x')), Outcome { swallow: false, actions: vec![Action::HideBubble] }, "nothing left for helmex");
        assert_eq!(idle(&mut e, 600), vec![]);
        assert_eq!(e.on_key(Key::Char('q')), Outcome::pass(), "no rows up: the next key waits for the pause again");
    }
    #[test] fn a_key_keeps_the_wrong_layout_thai_rows_under_the_live_row() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        typ(&mut e, &keys("ภาษ")); assert_eq!(labels(&e), ["ภาษ"]);
        idle(&mut e, 600); assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"]);
        let o = e.on_key(Key::Char(keys("า").chars().next().unwrap()));
        assert!(matches!(o.actions.as_slice(), [Action::ShowBubble(_)]), "{:?}", o.actions);
        assert_eq!(labels(&e), ["ภาษา", "ภาษาไทย"], "the live row follows the word and the rows stay under it");
    }
    /// Counts the spell checker's completion calls.
    struct CountingSpeller(Arc<std::sync::atomic::AtomicUsize>);
    impl Speller for CountingSpeller {
        fn guesses(&self, _: &str) -> Option<Vec<String>> { None }
        fn completions(&self, p: &str) -> Vec<String> { self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst); if p.starts_with("hel") { vec!["helium".into()] } else { vec![] } }
    }
    #[test] fn a_key_keeps_spell_checker_rows_that_still_fit_without_calling_it() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        e.en_comp = Some(Completer::new([("help".to_string(), 30.0)])); e.speller = Some(Box::new(CountingSpeller(calls.clone())));
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["help", "helium"]);
        let before = calls.load(std::sync::atomic::Ordering::SeqCst);
        assert_eq!(e.on_key(Key::Char('i')).actions, vec![Action::ShowBubble(vec![BubbleItem { label: "helium".into(), kind: BubbleKind::Complete { typed: "heli".into(), insert: "helium".into() } }])]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before, "no spell checker on a key");
        idle(&mut e, 600); assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before + 1, "the pause tops up");
        assert_eq!(labels(&e), ["helium"]);
    }
    #[test] fn pause_mode_waits_for_the_configured_delay() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        assert!(e.apply_settings(SettingsCmd::SetCompleteIdle(Duration::from_millis(150))));
        assert!(!e.apply_settings(SettingsCmd::SetCompleteIdle(Duration::from_millis(150))), "same delay is not a change");
        assert_eq!(e.settings_snapshot().complete_idle, Duration::from_millis(150));
        typ(&mut e, "hel");
        assert_eq!(idle(&mut e, 149), vec![], "just before the delay");
        assert_eq!(idle(&mut e, 150).len(), 1, "at the delay");
        assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.apply_settings(SettingsCmd::SetCompleteIdle(Duration::from_millis(600)));
        e.reset(); typ(&mut e, "hel");
        assert_eq!(idle(&mut e, 599), vec![], "a longer delay waits longer");
        assert_eq!(idle(&mut e, 600).len(), 1);
    }
    #[test] fn an_accepted_completion_is_edited_as_the_word() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); tab(&mut e);
        e.on_key(Key::Backspace); assert_eq!(e.buffer(), "hel");
        assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.reset(); typ(&mut e, "hel"); tab(&mut e); typ(&mut e, "er");
        assert_eq!(e.buffer(), "helper");
        e.reset(); typ(&mut e, "hel"); tab(&mut e); e.on_key(Key::Boundary(Boundary::Space));
        e.on_key(Key::Backspace); e.on_key(Key::Backspace); assert_eq!(e.buffer(), "hel");
        e.reset(); e.set_thai_active(true); typ_th(&mut e, "ภาษ"); tab(&mut e);
        e.on_key(Key::Backspace); assert_eq!(e.buffer(), keys("ภาษ"));
        assert_eq!(labels(&e), ["ภาษา", "ภาษี", "ภาษาไทย"]);
        tab(&mut e); typ_th(&mut e, "ไทย"); assert_eq!(e.buffer(), keys("ภาษาไทย"));
    }
    struct FixingSpeller;
    impl Speller for FixingSpeller { fn guesses(&self, w: &str) -> Option<Vec<String>> { matches!(w, "ths" | "thiss").then(|| vec!["this".into()]) } }
    #[test] fn a_typo_row_retypes_only_what_differs() {
        let Some(mut e) = engine_typo() else { return };
        e.opts.auto_convert = true;
        let mut log = vec![]; feed(&mut e, &mut log, "l;ylf ");
        assert_eq!(labels(&e), ["l;ylf", "สวัสดี", "สวัสดิ", "วัด"]);
        hot(&mut e, &mut log, Hotkey::Accept(1));
        assert_eq!(log.last().unwrap().1, applied(1, "ี ", None, InputLang::Thai), "only the space goes; ด stays");
        assert_eq!(screen(&log), "สวัสดี ");
        e.reset(); e.opts.auto_convert = false; e.speller = Some(Box::new(FixingSpeller));
        let mut log = vec![]; feed(&mut e, &mut log, "ths ");
        assert_eq!(labels(&e), ["this"]);
        hot(&mut e, &mut log, Hotkey::Accept(0));
        assert_eq!(log.last().unwrap().1, applied(2, "is ", None, InputLang::Latin));
        assert_eq!(screen(&log), "this ");
    }
    #[test] fn a_fix_that_changes_only_spacing_keeps_the_layout() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FixingSpeller));
        let mut log = vec![]; feed(&mut e, &mut log, "thiss ");
        hot(&mut e, &mut log, Hotkey::Accept(0));
        assert_eq!(log.last().unwrap().1, Outcome { swallow: true, actions: vec![Action::Replace { delete: 2, insert: " ".into(), then: None }, Action::HideBubble] }, "no input source switch");
        assert_eq!(screen(&log), "this ");
    }
    #[test] fn a_completion_in_a_terminal_keeps_the_text_before_the_word() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        e.delete_unit = DeleteUnit::Grapheme;
        let mut log = vec![]; feed(&mut e, &mut log, "ok ");
        e.set_thai_active(true);
        for c in keys("ฉันชอบภาษ").chars() { log.push((en_to_th(&c.to_string()).chars().next(), e.on_key(Key::Char(c)))); }
        log.push((None, tab(&mut e)));
        assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: 0, insert: "า".into(), then: None });
        assert_eq!(screen_in(DeleteUnit::Grapheme, &log), "ok ฉันชอบภาษา");
    }
    #[test] fn undo_counts_backspaces_in_the_apps_unit() {
        for (unit, n) in [(DeleteUnit::CodePoint, 6), (DeleteUnit::Grapheme, 4)] {
            let Some(mut e) = engine() else { return };
            e.delete_unit = unit;
            let mut log = vec![]; feed(&mut e, &mut log, "l;ylfu\t");
            assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: None });
            hot(&mut e, &mut log, Hotkey::Undo);
            assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: n, insert: "l;ylfu".into(), then: None }, "{unit:?}: ส|วั|ส|ดี");
            assert_eq!(screen_in(unit, &log), "l;ylfu");
        }
    }
    #[test] fn a_reverse_conversion_counts_backspaces_in_the_apps_unit() {
        for (unit, n) in [(DeleteUnit::CodePoint, 4), (DeleteUnit::Grapheme, 3)] {
            let Some(mut e) = engine() else { return };
            e.delete_unit = unit; e.opts.rev_live = 0.5;
            e.set_thai_active(true);
            let mut log = vec![];
            for c in keys("าืนไ").chars() { log.push((en_to_th(&c.to_string()).chars().next(), e.on_key(Key::Char(c)))); }
            assert_eq!(labels(&e), ["know"]);
            log.push((None, tab(&mut e)));
            assert_eq!(log.last().unwrap().1.actions[0], Action::Replace { delete: n, insert: "know".into(), then: None }, "{unit:?}: าื|น|ไ");
            assert_eq!(screen_in(unit, &log), "know");
        }
    }
    struct CompletingSpeller;
    impl Speller for CompletingSpeller {
        fn guesses(&self, _: &str) -> Option<Vec<String>> { None }
        fn completions(&self, p: &str) -> Vec<String> { if p == "hel" { vec!["helium".into()] } else { vec![] } }
    }
    #[test] fn the_spell_checker_tops_up_only_after_a_pause() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        e.en_comp = Some(Completer::new([("help".to_string(), 30.0)])); e.speller = Some(Box::new(CompletingSpeller));
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help"], "no spell checker on a keystroke");
        e.reset(); e.opts.complete = CompleteMode::Pause;
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["help", "helium"]);
    }
    #[test] fn esc_mutes_completions_until_the_word_ends() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        typ(&mut e, "hel"); idle(&mut e, 600); assert!(e.bubble_visible());
        let o = e.on_key(Key::Esc); assert!(o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]);
        typ(&mut e, "m"); assert_eq!(idle(&mut e, 600), vec![]); assert!(!e.bubble_visible());
        typ(&mut e, "x hel"); idle(&mut e, 600);
        assert_eq!(labels(&e), ["help", "hello", "helmet"]);
    }
    #[test] fn off_mode_never_completes() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Off);
        typ(&mut e, "hel"); assert_eq!(idle(&mut e, 600), vec![]); assert!(!e.bubble_visible());
        e.reset(); typ(&mut e, &keys("ภาษ")); idle(&mut e, 600);
        assert_eq!(labels(&e), ["ภาษ"], "only the live conversion");
        completers(&mut e, CompleteMode::Typing); e.reset(); typ(&mut e, &keys("ภาษ"));
        assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"], "same engine, completions on");
    }
    #[test] fn finished_words_rank_first() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.reset(); typ(&mut e, "helmet helmet helmet hel");
        assert_eq!(labels(&e), ["helmet", "help", "hello"]);
    }
    #[test] fn a_new_word_is_offered_after_three_finishes() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "helix helix hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.reset(); typ(&mut e, "helix hel"); assert_eq!(labels(&e), ["helix", "help", "hello"]);
    }
    #[test] fn an_accepted_completion_is_learned() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        for _ in 0..3 {
            typ(&mut e, "hel");
            let i = labels(&e).iter().position(|l| l == "helmet").expect("helmet offered");
            e.on_key(Key::Hotkey(Hotkey::Accept(i))); typ(&mut e, " ");
        }
        typ(&mut e, "x hel"); assert_eq!(labels(&e), ["helmet", "help", "hello"]);
    }
    #[test] fn learns_letters_only_words_without_their_punctuation() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "helium9 helium9 helium9 hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.reset(); typ(&mut e, "(Helium), helium. helium! hel"); assert_eq!(labels(&e), ["helium", "help", "hello"]);
        e.reset(); e.never.insert("helmet".into());
        typ(&mut e, "helmet helmet helmet hel"); assert_eq!(labels(&e), ["helium", "help", "hello"], "blocked words are not learned");
    }
    #[test] fn a_converted_word_is_learned_as_its_result() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ_th(&mut e, "ภาษ"); assert_eq!(labels(&e), ["ภาษา", "ภาษี", "ภาษาไทย"]);
        e.reset(); e.set_thai_active(false);
        typ(&mut e, &format!("{} ", keys("ภาษี"))); e.on_key(Key::Hotkey(Hotkey::ForceConvert)); typ(&mut e, "a");
        e.reset(); typ_th(&mut e, "ภาษ"); assert_eq!(labels(&e), ["ภาษี", "ภาษา", "ภาษาไทย"]);
    }
    #[test] fn learning_off_neither_learns_nor_ranks_by_memory() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        assert!(e.apply_settings(SettingsCmd::SetLearnWords(false)));
        assert!(!e.apply_settings(SettingsCmd::SetLearnWords(false)), "same value is not a change");
        typ(&mut e, "helmet helmet helmet hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        e.apply_settings(SettingsCmd::SetLearnWords(true));
        e.reset(); typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"], "nothing was learned while off");
        e.reset(); typ(&mut e, "helmet helmet helmet hel"); assert_eq!(labels(&e), ["helmet", "help", "hello"]);
        e.apply_settings(SettingsCmd::SetLearnWords(false));
        e.reset(); typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"], "what was learned is not used while off");
        assert!(!e.settings_snapshot().learn_words);
    }
    #[test] fn forget_clears_learned_words() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        typ(&mut e, "helmet helmet helmet hel"); assert_eq!(labels(&e), ["helmet", "help", "hello"]);
        let saves = e.events_since_save;
        assert_eq!(e.settings_snapshot().words_learned, 1);
        assert!(e.apply_settings(SettingsCmd::ForgetWords));
        assert_eq!(e.settings_snapshot().words_learned, 0);
        assert_eq!(e.events_since_save, saves + 1, "a forget is saved");
        e.reset(); typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
    }
    #[test] fn the_previous_word_ranks_completions() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        e.opts.learn_words = false;
        let pairs = |v: &[(&str, &str)]| Some(Bigrams::new(v.iter().map(|(p, n)| (p.to_string(), n.to_string(), 5.0))));
        e.en_bigrams = pairs(&[("my", "helmet")]);
        e.thai_bigrams = pairs(&[("ชอบ", "ภาษี")]);
        typ(&mut e, "my hel"); assert_eq!(labels(&e), ["helmet", "help", "hello"]);
        e.reset(); typ(&mut e, "my\nhel"); assert_eq!(labels(&e), ["help", "hello", "helmet"], "not across a line break");
        e.reset(); typ(&mut e, "hel"); assert_eq!(labels(&e), ["help", "hello", "helmet"]);
        // Thai: the dictionary word before the tail inside the run, else the word before the run
        e.reset(); typ_th(&mut e, "ภาษ"); assert_eq!(labels(&e), ["ภาษา", "ภาษี", "ภาษาไทย"]);
        e.reset(); typ_th(&mut e, "ฉันชอบภาษ"); assert_eq!(labels(&e), ["ภาษี", "ภาษา", "ภาษาไทย"]);
        e.reset(); typ_th(&mut e, "ชอบ ภาษ"); assert_eq!(labels(&e), ["ภาษี", "ภาษา", "ภาษาไทย"]);
        e.reset(); typ_th(&mut e, "ชอบมากภาษ"); assert_eq!(labels(&e), ["ภาษา", "ภาษี", "ภาษาไทย"]);
    }
    /// `cargo test --release -p typethai-desktop completion_cost -- --ignored --nocapture`: per-keystroke cost of the
    /// Typing-mode rows on the real model, and each prefix's top word and confidence.
    #[test] #[ignore] fn completion_cost() {
        let (Some(mut m), Some(mut e)) = (model(), engine()) else { return };
        e.opts.complete = CompleteMode::Typing; e.load_completers(&mut m); e.speller = crate::platform::spell::system();
        let (th, en) = (m.thai_completer(), m.english_completer());
        for p in ["hel", "wor", "tha", "pro", "com", "beca", "thi", "sta", "goo", "abo"] { let c = en.complete(p, 1, Context::default()); println!("en {p}\t{:?}\t{:.2}", c.words, c.confidence) }
        for p in ["ภาษ", "สวั", "ขอบ", "ประ", "ความ", "โรงเ", "มหา", "คอม", "อร่", "ทำง"] { let c = th.complete(p, 1, Context::default()); println!("th {p}\t{:?}\t{:.2}", c.words, c.confidence) }
        let run = keys("วันนี้ฉันอยากไปกินข้าวที่ร้านอาห");
        let cases: [(&str, bool, String); 8] = [("en hel", false, "hel".into()), ("en the hel", false, "the hel".into()), ("en becau", false, "becau".into()), ("en a candel", false, "a candel".into()),
            ("en the tje", false, "the tje".into()), ("wrong-layout ภาษ", false, keys("ภาษ")), ("thai slip สวัสก", true, keys("สวัสก")), ("thai run 30", true, run.clone())];
        let measure = |e: &mut Engine| {
            for (name, thai, k) in &cases {
                e.reset(); e.set_thai_active(*thai); typ(e, k);
                let up = e.completions_up();
                for ask in [Ask::Typing, Ask::Refresh] {
                    let mut ns: Vec<u128> = (0..2000).map(|_| { let t = Instant::now(); std::hint::black_box(e.completion_rows(ask, &up)); t.elapsed().as_nanos() }).collect();
                    ns.sort_unstable();
                    println!("{name} {ask:?}: rows {:?} p50 {:.1} us p99 {:.1} us", labels(e), ns[1000] as f64 / 1e3, ns[1980] as f64 / 1e3);
                }
            }
            // whole keystrokes (live row included) over the 30-char run on the English layout, with and without completion;
            // Pause: each key comes with the rows of a pause up, the in-place refresh
            for (mode, thai) in [(CompleteMode::Off, false), (CompleteMode::Typing, false), (CompleteMode::Pause, false), (CompleteMode::Off, true), (CompleteMode::Typing, true), (CompleteMode::Pause, true)] {
                e.opts.complete = mode;
                let mut ns: Vec<u128> = vec![];
                for _ in 0..50 { e.reset(); e.set_thai_active(thai); for c in run.chars() {
                    if mode == CompleteMode::Pause && e.completions_up().is_empty() && !e.buffer().is_empty() { e.on_idle(Instant::now() + Duration::from_secs(1)); }
                    let t = Instant::now(); std::hint::black_box(e.on_key(Key::Char(c))); ns.push(t.elapsed().as_nanos())
                } }
                ns.sort_unstable();
                println!("on_key {mode:?} {}: p50 {:.1} us p99 {:.1} us (n={})", if thai { "thai layout" } else { "en layout" }, ns[ns.len() / 2] as f64 / 1e3, ns[ns.len() * 99 / 100] as f64 / 1e3, ns.len());
            }
        };
        // Ctrl+Opt+R over the longest selection the local rows read: spell checker and corrector, on the key thread
        e.corrector = Some(m.corrector());
        for (name, text) in [("en", "teh quick brwon fox jumsp ovre the lazy dog "), ("th", "เมื่อวานฉันไปตลาดกับแม่ ")] {
            let text: String = text.repeat(20).chars().take(SELECTION_LOCAL_MAX).collect();
            let t = Instant::now(); let rows = e.selection_rows(&text);
            println!("selection {name} {} chars: {} rows in {:.1} ms", text.chars().count(), rows.len(), t.elapsed().as_secs_f64() * 1e3);
        }
        for w in ["teh", "the", "brwon", "quick"] {
            let t = Instant::now(); let g = e.speller.as_ref().and_then(|s| s.guesses(w)).map_or(0, |g| g.len());
            println!("spell checker guesses({w}): {g} in {:.2} ms", t.elapsed().as_secs_f64() * 1e3);
        }
        e.corrector = None;
        println!("-- no context");
        measure(&mut e);
        // worst case for the context path: a memory at its caps, and 20k corpus followers after the previous word
        let (mut en_words, mut th_words): (Vec<&String>, Vec<&String>) = (m.english_freq.keys().collect(), m.thai_words.iter().collect());
        en_words.sort(); th_words.sort();
        for (i, w) in en_words.iter().take(2500).chain(th_words.iter().take(2500)).enumerate() {
            for j in 0..2 { Arc::make_mut(&mut e.words).learn(Some(["the", "ที่"][(i + j) % 2]), w) }
        }
        let followers = |prev: &str, ws: &[&String]| Bigrams::new(ws.iter().take(20_000).enumerate().map(|(i, w)| (prev.to_string(), w.to_string(), 1.0 + i as f64 % 97.0)));
        (e.en_bigrams, e.thai_bigrams) = (Some(followers("the", &en_words)), Some(followers("ที่", &th_words)));
        println!("-- memory of {} words, followers: the {} ที่ {}", e.words.len(), e.en_bigrams.as_ref().unwrap().followers("the").len(), e.thai_bigrams.as_ref().unwrap().followers("ที่").len());
        let t = Instant::now(); let json = e.words.to_json();
        println!("words.json: {} KB, to_json {:.2} ms", json.len() / 1024, t.elapsed().as_secs_f64() * 1e3);
        e.opts.complete = CompleteMode::Typing;
        measure(&mut e);
    }
    /// Flags `candel` with the guesses the macOS checker gives, in its order.
    struct CandelSpeller;
    impl Speller for CandelSpeller { fn guesses(&self, w: &str) -> Option<Vec<String>> { (w == "candel").then(|| ["cancel", "candle", "Handel"].map(String::from).to_vec()) } }
    #[test] fn a_slipped_key_still_reaches_the_word_meant() {
        let (Some(mut m), Some(mut e)) = (model(), engine_typo()) else { return };
        e.opts.complete = CompleteMode::Pause; e.load_completers(&mut m); e.speller = Some(Box::new(CandelSpeller));
        typ(&mut e, "candel"); idle(&mut e, 600);
        assert_eq!(labels(&e), ["candles", "candle", "cancel"], "candle is el swapped; mid-word the word may go on");
        e.opts.slips = None;
        e.reset(); typ(&mut e, "candel"); assert_eq!(idle(&mut e, 600), vec![], "read as typed, no word starts with candel");
        e.opts.slips = Some(Slips::default());
        e.reset(); typ(&mut e, "light a candel "); assert_eq!(labels(&e), ["candle", "cancel", "Handel"]);
        e.reset(); typ(&mut e, "we must candel "); assert_eq!(labels(&e), ["cancel", "candle", "Handel"], "the previous word decides");
        // Thai on the Thai layout, completed mid-word; Thai typed on the English layout, fixed under its conversion
        e.reset(); e.set_thai_active(true); typ(&mut e, &keys("สวัสก")); idle(&mut e, 600);
        assert_eq!(labels(&e), ["สวัสดี", "สวัสดิ์", "สวัสดิภาพ"], "ก is next to ด");
        e.reset(); e.set_thai_active(false); typ(&mut e, &format!("{} ", keys("โรงเรยีน")));
        assert_eq!(labels(&e), ["โรงเรยีน", "โรงเรียน", "โรงเรือน"], "ยี swapped");
        e.reset(); typ(&mut e, &format!("{} ", keys("ร้านอาหร"))); assert_eq!(labels(&e), ["ร้านอาหร", "ร้านอาหาร"], "า left out");
    }
    #[test] fn model_completers_skip_english_without_its_frequencies() {
        let Some(mut m) = model() else { return };
        let Some(mut e) = engine() else { return };
        e.opts.complete = CompleteMode::Pause;
        e.load_completers(&mut m);
        typ(&mut e, &keys("ภาษ")); idle(&mut e, 600);
        assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"]);
        e.reset(); typ(&mut e, "hel"); idle(&mut e, 600);
        if !m.english_freq.is_empty() { assert_eq!(labels(&e), ["help", "hello", "hell"]); }
        m.english_freq.clear(); e.load_completers(&mut m);
        e.reset(); typ(&mut e, "hel"); assert_eq!(idle(&mut e, 600), vec![], "no English frequencies: no English completion");
        e.reset(); typ(&mut e, &keys("ภาษ")); idle(&mut e, 600);
        assert_eq!(labels(&e), ["ภาษ", "ภาษา", "ภาษี"], "Thai still completes");
    }
}
