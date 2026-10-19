//! Platform-independent typing engine: per-word key buffer -> decisions -> actions for the platform layer.
//! The platform layer feeds `Key`s (already mapped to the US-QWERTY character the key would produce), and executes the
//! returned `Action`s (delete N chars + inject text, show/hide the suggestion bubble). Fully unit-testable on Linux.
use crate::backspace::{self, DeleteUnit};
use crate::bridge::BubbleLayout;
use crate::candidate::{src_of_kind, Candidate, Floors, Src};
use crate::predict;
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use crate::rank::{Ctx, How, RankLog, Row};
use crate::rewrite::{self, Captured, Job, Preset, Reply, ReqId, Run};
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use typethai_core::{completer::{thai_tail, Bigrams, Completer, Completions, Context, WordMemory}, slip::Slips, reverse::{RevDecision, Reverse}, corrector::Corrector, detector::Detector, keymap::en_to_th, personal::{Feedback, PersonalModel}, reverse::th_text_to_keys, TokenDecision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Boundary { Space, Enter, Tab }
impl Boundary { pub fn text(self) -> &'static str { match self { Boundary::Space => " ", Boundary::Enter => "\n", Boundary::Tab => "\t" } } }
/// English spell checker (the OS one; see `platform::spell`). `before` is the text on the line in front of the word
/// (it ends where the word starts): macOS ranks its guesses and completions by it.
pub trait Speller {
    /// None when `word` is spelled right (or cannot be checked); else the guesses, best first.
    fn guesses(&self, before: &str, word: &str) -> Option<Vec<String>>;
    /// Words that start with `prefix`, best first (empty where the OS has no completion API).
    fn completions(&self, _before: &str, _prefix: &str) -> Vec<String> { vec![] }
}
/// How much of the line in front of a word the spell checker is given, in bytes.
const SPELL_CONTEXT: usize = 200;
/// Marks a next-word query's key: the completion queries never set this bit, so an answer arriving on the shared
/// channel can be told apart.
const NEXT_WORD_KEY: u64 = 1 << 63;
/// How much of the line before a word the predictor is given, in characters (the model reads a recent tail).
const PREDICT_BEFORE_CHARS: usize = 2000;
/// The query the predictor is asked for a word: the line, the head kept in front of it, and the word being typed, so
/// the same state is asked once and an answer for a state the user typed past is dropped.
fn query_key(line: &str, head: &str, tail: &str, thai: bool) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (line, head, tail, thai).hash(&mut h);
    h.finish()
}
/// The end of `text` the spell checker reads as a word's context: its last line, at most `SPELL_CONTEXT` bytes.
fn spell_context(text: &str) -> &str {
    let line = &text[text.rfind('\n').map_or(0, |i| i + 1)..];
    let mut at = line.len().saturating_sub(SPELL_CONTEXT);
    while !line.is_char_boundary(at) { at += 1 }
    &line[at..]
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hotkey { Undo, ForceConvert, Guess, Accept(usize), Dismiss, TogglePause,
    /// Ctrl+Opt/Alt+R: options for the selected text (other layout, spelling, AI rewrite), or with nothing selected for
    /// the word before the caret.
    Rewrite,
    /// A thumb clicked on option `i` while Shift is held: up (`true`) or down. Only a conversion of a word typed on the
    /// English keys is rated (`BubbleItem::rated_word`): down puts that word's conversion last from now on
    /// (`Engine::offer_last`), up puts it back in its place. Neither stops it being offered.
    Rate(usize, bool) }
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
    /// A plain arrow key. Navigation, except with `Options::pick_arrows`: Up moves into a visible bubble, then
    /// Left/Right move the selection, Up or Enter applies it and Down leaves.
    Arrow(Arrow),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrow { Up, Down, Left, Right }
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
/// A bubble row as the rank log keeps it: the text that would be applied, why it was offered (`th`, `en`, `typo`;
/// None for a completion, a next-word row or an AI row), the score its source computed, and the source itself.
fn rank_row(c: &Candidate) -> Row {
    let r = match c.item.reason() {
        Some(Reason::Layout(InputLang::Thai)) => Some("th".into()),
        Some(Reason::Layout(InputLang::Latin)) => Some("en".into()),
        Some(Reason::Typo) => Some("typo".into()),
        None => None,
    };
    Row { w: c.item.label.clone(), r, c: c.conf.map(|v| v as f32), s: Some(c.src.label().to_string()) }
}

/// Bubble selection: index into the visible items, wrapping. Pure state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection { pub len: usize, pub idx: usize }
impl Selection {
    pub fn new(len: usize) -> Self { Selection { len, idx: 0 } }
    pub fn next(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + 1) % self.len } self.idx }
    pub fn prev(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + self.len - 1) % self.len } self.idx }
}
/// Tab held. Over the bubble Tab alone applies the selected row when released and Tab+1..3 applies that row instead;
/// elsewhere the release types the Tab. Tab+R (held `TAB_CHORD_MIN` first), or Tab held alone for `TAB_HOLD`, opens the
/// ⌃⌥R rows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TabChord {
    /// Tab is up, or went down as a plain Tab (passed through).
    #[default] Up,
    /// Down over the bubble with nothing applied yet: the release applies the selected row.
    Armed,
    /// Down with no bubble: held back in case R follows or it is held for a rewrite; the release sends the Tab.
    Held,
    /// Still down after a digit picked a row or a rewrite started: the release and auto-repeats do nothing.
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
    /// The arrow keys moved into (true) or out of (false) the bubble.
    Focus(bool),
    /// Switch the OS input source to fit the text just inserted (platform: no-op if already that kind).
    SwitchInput(InputLang),
    /// Hand to the rewrite worker. Answers come back through `Engine::on_reply`.
    Rewrite(Job),
    /// Show (true) or take down (false) the input-language preview: focus is in a text field nothing has been typed in
    /// yet, or typing resumed / focus left it.
    Badge(bool),
    /// Tab is being held towards `TAB_HOLD`: show how far (None: take it down).
    Hold(Option<Hold>),
}
/// A held Tab's progress towards opening the ⌃⌥R rows: shown from `from` (past a tap), full at `full`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold { pub from: Instant, pub full: Instant }
impl Hold {
    /// 0 at `from`, 1 at `full`.
    pub fn progress(&self, now: Instant) -> f32 {
        let span = self.full.saturating_duration_since(self.from).as_secs_f32();
        if span <= 0.0 { return 1.0 } (now.saturating_duration_since(self.from).as_secs_f32() / span).min(1.0)
    }
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
    /// The typed word a thumb rates (`Hotkey::Rate`): a finished word offered in Thai. The personal model learns the
    /// English-keys-to-Thai direction only, so nothing else is rated.
    pub fn rated_word(&self) -> Option<&str> { match &self.kind { BubbleKind::Convert { typed, .. } => Some(typed), _ => None } }
}
/// The visible rewrite phases; a bubble holds rows of one phase.
#[derive(Clone, Debug, PartialEq)]
pub enum RewriteRow {
    /// Choosing: available presets, each carrying the captured selection.
    Preset(Preset, String),
    /// Pending until `Reply::Rewritten` for this id.
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
    /// Lower reverse band, offered only after a pause or at a boundary: a known English word of `REV_SUGGEST_LEN`+ keys
    /// whose Thai reading is not a dictionary word (`stab` -> หะฟิ scores 0.76).
    pub rev_suggest: f64,
    /// A word left as Thai is offered together with the next word (in the live bubble, and again after Space)
    /// when that next word is confident and this word's probability with that right context is above `rev_prev`.
    pub rev_prev: f64,
    /// Word completion: `Typing` shows it on a keystroke when the top word's share of the prefix's frequency is at
    /// least `complete_conf`; `Pause` shows it, ungated, once no key came for `complete_idle`.
    pub complete: CompleteMode, pub complete_conf: f64, pub complete_idle: Duration,
    /// The display trigger: per-source confidence floors. Every other source is kept (log-only). No margin yet.
    pub floors: Floors,
    /// How long Tab is held on its own to open the ⌃⌥R rows (`TAB_HOLD_CHOICES_MS`).
    pub tab_hold: Duration,
    /// Learn the words the user finishes and rank completions by them; off neither learns nor uses what was learned.
    pub learn_words: bool,
    /// Offer the word the personal word n-gram predicts from the words around it, beside the dictionary completions.
    /// Off means the model is not even started: nothing is read from or written to its state file.
    pub predict: bool,
    pub accept: AcceptKeys,
    /// Ctrl+1..3 picks a bubble row.
    pub pick_ctrl: bool,
    /// The arrow keys can pick from the bubble, like Windows text suggestions: Up while it shows moves into it,
    /// Left/Right pick, Enter or Up again applies, Down / Esc / typing leave. Until Up the arrows are navigation, and
    /// off they always are.
    pub pick_arrows: bool,
    /// The input-language badge at the caret when the input source changes; read by the platform layer. The badge also
    /// pins while focus sits in a text field nothing has been typed in yet, and comes back after `badge_idle`.
    pub input_badge: bool,
    /// How long a typing pause brings the input-language preview back (`Engine::on_idle`); the platform's timer ticks
    /// every 25 ms. None: never — it shows when focus lands in a text field and stays until typing, but a pause does
    /// not bring it back. Settings offers `Off` and `BADGE_IDLE_CHOICES_MS`.
    pub badge_idle: Option<Duration>,
    /// A pause after a word (after a space in English) offers the word likely to follow it (with completion on).
    pub next_word: bool,
    /// How the keys typed may differ from the word meant, for completion and spelling rows; None reads them as typed.
    pub slips: Option<Slips>,
    /// How the bubble lays out word options; the engine only stores it for Settings and the platform layer.
    pub bubble: BubbleLayout,
}
impl Default for Options { fn default() -> Self { Options { auto_convert: false, switch_input: true, suggest_band: 0.5, typo_suggestions: true, context_words: 8,
live: true, live_show: LIVE_SHOW, live_min_len: 3, live_stable: 1, rev_auto: REV_AUTO, rev_live: REV_LIVE, rev_suggest: REV_SUGGEST, rev_prev: REV_PREV,
    complete: CompleteMode::default(), complete_conf: 0.4, complete_idle: Duration::from_millis(COMPLETE_IDLE_DEFAULT_MS), floors: Floors::default(), tab_hold: TAB_HOLD, learn_words: true, predict: true, accept: AcceptKeys::Tab, pick_ctrl: false, pick_arrows: true, input_badge: true, badge_idle: Some(Duration::from_millis(BADGE_IDLE_DEFAULT_MS)), next_word: true, slips: Some(Slips::default()), bubble: BubbleLayout::default() } } }
/// Pause lengths Settings offers, shortest first (the capture timer ticks every 25 ms). The default is the shortest, so
/// the bubble comes up as soon as typing stops; under the gap between keys in fluent typing it can flash mid-word, and
/// 250 ms or more keeps it away until a real pause.
pub const COMPLETE_IDLE_CHOICES_MS: [u64; 6] = [50, 100, 150, 250, 400, 600];
pub const COMPLETE_IDLE_DEFAULT_MS: u64 = COMPLETE_IDLE_CHOICES_MS[0];
/// When word completions appear in the bubble.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum CompleteMode { Off, #[default] Pause, Typing }
/// What a key or a pause asks of the completion rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ask {
    /// A key in `CompleteMode::Typing`: rows nobody waited for, so a Thai tail needs 3 chars and `complete_conf`.
    Typing,
    /// A key while rows from a pause are up: they follow the word in place instead of going until the next pause.
    Refresh,
    /// The pause in `CompleteMode::Pause`: a Thai tail needs 2 chars, rows are ungated, and the spell checker tops them
    /// up. An English word is asked from its first letter whatever the ask (see `completion_rows`).
    Pause,
}
impl Ask {
    /// The Thai tail the rows read: while typing, nobody waited for the rows, so a tail needs 3 keys; the pause and the
    /// keys that follow it ask from the first key. English is asked from its first letter whatever the ask (see
    /// `completion_rows`), and `complete_conf` keeps Typing-mode rows away unless one word really is the favourite.
    fn min_len(self) -> usize { if self == Ask::Typing { 3 } else { 1 } }
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
/// With the conditions on `Options::rev_suggest`: 0 of 9,433 Thai words, prefixes and joins of the model's top 3,000
/// Thai words, typed on the Thai layout, are offered, against 4,835 of the top 5,000 English words of 4+ letters
/// (2,165 at `REV_LIVE`).
pub const REV_SUGGEST: f64 = 0.6;
pub const REV_SUGGEST_LEN: usize = 4;
/// How long Tab must be down before R makes Tab+R. Fast typing rolls Tab into the next key in less: that R is typed.
pub const TAB_CHORD_MIN: Duration = Duration::from_millis(150);
/// Tab held this long on its own opens the ⌃⌥R rows, like the hotkey: the selection, or the word before the caret.
/// `Options::tab_hold` is one of the choices Settings offers; the default is the quickest. It stays past
/// `TAB_CHORD_MIN`: a tap is shorter, and the indicator needs a moment to fill.
pub const TAB_HOLD_CHOICES_MS: [u64; 6] = [200, 250, 350, 500, 750, 1000];
pub const TAB_HOLD_DEFAULT_MS: u64 = TAB_HOLD_CHOICES_MS[0];
pub const TAB_HOLD: Duration = Duration::from_millis(TAB_HOLD_DEFAULT_MS);
/// The least share of the corpus pairs after a word that a next word needs to be offered.
const NEXT_WORD_SHARE: f64 = 0.2;
/// The user's own pairs (`WordMemory`) offer a next word finished after that word at least twice: two finishes decay
/// to this after about 20,000 words. Theirs come first, best first, and the corpus fills what is left.
const NEXT_WORD_MINE: f64 = 1.5;
/// Next words offered at once. Beside the layout flip of the word just typed they make a fourth row (`ok ` → นา |
/// lets | done | cool), which has no digit shortcut.
const NEXT_WORDS: usize = 3;
/// Retroactive previous-word flip threshold (only when the next word auto-flips).
pub const REV_PREV: f64 = 0.8;
/// English words offered back from the Thai layout whatever the model says, because their Thai reading is a real word
/// (`ok` -> นา). Words on the don't-translate list are offered the same way.
pub const REV_ALWAYS: [&str; 2] = ["ok", "it"];
/// Pauses Settings offers for the input-language preview to come back after: long enough to read as a real stop, and
/// past the pause that brings up word completions. The default is the second: two seconds.
pub const BADGE_IDLE_CHOICES_MS: [u64; 5] = [1000, 2000, 3000, 5000, 10000];
pub const BADGE_IDLE_DEFAULT_MS: u64 = BADGE_IDLE_CHOICES_MS[1];

pub struct Engine {
    /// Reverse-direction model; None disables it (the engine then ignores keys while Thai is active).
    pub rev: Option<Reverse>,
    /// The OS input source is Thai: words are scored in the reverse direction.
    thai_active: bool,
    pub det: Detector, pub personal: PersonalModel, pub corrector: Option<Corrector>, pub opts: Options,
    pub paused: bool,
    /// The app in front is on the user's don't-use-typeThai list: every key passes through untouched, nothing is shown
    /// and nothing is learned, as if the field were a password field.
    off: bool,
    word: String,
    /// The last key was a Tab that inserted itself: a second Tab runs Guess.
    tab_pending: bool,
    tab: TabChord,
    /// When Tab went down into `Armed` or `Held`.
    tab_at: Option<Instant>,
    /// The hold indicator is up (`Action::Hold`).
    hold_up: bool,
    /// The bubble was opened by ⌃⌥R and offers something to apply: Enter applies the highlighted row, like Tab.
    fixing: bool,
    line: Vec<Committed>,
    /// Index of the first word of the current line in `line` (after Enter the last word is kept so Tab still applies).
    line_start: usize,
    /// Items currently on the bubble (accept by index).
    bubble: Vec<BubbleItem>,
    /// The rows on the bubble in display order, with the scores and sources the trigger saw. Same length and order as
    /// `bubble`; apply paths read `bubble`, and nothing but `show_candidates` writes either.
    shown: Vec<Candidate>,
    sel: Selection,
    /// The arrow keys are in the bubble (Up moved into it). Only while it shows.
    in_bubble: bool,
    live_streak: usize,
    /// The visible bubble is a live (mid-word) suggestion.
    live: bool,
    /// Words in the offered phrase (current buffer or last committed word, plus earlier words); 0 = nothing pending.
    /// Lets Backspace keep offering what is left of a phrase.
    phrase: usize,
    pub events_since_save: usize,
    /// English key tokens the user does not want translated (`ok` stays as typed, not `นา`).
    pub never: BTreeSet<String>,
    /// English key tokens whose conversion the user rated down (`Hotkey::Rate`): still offered, after every other
    /// option (`ok ` → lets | done | cool | นา).
    pub offer_last: BTreeSet<String>,
    /// `never` / `offer_last` changed outside Settings: the host saves prefs.json.
    pub prefs_dirty: bool,
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
    /// Focus is in a text field (the platform's gate): the input-language preview shows there and nowhere else.
    focus: bool,
    /// The preview is up (`Action::Badge(true)`); a key, focus leaving, the bubble or a hold takes it down.
    badge_up: bool,
    /// The last keystroke (`on_keystroke`): the preview comes back `Options::badge_idle` after it (never, when None).
    typed_at: Option<Instant>,
    /// Esc / Dismiss: no completions until the next word boundary.
    comp_muted: bool,
    /// The row the spell checker put first at the last pause (what it inserts): a key keeps it first while it still fits.
    spell_lead: Option<String>,
    /// Contextual word prediction: the personal word n-gram on its own thread (`predict.rs`). None when the setting is
    /// off or the state could not be opened; the rows it offers are added to the completion rows.
    pub predict: Option<predict::Predict>,
    /// The query the predictor was last asked (`key`, the word being typed, and whether the buffer reads as Thai), the
    /// answer it gave (`key`, its confidence, the whole word), and the word last shown with its confidence, which stays
    /// while the word is typed through: a ghost must not move under the user's fingers.
    pred_ask: Option<(u64, String, bool)>, pred_answer: Option<(u64, f64, String)>, pred_shown: Option<(String, bool, String, f64)>,
    /// The next-word query asked for this line state (its key), and its answer: the key, confidence, word.
    pred_next_ask: Option<u64>, pred_next_answer: Option<(u64, f64, String)>,
    /// The finished line has been given to the predictor (a line is learned once).
    line_learned: bool,
    /// What one Backspace removes in the focused app; the platform sets it when another app gets the keys.
    pub delete_unit: DeleteUnit,
    /// The bubble offers this run made and the row applied to each (`rank.rs`): what `--rank-report` measures and what
    /// a ranker would be trained on. Nothing reads it while you type, and **Learn from my typing** off stops it.
    pub rank: RankLog,
    /// How the row being applied was picked; `Hotkey::Accept` alone cannot tell a click from a number. Taken (back to
    /// `How::Other`) by every apply.
    pick: How,
}

impl Engine {
    pub fn new(det: Detector, personal: PersonalModel, corrector: Option<Corrector>, opts: Options) -> Self {
        Engine { rev: None, thai_active: false, det, personal, corrector, opts, paused: false, off: false, word: String::new(), tab_pending: false, tab: TabChord::Up, tab_at: None, hold_up: false, line: vec![], line_start: 0, bubble: vec![], shown: vec![], sel: Selection::default(), in_bubble: false, live_streak: 0, live: false, phrase: 0, events_since_save: 0, never: BTreeSet::new(), offer_last: BTreeSet::new(), prefs_dirty: false, speller: None, rewrite: rewrite::Config::default(), check_updates: true, capturing: None, last_req: ReqId::default(), thai_comp: None, en_comp: None, thai_bigrams: None, en_bigrams: None, words: Arc::default(), last_key: None, focus: false, badge_up: false, typed_at: None, comp_muted: false, spell_lead: None, predict: None, pred_ask: None, pred_answer: None, pred_shown: None, pred_next_ask: None, pred_next_answer: None, line_learned: false, delete_unit: DeleteUnit::default(), rank: RankLog::default(), pick: How::default(), fixing: false }
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
    /// The app in front was added to / removed from the don't-use-typeThai list (`App::set_front_app`). Turning it off
    /// takes down whatever is on screen (a bubble, the preview) and starts a fresh, empty state; nothing is offered or
    /// learned again until it is turned back on. Returns the actions that take that down.
    pub fn set_app_off(&mut self, off: bool) -> Vec<Action> {
        if self.off == off { return vec![] }
        self.off = off;
        let mut a = self.reset();
        a.extend(self.hide_badge());
        a
    }
    pub fn app_off(&self) -> bool { self.off }
    /// The platform's gate looked at focus: it is (`text`) or is not in a text field. Focus landing in one shows the
    /// input-language preview and keeps it up: nothing has been typed there yet, so the language about to be typed is
    /// what the user needs to see. Focus leaving a text field takes it down.
    pub fn on_focus(&mut self, text: bool) -> Vec<Action> {
        self.focus = text; self.typed_at = None;
        if self.off { return self.hide_badge() }
        if text { self.badge_show() } else { self.hide_badge() }
    }
    /// The platform's gate looked at focus after a keystroke that may have moved it (Tab, ⌘K), so this is not a focus
    /// the user made: the verdict decides whether the preview may be up at all, and one that is up goes when it is no
    /// longer a text field. Typing just happened, so nothing is shown on the spot — the pause after it shows the
    /// preview, on a caret that has settled (the keystroke may have scrolled or laid the field out under it).
    pub fn set_field(&mut self, text: bool) -> Vec<Action> {
        self.focus = text;
        if self.off || text { vec![] } else { self.hide_badge() }
    }
    /// A keystroke the user made, however the platform then routes it (a converted key, a bypassed one, one of ours):
    /// typing takes the preview down, and it comes back `Options::badge_idle` later if focus is still in a text field
    /// (or not at all, when that is None).
    pub fn on_keystroke(&mut self, now: Instant) -> Vec<Action> {
        if self.off || !self.focus { return vec![] }
        self.typed_at = Some(now);
        self.hide_badge()
    }
    /// The preview is up (`Action::Badge(true)`): the platform layer keeps it on the caret while it is.
    pub fn badge_shown(&self) -> bool { self.badge_up }
    /// Show the input-language preview: one action per time it goes up, and never where the bubble has the window or
    /// typeThai is off in the app in front.
    fn badge_show(&mut self) -> Vec<Action> {
        if self.badge_up || self.paused || self.off || !self.opts.input_badge || self.bubble_visible() { return vec![] }
        self.badge_up = true; vec![Action::Badge(true)]
    }
    /// Take the preview down, if it was up: for typing and for focus leaving (above), and for a pause (the pause
    /// branch of `on_key`).
    fn hide_badge(&mut self) -> Vec<Action> { std::mem::take(&mut self.badge_up).then_some(Action::Badge(false)).into_iter().collect() }
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
        self.rev_score(last).map_or((0.0, false), |(p, d)| (p, d.convertible))
    }
    /// The English text a reverse conversion should insert: the typed keys, or their best spelling when they
    /// are a typo of a known English word.
    fn rev_target(&self, typed: &str) -> String {
        self.en_items("", typed, None).first().map(|i| i.label.clone()).unwrap_or_else(|| typed.to_string())
    }
    fn rev_score(&self, last: &str) -> Option<(f64, RevDecision)> {
        let r = self.rev.as_ref()?;
        let tail: Vec<&str> = self.line[self.line_start.min(self.line.len())..].iter().rev().take_while(|c| c.rev).take(self.opts.context_words).map(|c| c.typed.as_str()).collect::<Vec<_>>().into_iter().rev().chain([last]).collect();
        let mut ds = r.decide_line(&self.det, &tail); let p = *r.line_probs(&ds).last()?;
        Some((p, ds.pop()?))
    }
    /// The `rev_suggest` band (see there), for a pause or a boundary.
    fn rev_suggestable(&self, typed: &str) -> bool {
        if self.blocked(typed) || typed.chars().count() < REV_SUGGEST_LEN || !typed.chars().all(|c| c.is_ascii_alphabetic()) { return false }
        // A known English word, a typo of one the spell checker flags, or a typo of one the completer knows.
        let known = self.en_comp.as_ref().is_some_and(|c| c.has(typed));
        let near = self.en_comp.as_ref().is_some_and(|c| c.has_near(typed, 1.0));
        let spell = !self.en_items("", typed, None).is_empty();
        if !known && !near && !spell { return false }
        self.rev_score(typed).is_some_and(|(p, d)| d.convertible && !d.th_word && p > self.opts.rev_suggest)
    }
    pub fn bubble_visible(&self) -> bool { !self.bubble.is_empty() }
    pub fn selected(&self) -> usize { self.sel.idx }
    /// Keys the platform must route to the engine even when it would otherwise bypass it (Thai layout active, secure
    /// field check): the bubble controls while the bubble is up, the keys of a held Tab, and Space when it applies a live
    /// row (a layout change clears the buffer first, so a stale row never pulls Space in).
    pub fn is_bubble_key(&self, k: &Key) -> bool {
        if self.off { return false }
        (self.bubble_visible() && matches!(k, Key::Tab | Key::SelectPrev | Key::SelectNext | Key::Esc))
            || (self.tab != TabChord::Up && matches!(k, Key::Tab | Key::TabUp | Key::Char('1'..='3' | 'r' | 'R')))
            || (*k == Key::Boundary(Boundary::Space) && self.space_accepts())
            || (matches!(k, Key::PickRow(_)) && self.picks_rows())
            || (*k == Key::Boundary(Boundary::Enter) && self.fixing && self.bubble_visible())
            || (self.bubble_visible() && self.is_arrow_key(k))
    }
    /// Up while the bubble can be entered; every arrow and Enter once in it.
    fn is_arrow_key(&self, k: &Key) -> bool {
        (self.opts.pick_arrows && *k == Key::Arrow(Arrow::Up)) || (self.in_bubble && matches!(k, Key::Arrow(_) | Key::Boundary(Boundary::Enter)))
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
    /// Tab is held, and may still open the ⌃⌥R rows.
    fn tab_down(&self) -> bool { matches!(self.tab, TabChord::Armed | TabChord::Held) }
    /// Take the hold indicator down, if it is up.
    fn hold_off(&mut self) -> Option<Action> { std::mem::take(&mut self.hold_up).then_some(Action::Hold(None)) }
    /// Focus change, mouse click, secure field, layout change, any non-bubble key during a rewrite: forget everything.
    pub fn reset(&mut self) -> Vec<Action> {
        let hold = self.hold_off();
        let mut a = self.reset_inner(); a.splice(0..0, hold); a
    }
    fn reset_inner(&mut self) -> Vec<Action> {
        if !self.off { self.settle(); self.learn_line(); }
        self.rank.close();
        self.line_learned = false;
        self.word.clear(); self.tab_pending = false; self.tab = TabChord::Up; self.in_bubble = false; self.capturing = None; self.line.clear(); self.line_start = 0; self.live_streak = 0; self.live = false;
        self.last_key = None; self.comp_muted = false;
        self.pred_shown = None;
        self.shown.clear();
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
    /// `before`: the text in front of `typed` on the line.
    pub(crate) fn en_items(&self, before: &str, typed: &str, prev: Option<&str>) -> Vec<BubbleItem> {
        let Some(sp) = &self.speller else { return vec![] };
        if !self.opts.typo_suggestions { return vec![] }
        let core = typed.trim_matches(|c: char| !c.is_ascii_alphabetic());
        if core.len() < 2 || !core.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') { return vec![] }
        let at = typed.find(core).unwrap_or(0);
        let (pre, post) = (&typed[..at], &typed[at + core.len()..]);
        let Some(guesses) = sp.guesses(&format!("{before}{pre}"), core) else { return vec![] };
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
    /// Display-only: show these items with the source their kind implies and no score. This row can never be gated;
    /// a producer with a score returns candidates and calls `show_candidates` instead.
    fn show(&mut self, items: Vec<BubbleItem>, acts: &mut Vec<Action>) {
        let rows = items.into_iter().map(|item| { let src = src_of_kind(&item.kind); Candidate::bare(item, src) }).collect();
        self.show_candidates(rows, acts)
    }
    /// The rows the bubble would show, sorted as drawn, gated as configured, logged as shown, then mapped back to the
    /// apply instructions. The one choke point: every ShowBubble / HideBubble goes through here. The gate runs after
    /// the display sort and before `self.bubble` and the rank offer, so `Pick.i`, the digits, Tab and clicks address
    /// the shown vector.
    fn show_candidates(&mut self, mut items: Vec<Candidate>, acts: &mut Vec<Action>) {
        self.fixing = false;
        // new rows start with the arrow keys outside (the UI resets on Show / Hide)
        self.in_bubble = false;
        // grouped as drawn, so a row's index is its place on screen for Tab, the digits and clicks; a conversion rated
        // down goes after everything
        items.sort_by_key(|c| if c.item.rated_word().is_some_and(|w| self.rated_down(w)) { 3 } else { match c.item.reason() { Some(Reason::Layout(_)) => 0, Some(Reason::Typo) => 1, None => 2 } });
        // the offer is about the rows before the gate (a suppressed set still names the word it was for)
        let ctx = self.opts.learn_words.then(|| { let all: Vec<BubbleItem> = items.iter().map(|c| c.item.clone()).collect(); self.offer_ctx(&all) });
        let offered = items.len();
        let kept = crate::candidate::gate(items, &self.opts.floors);
        let rows: Vec<BubbleItem> = kept.iter().map(|c| c.item.clone()).collect();
        // what this bubble offered, for the rank log: the rows of one word are one offer, and the index a row is
        // applied by is its place here
        if let Some(ctx) = ctx {
            if kept.is_empty() && offered > 0 { self.rank.suppressed(ctx, offered.min(u8::MAX as usize) as u8) }
            else { self.rank.offer(ctx, kept.iter().map(rank_row).collect()) }
        }
        self.live = matches!(rows.first(), Some(BubbleItem { kind: BubbleKind::Live { .. }, .. }));
        let act = if rows.is_empty() {
            self.phrase = 0;
            self.shown.clear();
            if self.bubble.is_empty() {
                // an earlier ShowBubble queued in this key must not outlive the empty decision
                acts.retain(|a| !matches!(a, Action::ShowBubble(_) | Action::HideBubble));
                return
            }
            self.bubble.clear(); Action::HideBubble
        } else {
            self.sel = Selection::new(rows.len()); self.bubble = rows.clone(); self.shown = kept; self.badge_up = false; Action::ShowBubble(rows)
        };
        // one bubble update per key (the live row and the completions under it are worked out one after the other)
        acts.retain(|a| !matches!(a, Action::ShowBubble(_) | Action::HideBubble));
        acts.push(act);
    }
    /// The conversion of `typed` was rated down (`offer_last`).
    fn rated_down(&self, typed: &str) -> bool { normalize_word(typed).is_some_and(|k| self.offer_last.contains(&k)) }
    fn feedback(&mut self, typed: &str, kind: Feedback) {
        let ds = self.decisions(typed); let i = ds.len() - 1; self.personal.feedback(&ds, i, kind); self.events_since_save += 1;
    }

    pub fn on_key(&mut self, k: Key) -> Outcome {
        if self.off { return Outcome::pass() }
        let mut o = self.on_key_inner(k);
        // the held Tab let go (released, rolled into a key, Tab+R / Tab+digit): its indicator goes first
        if !self.tab_down() { o.actions.splice(0..0, self.hold_off()); }
        // after inserting text, switch the OS input source to match it (Thai after a conversion, Latin after an undo)
        if self.opts.switch_input {
            if let Some(Action::Replace { insert, .. }) = o.actions.iter().rev().find(|a| matches!(a, Action::Replace { .. })) {
                if let Some(l) = layout_of(insert) { o.actions.push(Action::SwitchInput(l)); }
            }
        }
        // the line may have changed: ask the predictor for the word that follows it now, so its answer is in hand by the
        // pause that shows the next-word rows (the model answers from its own thread, a tick later)
        self.ask_next_word();
        o
    }
    fn on_key_inner(&mut self, k: Key) -> Outcome {
        if let Key::Hotkey(Hotkey::TogglePause) = k { self.paused = !self.paused; let mut a = self.reset(); a.extend(self.hide_badge()); return Outcome { swallow: true, actions: a } }
        if self.paused { return Outcome::pass() }
        if self.in_bubble && !self.is_bubble_key(&k) && !matches!(k, Key::Hotkey(_) | Key::TabUp) {
            // any other key leaves the bubble and is handled as usual
            self.in_bubble = false; let mut o = self.on_key_inner(k); o.actions.insert(0, Action::Focus(false)); return o
        }
        if let Some(o) = self.tab_chord(&k) { return o }
        let k = match k { Key::PickRow(i) if self.picks_rows() => return self.pick_with(How::Digit, i), Key::PickRow(_) => Key::Nav, k => k };
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
            Key::Tab => self.plain_tab(),
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
            Key::Boundary(Boundary::Space) if self.space_accepts() => { self.pick = How::Space; self.accept_rolled(k) }
            Key::Arrow(a) => self.on_arrow(a),
            // Enter in the bubble applies the selection, like Tab
            Key::Boundary(Boundary::Enter) if (self.fixing || self.in_bubble) && self.bubble_visible() => {
                let mut o = self.on_hotkey(Hotkey::Accept(self.sel.idx)); o.swallow = true; o
            }
            Key::Boundary(Boundary::Enter) => self.on_enter(),
            Key::Boundary(b) => self.on_boundary(b),
            Key::Hotkey(h) => self.on_hotkey(h),
        }
    }
    /// Arrow-key picking (see `Options::pick_arrows`); outside the bubble an arrow is navigation.
    fn on_arrow(&mut self, a: Arrow) -> Outcome {
        if !self.bubble_visible() || !self.is_arrow_key(&Key::Arrow(a)) { return self.on_key_inner(Key::Nav) }
        let acts = match (self.in_bubble, a) {
            (false, _) => { self.in_bubble = true; vec![Action::Focus(true)] }
            (true, Arrow::Up) => return self.pick_with(How::Arrow, self.sel.idx),
            (true, Arrow::Left) => vec![Action::Select(self.sel.prev())],
            (true, Arrow::Right) => vec![Action::Select(self.sel.next())],
            (true, Arrow::Down) => { self.in_bubble = false; vec![Action::Focus(false)] }
        };
        Outcome { swallow: true, actions: acts }
    }
    /// The keys of a held Tab (see `TabChord`); None for keys handled as usual.
    fn tab_chord(&mut self, k: &Key) -> Option<Outcome> {
        let swallowed = Outcome { swallow: true, actions: vec![] };
        Some(match (self.tab, k) {
            (TabChord::Up, Key::Tab) if self.bubble_visible() => { self.tab = TabChord::Armed; self.tab_at = Some(Instant::now()); self.tab_pending = false; swallowed }
            (TabChord::Up, Key::Tab) => { self.tab = TabChord::Held; self.tab_at = Some(Instant::now()); swallowed }
            (TabChord::Up, _) => return None,
            (_, Key::Tab) => swallowed,
            (TabChord::Armed | TabChord::Held, Key::Char('r' | 'R')) if self.tab_at.is_some_and(|t| t.elapsed() >= TAB_CHORD_MIN) => {
                // still Spent afterwards (starting a rewrite lets Tab go): repeats and the release type nothing
                self.tab = TabChord::Spent; let mut o = self.on_key_inner(Key::Hotkey(Hotkey::Rewrite)); self.tab = TabChord::Spent; o.swallow = true; o
            }
            (TabChord::Held, Key::TabUp) => { self.tab = TabChord::Up; self.release_tab() }
            // typed on before Tab came up: the Tab goes first, then the key
            (TabChord::Held, Key::Char(_) | Key::Boundary(_)) => { self.tab = TabChord::Up; self.release_tab_before(k.clone()) }
            (TabChord::Held, _) => { self.tab = TabChord::Up; let mut o = self.release_tab(); let p = self.on_key_inner(k.clone()); o.actions.extend(p.actions); o.swallow = p.swallow; o }
            (TabChord::Armed, Key::TabUp) => { self.tab = TabChord::Up; self.pick_with(How::Tab, self.sel.idx) }
            (TabChord::Spent, Key::TabUp) => { self.tab = TabChord::Up; swallowed }
            // a digit is never typed while Tab is down; past the last row it does nothing
            (_, Key::Char(d @ '1'..='3')) => { self.tab = TabChord::Spent; self.pick_with(How::Digit, *d as usize - '1' as usize) }
            // bubble controls work as usual: the release applies whatever is then selected, if the bubble is still up
            (_, Key::SelectPrev | Key::SelectNext | Key::Esc | Key::Hotkey(_)) => return None,
            (TabChord::Armed, Key::Char(_) | Key::Boundary(_)) => { self.tab = TabChord::Up; self.pick = How::Tab; self.accept_rolled(k.clone()) }
            // caret moves and deletions drop the held Tab: the bubble no longer matches the text
            _ => { self.tab = TabChord::Up; return None }
        })
    }
    /// Tab as it always was: a boundary for the word, and a second Tab in a row runs Guess.
    fn plain_tab(&mut self) -> Outcome {
        if let Some(item) = self.tab_pending.then(|| self.guess()).flatten() {
            // second Tab in a row: show the proposal (a third Tab applies it); with nothing to propose it is a tab
            self.tab_pending = false;
            let mut a = vec![]; self.show(vec![item], &mut a);
            return Outcome { swallow: true, actions: a }
        }
        let o = self.on_boundary(Boundary::Tab);
        self.tab_pending = true;
        o
    }
    /// A held Tab came up without R: handle it as a plain Tab, and type it if it would have passed through.
    fn release_tab(&mut self) -> Outcome {
        let mut o = self.plain_tab();
        if !o.swallow { o.actions.push(Action::Replace { delete: 0, insert: String::new(), then: Some(Boundary::Tab) }); o.swallow = true; }
        o
    }
    /// `k` came while Tab was held: type the Tab, then `k` (injected too, so it cannot land before the Tab).
    fn release_tab_before(&mut self, k: Key) -> Outcome {
        let mut o = self.release_tab();
        let p = self.on_key_inner(k.clone());
        o.actions.extend(p.actions);
        if !p.swallow {
            match k { Key::Char(c) => o.actions.push(Action::Replace { delete: 0, insert: self.screen_of(&c.to_string()), then: None }),
                Key::Boundary(b) => o.actions.push(Action::Replace { delete: 0, insert: String::new(), then: Some(b) }), _ => unreachable!() }
        }
        o
    }
    fn accept_held(&mut self, i: usize) -> Outcome { let mut o = self.on_hotkey(Hotkey::Accept(i)); o.swallow = true; o }
    /// Apply row `i`, remembering which key did it (the log's `rank::How`: `Hotkey::Accept` cannot tell a click from a
    /// number, and a reflex Tab from a deliberate pick).
    fn pick_with(&mut self, how: How, i: usize) -> Outcome { self.pick = how; self.accept_held(i) }
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
    /// Keys typed on the Thai layout that are always offered as the English they spell: `REV_ALWAYS`, and the
    /// don't-translate list (a word kept as English one way is the English meant the other way).
    fn rev_wanted(&self, typed: &str) -> bool {
        let k = typed.to_lowercase();
        REV_ALWAYS.contains(&k.as_str()) || self.blocked(&k)
    }
    pub fn apply_settings(&mut self, cmd: SettingsCmd) -> bool {
        match cmd {
            SettingsCmd::AddNever(w) => self.insert_never(&w),
            SettingsCmd::RemoveNever(w) => self.remove_never(&w),
            SettingsCmd::Forget(w) => self.forget_learned(&w),
            SettingsCmd::ResetLearning => { self.personal.reset(); self.rank.reset(); self.events_since_save += 1; true }
            SettingsCmd::SetCompletion(m) => { let changed = self.opts.complete != m; self.opts.complete = m; changed }
            SettingsCmd::SetCompleteIdle(d) => { let changed = self.opts.complete_idle != d; self.opts.complete_idle = d; changed }
            SettingsCmd::SetTabHold(d) => { let changed = self.opts.tab_hold != d; self.opts.tab_hold = d; changed }
            SettingsCmd::SetLearnWords(on) => { let changed = self.opts.learn_words != on; self.opts.learn_words = on; changed }
            SettingsCmd::SetAccept(a) => { let changed = self.opts.accept != a; self.opts.accept = a; changed }
            SettingsCmd::SetPickCtrl(on) => { let changed = self.opts.pick_ctrl != on; self.opts.pick_ctrl = on; changed }
            SettingsCmd::SetPickArrows(on) => { let changed = self.opts.pick_arrows != on; self.opts.pick_arrows = on; changed }
            SettingsCmd::SetInputBadge(on) => { let changed = self.opts.input_badge != on; self.opts.input_badge = on; changed }
            SettingsCmd::SetBadgeIdle(d) => { let changed = self.opts.badge_idle != d; self.opts.badge_idle = d; changed }
            SettingsCmd::SetNextWord(on) => { let changed = self.opts.next_word != on; self.opts.next_word = on; changed }
            SettingsCmd::SetBubbleLayout(l) => { let changed = self.opts.bubble != l; self.opts.bubble = l; changed }
            SettingsCmd::ForgetWords => { self.words = Arc::default(); self.rank.reset(); if let Some(p) = &self.predict { p.forget() } self.pred_ask = None; self.pred_answer = None; self.pred_shown = None; self.events_since_save += 1; true }
            SettingsCmd::SetPredict(on) => { let changed = self.opts.predict != on; self.opts.predict = on; self.pred_ask = None; self.pred_answer = None; self.pred_shown = None; self.pred_next_ask = None; self.pred_next_answer = None; changed }
            SettingsCmd::SetRewrite(c) => { let changed = self.rewrite != c; self.rewrite = c; changed }
            SettingsCmd::SetCheckUpdates(on) => { let changed = self.check_updates != on; self.check_updates = on; changed }
            // handled by `App` (it owns the off-app list and the app in front), not the engine's own settings
            SettingsCmd::AppOff { .. } | SettingsCmd::AppOn(_) => false,
        }
    }
    pub fn settings_snapshot(&self) -> SettingsSnap {
        SettingsSnap { never: self.never.iter().cloned().collect(), learned: self.personal.learned(), completion: self.opts.complete, complete_idle: self.opts.complete_idle, tab_hold: self.opts.tab_hold, learn_words: self.opts.learn_words, predict: self.opts.predict, accept: self.opts.accept, pick_ctrl: self.opts.pick_ctrl, pick_arrows: self.opts.pick_arrows, input_badge: self.opts.input_badge, badge_idle: self.opts.badge_idle, next_word: self.opts.next_word, bubble: self.opts.bubble, words_learned: self.words.len(), rewrite: self.rewrite.clone(), check_updates: self.check_updates, off_apps: vec![] }
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
        let text = if self.thai_active { self.rev_target(&typed) } else { en_to_th(&typed) };
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
    fn reoffer(&mut self, was_shown: bool, phrase: usize, up: &[Candidate], acts: &mut Vec<Action>) {
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
    /// A Tab held `Options::tab_hold` on its own opens the ⌃⌥R rows; held past a tap (`TAB_CHORD_MIN`), its indicator shows.
    pub fn on_idle(&mut self, now: Instant) -> Vec<Action> {
        let mut acts = vec![];
        if self.paused || self.off { return acts }
        // the model's answer lands a tick or two after the state that asked for it: the rows that state shows take it
        // in, without the pause's spell checker call
        let answered = self.poll_prediction();
        if answered && !self.word.is_empty() && self.opts.complete != CompleteMode::Off
            && (self.opts.complete == CompleteMode::Typing || self.last_key.is_none())
        {
            let up = self.completions_up();
            if self.opts.complete == CompleteMode::Typing || !up.is_empty() { self.update_completions(false, &up, &mut acts) }
            else { self.update_completions(true, &[], &mut acts) }
        }
        // typing stopped while focus sits in a text field: the input-language preview comes back
        if self.focus && self.opts.badge_idle.is_some_and(|idle| self.typed_at.is_some_and(|t| now.saturating_duration_since(t) >= idle)) { acts.extend(self.badge_show()); }
        if let Some(t) = self.tab_at.filter(|_| self.tab_down()) {
            let held = now.saturating_duration_since(t);
            if held >= self.opts.tab_hold {
                // starting it lets Tab go: keep it Spent so the repeats and the release type nothing
                let mut a: Vec<Action> = self.hold_off().into_iter().collect();
                // Spent first, as for Tab+R: a hotkey while Tab is Held would count as its release and type the Tab
                self.tab = TabChord::Spent; let o = self.on_key_inner(Key::Hotkey(Hotkey::Rewrite)); self.tab = TabChord::Spent; a.extend(o.actions); return a
            }
            if held >= TAB_CHORD_MIN && !self.hold_up { self.hold_up = true; acts.push(Action::Hold(Some(Hold { from: t + TAB_CHORD_MIN, full: t + self.opts.tab_hold }))) }
        }
        // the model answers from its own thread, so the word it adds to the next-word rows can arrive after the pause
        // tick that offered the pairs: take it on a later tick, once (the row it shows keeps this quiet afterwards)
        if self.last_key.is_none() && self.word.is_empty() && self.next_word_room()
            && self.pred_next_answer.as_ref().is_some_and(|(key, _, _)| Some(*key) == self.pred_next_ask) {
            self.offer_next_words(&mut acts);
        }
        if self.last_key.is_none_or(|t| now.saturating_duration_since(t) < self.opts.complete_idle) { return acts }
        self.last_key = None;
        // English on the Thai layout: its reverse row, which the words just typed may also get. A word whose keys are
        // always offered back as the English (`ok` -> นา, `REV_ALWAYS`, or a don't-translate word) gets it after the
        // pause too, not only at the word boundary.
        if self.opts.live && self.thai_active && !self.live_visible() && (self.rev_wanted(&self.word) || self.rev_suggestable(&self.word)) { let prev = self.live_prev(); self.show_live(prev, &mut acts); }
        if self.opts.complete == CompleteMode::Pause { self.update_completions(true, &[], &mut acts); }
        self.offer_next_words(&mut acts);
        acts
    }
    /// The next-word rows: the user's own pairs and the built-in pairs after the word just finished, and the personal
    /// model's word where it has one for this line. The pairs only show once the pause has elapsed (`on_idle` calls
    /// this below that guard); an answer that lands later brings the rows up on its own tick. They join a layout flip
    /// offered for that word (`next_word_room`), after it.
    fn offer_next_words(&mut self, acts: &mut Vec<Action>) {
        if self.opts.complete == CompleteMode::Off || !self.opts.next_word { return }
        self.ask_next_word();
        self.poll_prediction();
        if self.word.is_empty() {
            if self.next_word_room() {
                let next = self.next_word_rows();
                // nothing to offer here (the caret is not after a word): the answer is spent, and the next line asks
                // again. Without this the late-answer tick would rebuild the rows every tick until a key came.
                if next.is_empty() { self.pred_next_answer = None }
                let rows: Vec<Candidate> = self.shown.iter().cloned().chain(next).collect();
                if Self::items_of(&rows) != self.bubble { self.show_candidates(rows, acts) }
            }
        }
        // Thai is typed without spaces: a run that ends on a dictionary word offers the next one, ahead of the
        // completions of that word
        else if self.thai_active && self.bubble.iter().all(|b| matches!(b.kind, BubbleKind::Complete { .. })) {
            let rows = self.thai_next_rows();
            if !rows.is_empty() {
                let mut items = rows;
                for c in self.completions_up() { if items.len() < 3 && !items.iter().any(|r| r.item == c.item) { items.push(c) } }
                if Self::items_of(&items) != self.bubble { self.show_candidates(items, acts) }
            }
        }
    }
    /// The items of `rows`, in order: what "did the bubble change?" is asked of.
    fn items_of(rows: &[Candidate]) -> Vec<BubbleItem> { rows.iter().map(|c| c.item.clone()).collect() }
    /// The words to offer after `prev`: the pairs (`next_words`), and the personal model's word in the last slot when
    /// it has one for this line and clears the prediction floor (the pairs know the language and this user's habits,
    /// the model reads the line). Admission is decided before the truncation, so a dropped model word does not cost a
    /// pair its slot.
    fn next_words_here(&self, prev: &str, lang: InputLang) -> Vec<(String, Src, Option<f64>)> {
        let mut words = self.next_words(prev, lang);
        if let Some((w, conf)) = self.predicted_next() {
            if !words.iter().any(|(x, ..)| *x == w) && self.opts.floors.admits(Src::Prediction, Some(conf)) {
                words.truncate(NEXT_WORDS - 1);
                words.push((w, Src::Prediction, Some(conf)));
            }
        }
        words
    }
    /// The words likely to follow `prev` in `lang`: first the ones this user finished after it at least twice
    /// (`NEXT_WORD_MINE`, from `words` while `learn_words` is on), best first; then the corpus pairs after it, each at
    /// least `NEXT_WORD_SHARE` of them (`thank` → you; nothing after `you`, where no word stands out). Each carries its
    /// source: the user's pairs have no probability, the corpus pairs carry their share.
    fn next_words(&self, prev: &str, lang: InputLang) -> Vec<(String, Src, Option<f64>)> {
        let cx = self.context(Some(prev), lang);
        if cx.prev.is_none() { return vec![] }
        let mut words: Vec<(String, Src, Option<f64>)> = vec![];
        if let Some(memory) = cx.memory {
            let mut mine: Vec<(&str, f64)> = memory.followers(prev).filter(|&(w, n)| n >= NEXT_WORD_MINE && lang_of(w) == lang && !self.blocked(w)).collect();
            mine.sort_by(|a, b| b.1.total_cmp(&a.1));
            words.extend(mine.into_iter().take(NEXT_WORDS).map(|(w, _)| (w.to_string(), Src::NextWord, None)));
        }
        let comp = match lang { InputLang::Thai => &self.thai_comp, InputLang::Latin => &self.en_comp };
        let (Some(comp), Some(b)) = (comp, cx.bigrams) else { return words };
        // ponytail: one word of context; a longer one needs trigrams or a language model (docs/research)
        let mass = b.mass(prev);
        for (w, c) in b.followers(prev).take(NEXT_WORDS).take_while(|(_, c)| *c >= NEXT_WORD_SHARE * mass) {
            if words.len() == NEXT_WORDS { break }
            if comp.knows(w) && !self.blocked(w) && !words.iter().any(|(x, ..)| x == w) { words.push((w.to_string(), Src::NextWord, (mass > 0.0).then(|| c / mass))) }
        }
        words
    }
    /// The next-word rows may come up: the bubble is down, or holds only the layout flip of the word just typed
    /// (`ok ` → นา), which they join after (Tab still applies the flip).
    fn next_word_room(&self) -> bool { self.bubble.iter().all(|b| matches!(b.kind, BubbleKind::Convert { .. })) }
    /// The next words when the caret sits after a word and a space, in the input language. Applied like a completion
    /// of nothing. None at the start of a line.
    fn next_word_rows(&self) -> Vec<Candidate> {
        if self.comp_muted || !self.line.last().is_some_and(|c| c.after().ends_with(' ')) { return vec![] }
        let Some(prev) = self.word_before(self.line.len()).map(|w| w.to_lowercase()) else { return vec![] };
        let lang = if self.thai_active { InputLang::Thai } else { InputLang::Latin };
        self.next_words_here(&prev, lang).into_iter()
            .map(|(w, src, conf)| Candidate { item: BubbleItem { label: w.clone(), kind: BubbleKind::Complete { typed: String::new(), insert: w } }, conf, src })
            .collect()
    }
    /// The next words after the Thai run being typed, when its last word is a dictionary word (`แซ่บ` → มาก). Applied
    /// like a completion of the run, labelled with the next word only.
    fn thai_next_rows(&self) -> Vec<Candidate> {
        if self.comp_muted || self.blocked(&self.word) { return vec![] }
        let run = en_to_th(&self.word);
        let d = &self.det.th_dict;
        let Some(last) = d.segment(&run).last().map(|w| w.to_string()).filter(|w| d.has(w)) else { return vec![] };
        self.next_words_here(&last, InputLang::Thai).into_iter()
            .map(|(w, src, conf)| Candidate { item: BubbleItem { label: w.clone(), kind: BubbleKind::Complete { typed: self.word.clone(), insert: format!("{run}{w}") } }, conf, src })
            .collect()
    }
    /// The completion rows on the bubble now, with their scores.
    fn completions_up(&self) -> Vec<Candidate> { self.shown.iter().filter(|c| matches!(c.item.kind, BubbleKind::Complete { .. })).cloned().collect() }
    /// Re-derive the completion rows under the live row (if it is up); stale ones go. `idle`: the pause has elapsed.
    /// `up`: the rows that were on the bubble before this key.
    fn update_completions(&mut self, idle: bool, up: &[Candidate], acts: &mut Vec<Action>) {
        self.poll_prediction();
        let ask = match (self.opts.complete, idle) {
            (CompleteMode::Typing, false) => Some(Ask::Typing),
            (CompleteMode::Pause, false) if !up.is_empty() => Some(Ask::Refresh),
            (CompleteMode::Pause, true) => Some(Ask::Pause),
            _ => None,
        };
        let (rows, lead) = ask.map_or_else(|| (vec![], None), |ask| self.completion_rows(ask, if ask == Ask::Refresh { up } else { &[] }));
        if ask == Some(Ask::Pause) { self.spell_lead = lead }
        if rows.is_empty() && !self.bubble.iter().any(|b| matches!(b.kind, BubbleKind::Complete { .. })) { return }
        let live = self.shown.first().filter(|c| matches!(&c.item.kind, BubbleKind::Live { typed, .. } if *typed == self.word)).cloned();
        let items: Vec<Candidate> = live.into_iter().chain(rows).collect();
        if Self::items_of(&items) != self.bubble { self.show_candidates(items, acts) }
    }
    /// What the line shows in front of `line[..upto]`: every committed word with the boundary after it.
    fn shown_line(&self, upto: usize) -> String {
        self.line[self.line_start.min(upto)..upto].iter().map(|c| format!("{}{}", c.shown(), c.after())).collect()
    }
    /// What the predictor reads as the line so far: the words of the shown text with the gaps between them
    /// (`typethai_core::predict`), tail-capped the way the model's own clients cap a query.
    fn predict_text(&self, upto: usize) -> String {
        let shown = self.shown_line(upto);
        let n = shown.chars().count();
        let tail: String = if n > PREDICT_BEFORE_CHARS { shown.chars().skip(n - PREDICT_BEFORE_CHARS).collect() } else { shown };
        typethai_core::predict::engine_text(&tail, &self.det.th_dict)
    }
    /// Give the finished line to the predictor. A line is given once, when it ends (Enter, or the line is forgotten),
    /// which is the moment a chat prompt would be submitted: what the user typed within the line reaches the model
    /// through the words around the word being completed.
    fn learn_line(&mut self) {
        if self.line_learned || !self.opts.learn_words || self.predict.is_none() { return }
        self.line_learned = true;
        let text = self.predict_text(self.line.len());
        // an Enter with no words we track (an empty field, a terminal prompt we cannot read) has nothing to learn from
        if text.is_empty() { return }
        if let Some(p) = &self.predict { p.learn(text) }
    }
    /// Take the predictor's answers. The thread answers at most one query at a time and may answer none, so only a
    /// reply for the query the user is still on counts; the others are the states already typed past.
    fn poll_prediction(&mut self) -> bool {
        let Some(p) = &self.predict else { return false };
        let mut answered = false;
        while let Some((key, suffix, confidence)) = p.answer() {
            if key & NEXT_WORD_KEY != 0 { self.pred_next_answer = Some((key, confidence, suffix)); answered = true; continue }
            let Some((asked, tail, thai)) = self.pred_ask.clone() else { continue };
            if asked != key { continue }
            let typed = format!("{tail}{suffix}");
            // Latin words come back lowercased; the row is what the keys typed, case included
            let word = if thai { typed } else { typethai_core::completer::recase(&typed, &tail) };
            self.pred_answer = Some((key, confidence, word));
            answered = true;
        }
        answered
    }
    /// Ask the predictor for the word that follows the line, once per line state: the answer is in hand by the pause
    /// that shows the next-word rows. The context is the line as the model reads it (`predict_text`).
    fn ask_next_word(&mut self) {
        if !self.opts.predict || self.predict.is_none() { self.pred_next_ask = None; self.pred_next_answer = None; return }
        let text = self.predict_text(self.line.len());
        let key = query_key(&text, "", "", self.thai_active) | NEXT_WORD_KEY;
        if self.pred_next_ask == Some(key) { return }
        self.pred_next_ask = Some(key);
        self.pred_next_answer = None;
        if let Some(p) = &self.predict { p.next_word(key, text) }
    }
    /// The word the predictor offers after the line's last word, when its answer for this line state is in: a name,
    /// jargon or a habit of this user's that the built-in word pairs do not have.
    fn predicted_next(&self) -> Option<(String, f64)> {
        let (key, confidence, word) = self.pred_next_answer.as_ref()?;
        (Some(*key) == self.pred_next_ask && !word.is_empty()).then(|| (word.clone(), *confidence))
    }
    /// The row the predictor offers for the word being typed: `(label, insert, confidence)`, or None while its answer
    /// is on the way. The query is asked once per state of the word; until the answer arrives the word last shown
    /// stays, as long as the user is still typing into it.
    fn predicted(&mut self, head: &str, tail: &str, thai: bool, ask: Ask) -> Option<(String, String, f64)> {
        if !self.opts.predict || tail.is_empty() || self.predict.is_none() { return None }
        let line = self.shown_line(self.line.len());
        let key = query_key(&line, head, tail, thai);
        if let Some((answer, confidence, word)) = self.pred_answer.clone() {
            if answer == key {
                if ask.gated() && confidence < self.opts.complete_conf { return None }
                self.pred_shown = Some((line, thai, word.clone(), confidence));
                return Some((word.clone(), format!("{head}{word}"), confidence))
            }
        }
        if let Some((shown, shown_thai, word, confidence)) = self.pred_shown.clone() {
            if shown == line && shown_thai == thai && word.len() > tail.len() && word.starts_with(tail) {
                return Some((word.clone(), format!("{head}{word}"), confidence))
            }
        }
        if self.pred_ask.as_ref().is_none_or(|(asked, ..)| *asked != key) {
            let before = typethai_core::predict::engine_text(&format!("{line}{head}"), &self.det.th_dict);
            self.pred_ask = Some((key, tail.to_string(), thai));
            self.pred_answer = None;
            if let Some(p) = &self.predict { p.ask(key, before, tail.to_string()) }
        }
        None
    }
    /// Completions of the buffer, at most 3 rows with the live row. Thai (a Thai layout run, or a wrong-layout word the
    /// live row converts): the run's last word is completed and the rest kept, labelled with that word only. Otherwise
    /// an English word. `ask` sets the length, the gate and the spell checker; rows in `carry` that still extend the
    /// buffer fill what is left, except the spell checker's lead row (`spell_lead`), which stays first. Also returns the
    /// row the spell checker put first, if it did.
    fn completion_rows(&mut self, ask: Ask, carry: &[Candidate]) -> (Vec<Candidate>, Option<String>) {
        if self.comp_muted || self.word.is_empty() || self.blocked(&self.word) { return (vec![], None) }
        let live = self.bubble.first().and_then(|b| match &b.kind { BubbleKind::Live { typed, thai, .. } if *typed == self.word => Some(thai.clone()), _ => None });
        // English typed on the Thai layout: the reverse live row is what was meant. A word the app always offers back
        // (`ok` -> นา) keeps its Thai completions under that row instead, so the bubble holds both readings.
        if self.thai_active && live.is_some() && !self.rev_wanted(&self.word) { return (vec![], None) }
        let (min, k) = (ask.min_len(), 3 - usize::from(live.is_some()));
        let conf = self.opts.complete_conf;
        let gate = |c: &Completions| !ask.gated() || c.confidence >= conf;
        let thai = self.thai_active || live.is_some();
        // `reading`: the buffer as the rows read it, `head` its part kept in front of each completed word, `tail` the
        // word being typed (Thai runs are not separated by spaces, so the row keeps the head in front of it)
        let mut lead = None;
        let (head, reading, tail, found) = if thai {
            let Some(comp) = &self.thai_comp else { return (vec![], None) };
            let run = en_to_th(&self.word);
            let Some(at) = thai_tail(&run, &self.det.th_dict, comp, 1, self.opts.slips.as_ref()).filter(|&at| run.chars().count() - at >= min) else { return (vec![], None) };
            let (head, tail) = run.split_at(run.char_indices().nth(at).map_or(run.len(), |(b, _)| b));
            // the word before the tail in the run, else the one the line shows before the run
            let d = &self.det.th_dict;
            let prev = match d.segment(head).last() { Some(w) => d.has(w).then(|| w.to_string()), None => self.word_before(self.line.len()) };
            (head.to_string(), run.clone(), tail.to_string(), comp.complete(tail, k, self.context(prev.as_deref(), InputLang::Thai)))
        } else {
            let w = self.word.as_str();
            // an English word is asked from its first letter: `t` offers the frequency list's `the`, the way a ghost
            // text does. Noise stays away by itself — `complete_conf` gates Typing-mode rows (0.4), and the pause is
            // what a Pause-mode row waited for.
            if !w.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') { return (vec![], None) }
            let Some(comp) = &self.en_comp else { return (vec![], None) };
            let prev = self.word_before(self.line.len());
            let mut found = comp.complete(w, k, self.context(prev.as_deref(), InputLang::Latin));
            if ask.spells() { if let Some(sp) = &self.speller {
                // keep the typed prefix's case: the checker may capitalise
                let mut theirs = sp.completions(&self.text_before(self.line.len()), w).into_iter()
                    .filter_map(|g| g.get(w.len()..).filter(|r| !r.is_empty() && g[..w.len()].eq_ignore_ascii_case(w)).map(|rest| format!("{w}{rest}")));
                // the checker ranks by the line and the words the user taught the system: its best leads, the model's
                // words follow, and its others fill what is left
                if let Some(first) = theirs.next() {
                    found.words.retain(|f| !f.eq_ignore_ascii_case(&first));
                    found.words.insert(0, first.clone()); found.words.truncate(k); lead = Some(first);
                }
                for g in theirs {
                    if found.words.len() >= k { break }
                    if !found.words.iter().any(|f| f.eq_ignore_ascii_case(&g)) { found.words.push(g) }
                }
            } }
            (String::new(), w.to_string(), w.to_string(), found)
        };
        // the contextual predictor's word leads the rows that follow the spell checker's, and stands on its own: a word
        // the frequency list does not have, typed before, is exactly what the model is for
        let predicted = self.predicted(&head, &tail, thai, ask);
        if !gate(&found) && predicted.is_none() { return (vec![], None) }
        // one offer, one completion score: every completion candidate carries `found.confidence`, so the gate admits or
        // drops the source as a whole rather than deleting the tail of a stable shortlist. Only when the offer has no
        // score at all (no words from the list) do the carried rows keep theirs.
        let offer_conf = found.confidence;
        let typed = self.word.clone();
        let carried: Vec<Candidate> = carry.iter().filter(|c| match &c.item.kind {
            BubbleKind::Complete { insert, .. } => insert.len() > reading.len() && insert.starts_with(&reading),
            _ => false,
        }).cloned().collect();
        let kept_lead = carried.iter().find(|c| matches!(&c.item.kind, BubbleKind::Complete { insert, .. } if self.spell_lead.as_ref() == Some(insert))).cloned();
        // a carried row is re-issued for the word as it reads now, keeping its text; its source and score are the new
        // offer's when there is one
        let normalize = |c: Candidate| {
            let label = c.item.label;
            let kind = match c.item.kind { BubbleKind::Complete { insert, .. } => BubbleKind::Complete { typed: typed.clone(), insert }, kind => kind };
            let item = BubbleItem { label, kind };
            if offer_conf > 0.0 { Candidate::scored(item, Src::Completion, offer_conf) } else { Candidate { conf: c.conf, src: c.src, item } }
        };
        let kept_lead = kept_lead.map(|c| {
            let label = c.item.label;
            let kind = match c.item.kind { BubbleKind::Complete { insert, .. } => BubbleKind::Complete { typed: typed.clone(), insert }, kind => kind };
            Candidate::bare(BubbleItem { label, kind }, Src::Completion)
        });
        let extensions = found.words.into_iter().map(|w| {
            let insert = format!("{head}{w}");
            Candidate::scored(BubbleItem { label: w, kind: BubbleKind::Complete { typed: self.word.clone(), insert } }, Src::Completion, offer_conf)
        });
        let predicted = predicted.map(|(label, insert, conf)| {
            Candidate::scored(BubbleItem { label, kind: BubbleKind::Complete { typed: self.word.clone(), insert } }, Src::Prediction, conf)
        });
        let mut rows: Vec<Candidate> = vec![];
        for c in kept_lead.into_iter().chain(predicted).chain(extensions).chain(carried.into_iter().map(|c| normalize(c))) {
            if rows.len() == k { break }
            let Some(insert) = (match &c.item.kind { BubbleKind::Complete { insert, .. } => Some(insert.clone()), _ => None }) else { continue };
            if live.as_deref() == Some(insert.as_str()) || rows.iter().any(|r| matches!(&r.item.kind, BubbleKind::Complete { insert: i, .. } if *i == insert)) { continue }
            rows.push(c);
        }
        (rows, lead)
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
    /// What the line shows in front of `line[j]` (the buffer, with `j` = `line.len()`), as the spell checker's context.
    fn text_before(&self, j: usize) -> String { spell_context(&self.shown_line(j)).to_string() }
    /// The last word `line[j - 1]` shows, unless a line break follows it.
    fn word_before(&self, j: usize) -> Option<String> {
        let c = self.line.get(j.checked_sub(1)?)?;
        if c.after().contains('\n') { return None }
        self.screen_words(c.shown()).pop().flatten()
    }
    /// What the bubble about to show is about (`rank::Ctx`): the buffer being typed, or the word just finished — a
    /// boundary offers it before it reaches `line`, and the rows name the text they would replace. Rows for one word,
    /// however many keystrokes refreshed them, are one offer in the rank log.
    fn offer_ctx(&self, items: &[BubbleItem]) -> Ctx {
        let w = if !self.word.is_empty() { self.word.clone() } else {
            items.iter().find_map(|i| match &i.kind {
                BubbleKind::Live { typed, .. } | BubbleKind::Convert { typed, .. } => Some(typed.clone()),
                BubbleKind::Typo { word } => Some(word.clone()),
                BubbleKind::UndoConversion => Some(i.label.clone()),
                _ => None,
            }).or_else(|| self.line.last().map(|c| c.shown().to_string())).unwrap_or_default()
        };
        Ctx { line: self.line.len(), w, prev: self.word_before(self.line.len()), rev: self.thai_active }
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
    /// Enter ends the line and closes the bubble, as Esc would: what was offered is dismissed, not carried to the next
    /// line. The word is still committed (undo and ⌃⌥R can reach it).
    fn on_enter(&mut self) -> Outcome {
        let was = self.bubble_visible();
        let mut o = self.on_boundary(Boundary::Enter);
        o.actions.retain(|a| !matches!(a, Action::ShowBubble(_) | Action::Select(_)));
        if !self.bubble.is_empty() { self.bubble.clear(); self.shown.clear(); self.live = false; self.phrase = 0; }
        if was && !o.actions.contains(&Action::HideBubble) { o.actions.push(Action::HideBubble) }
        o
    }
    fn on_boundary(&mut self, b: Boundary) -> Outcome {
        self.tab_pending = false;
        // a pause after a space may offer the next word
        self.live_streak = 0; self.comp_muted = false; self.last_key = (b == Boundary::Space).then(Instant::now);
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
            let en = self.en_items(&self.text_before(self.line.len()), &typed, prev.as_deref());
            items.extend(en.first().cloned());
            for t in self.typo_items(&thai, prev.as_deref()) { if t.label != thai { items.push(t) } }
            items.extend(en.into_iter().skip(1));
            items.truncate(3);
            self.show(items, &mut acts); self.phrase = 1;
        } else {
            // left as English: offer spelling corrections
            let en = self.en_items(&self.text_before(self.line.len()), &typed, prev.as_deref());
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
        let target = self.rev_target(&typed);
        if self.rev_wanted(&typed) {
            self.show(vec![BubbleItem { label: target.clone(), kind: BubbleKind::ConvertPrev { k: 0 } }], &mut acts); self.phrase = 1;
            self.line.push(Committed { typed, orig: thai, converted: None, boundary: Some(b), rev: true, with_prev: 0, extra: String::new(), learned: false });
            if b == Boundary::Enter { self.new_line(); }
            return Outcome { swallow: false, actions: acts };
        }
        let (p, ok) = self.rev_prob(&typed);
        let mut with_prev = 0usize;
        let sure = ok && p > self.opts.rev_auto;
        if sure || self.rev_suggestable(&typed) {
            // right context: up to 3 preceding words left as Thai (e.g. "i" = ร) are re-scored now that
            // this word follows them, and offered together with it if they clear `rev_prev`
            let k = self.rev_prev_count(&typed);
            if self.opts.auto_convert && sure {
                // legacy (--auto-convert): flip at the boundary
                swallow = true; converted = Some(target.clone()); with_prev = k;
                let start = self.line.len() - k;
                let (mut ins, mut was) = (String::new(), String::new());
                for c in &self.line[start..] { ins += &c.typed; ins += &c.after(); was += &c.orig; was += &c.after(); }
                for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
                acts.push(self.replace(&format!("{was}{thai}"), &format!("{ins}{target}"), Some(b)));
                self.show(vec![BubbleItem { label: format!("{was}{thai}"), kind: BubbleKind::UndoConversion }], &mut acts);
            } else {
                let start = self.line.len() - k;
                let mut label = String::new(); for c in &self.line[start..] { label += &self.rev_target(&c.typed); label += &c.after(); }
                label += &target;
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
        // the finished line goes to the predictor before its last words are kept for the bubble
        self.learn_line();
        // keep the finished line's last few words (a pending ConvertPrev may span up to 4); contexts start after them
        let n = self.line.len().saturating_sub(4); self.line.drain(..n); self.line_start = self.line.len();
        self.line_learned = false;
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
        if rev { return self.rev_target(typed) }
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
                    let target = if c.rev { self.rev_target(&c.typed) } else { en_to_th(&c.typed) };
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
                let how = std::mem::take(&mut self.pick);
                let Some(item) = self.bubble.get(i).cloned() else { return Outcome::pass() };
                // what the user picked, which is what the log is for; the guards below may still turn down a row that
                // went stale, and Convert / UndoConversion hand off to their own hotkey and never reach the end
                self.rank.picked(i, how);
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
            Hotkey::Rate(i, up) => {
                let Some(key) = self.bubble.get(i).and_then(BubbleItem::rated_word).and_then(normalize_word) else { return Outcome { swallow: true, actions: acts } };
                let changed = if up { self.offer_last.remove(&key) } else { self.offer_last.insert(key) };
                // the bubble up now takes the new order too: down, the conversion moves behind the others and Tab
                // applies what is first now; up, it is back in front
                if changed { self.prefs_dirty = true; let items = self.shown.clone(); self.show_candidates(items, &mut acts); }
            }
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
            Key::SelectPrev | Key::SelectNext | Key::Esc | Key::Hotkey(Hotkey::Accept(_) | Hotkey::Dismiss | Hotkey::Rate(..)) => self.bubble_visible(),
            Key::Boundary(Boundary::Enter) => (self.fixing || self.in_bubble) && self.bubble_visible(),
            Key::Arrow(_) => self.bubble_visible() && self.is_arrow_key(k),
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
                let label = if preset == Preset::Finish { "Finishing locally…" } else { rewrite::REWRITING };
                let id = self.mint(); self.show(vec![row(label, RewriteRow::Busy(id))], &mut a); self.fixing = true;
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
        let at = self.line.len();
        if !self.word.is_empty() { return Some(self.word_rows(&self.text_before(at), &self.word, self.thai_active, self.word_before(at))) }
        let c = self.line.last().filter(|_| self.line.len() > self.line_start)?;
        // a converted word's other layout is its undo
        if c.converted.is_some() { return Some(vec![BubbleItem { label: c.orig.clone(), kind: BubbleKind::UndoConversion }]) }
        Some(self.word_rows(&self.text_before(at - 1), &c.typed, c.rev, self.word_before(at - 1)))
    }
    /// The conversion of `typed` (keys typed in the `rev` direction) when it looks meant, and the spelling fixes of the
    /// word as it is and as converted, in the order the boundary bubble uses. A reverse conversion clears a high bar,
    /// so the Thai on screen is not offered fixed next to it; a forward one keeps the English spelling in reach.
    fn word_rows(&self, before: &str, typed: &str, rev: bool, prev: Option<String>) -> Vec<BubbleItem> {
        let (screen, target) = if rev { (en_to_th(typed), self.rev_target(typed)) } else { (typed.to_string(), en_to_th(typed)) };
        let fixes = |text: &str, thai: bool| if thai { self.typo_items(text, prev.as_deref()) } else { self.en_items(before, text, prev.as_deref()) };
        let converts = if rev { self.rev_wanted(typed) || { let (p, ok) = self.rev_prob(typed); ok && p > self.opts.rev_prev } } else { self.guess_anchor_ok(typed, false) };
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
        if self.rewrite.enabled {
            rows.extend(Preset::ALL.into_iter().filter(|p| has_key || *p == Preset::Finish)
                .map(|p| row(p.label(), RewriteRow::Preset(p, text.to_string()))));
        }
        if rows.is_empty() { rows.push(row(rewrite::NO_SUGGESTIONS, RewriteRow::Notice)) }
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
            else if Instant::now() < deadline { self.en_items(spell_context(&text[..a]), w, None).into_iter().next().map(|i| i.label) }
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
        if self.off { return vec![] }
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

/// An engine with no model data at all, for tests outside this module (it exercises only the off-app / settings paths,
/// which need no detector). The model-free sibling of the tests' `bare_engine`.
#[cfg(test)]
pub(crate) fn bare_engine_for_test() -> Engine {
    let lr = |n: usize| typethai_core::classifier::Lr { w: vec![0.0; n], b: 0.0, mu: vec![0.0; n], sd: vec![1.0; n] };
    let flip = typethai_core::classifier::FlipModel { stage1: lr(29), stage2: lr(38), threshold: 0.0, names: vec![] };
    let det = Detector {
        thai: typethai_core::ngram::Ngram::default(), english: typethai_core::ngram::Ngram::default(),
        th_dict: Arc::new(typethai_core::dict::Dict::new(std::iter::empty::<String>())),
        en_words: Arc::new(Default::default()),
        brands: Default::default(), collapse: false, params: typethai_core::detector::Params::default(),
    };
    Engine::new(det, PersonalModel::new(flip, None, typethai_core::personal::DEFAULT_PERSONAL), None, Options::default())
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
    #[test] fn an_off_app_passes_every_key_through_and_shows_nothing() {
        let Some(mut e) = engine() else { return };
        assert!(e.set_app_off(true).is_empty(), "nothing was up");
        assert!(e.app_off());
        // l;ylfu + Space: no conversion, no live bubble, nothing swallowed
        for o in typ(&mut e, "l;ylfu ") {
            assert!(!o.swallow, "{:?}", o.actions);
            assert!(!o.actions.iter().any(|a| matches!(a, Action::Replace { .. } | Action::ShowBubble(_))), "{:?}", o.actions);
        }
        assert_eq!(e.buffer(), "");
        // hotkeys are not the engine's while off either: they reach the app
        for h in [Hotkey::ForceConvert, Hotkey::Undo, Hotkey::Guess, Hotkey::Accept(0), Hotkey::Rewrite] {
            let o = e.on_key(Key::Hotkey(h)); assert!(!o.swallow, "{h:?}"); assert!(o.actions.is_empty(), "{h:?} {:?}", o.actions);
        }
        // bubble keys are ordinary keys here too
        assert!(!e.is_bubble_key(&Key::Tab)); assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Space)));
        // the timer and the focus paths show nothing
        assert!(e.on_idle(Instant::now() + Duration::from_secs(5)).is_empty());
        assert!(e.on_focus(true).is_empty()); assert!(e.set_field(true).is_empty()); assert!(e.on_keystroke(Instant::now()).is_empty());
        assert!(!e.badge_shown());
        // and it is remembered: turning off again changes nothing
        assert!(e.set_app_off(true).is_empty());
    }
    #[test] fn turning_an_app_off_takes_the_bubble_and_preview_down() {
        let Some(mut e) = engine() else { return };
        assert_eq!(e.on_focus(true), vec![Action::Badge(true)]);
        let a = e.set_app_off(true);
        assert!(a.contains(&Action::Badge(false)), "{a:?}"); assert!(!e.badge_shown());
        // a bubble that is up goes too
        e.set_app_off(false);
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        let a = e.set_app_off(true); assert!(a.contains(&Action::HideBubble), "{a:?}"); assert!(!e.bubble_visible());
    }
    #[test] fn turning_an_app_back_on_resumes_conversion() {
        let Some(mut e) = engine() else { return };
        e.set_app_off(true);
        for o in typ(&mut e, "l;ylfu ") { assert!(!o.actions.iter().any(|a| matches!(a, Action::Replace { .. }))); }
        e.set_app_off(false);
        assert!(!e.app_off());
        typ(&mut e, "l;ylfu "); assert!(e.bubble_visible(), "the live bubble is back");
        let o = tab(&mut e); assert!(o.swallow); assert!(o.actions.iter().any(|a| matches!(a, Action::Replace { .. })), "{:?}", o.actions);
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
        // Tab without a bubble is a normal boundary, held until released (Tab+R) and then typed
        typ(&mut e, "hello"); let o = tab(&mut e); assert!(types_only_a_tab(&o), "{:?}", o.actions); assert!(!e.bubble_visible());
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
        let o = tab(&mut e); assert!(types_only_a_tab(&o), "{:?}", o.actions);
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
    /// `l;ylfu` with the live bubble up and arrow-key picking on.
    fn railed() -> Option<(Engine, usize)> {
        let mut e = engine()?;
        e.opts.live_show = 0.5; e.opts.live_stable = 1; e.opts.live_min_len = 3; e.opts.pick_arrows = true;
        typ(&mut e, "l;ylfu"); assert!(e.live_visible());
        let n = e.bubble.len(); Some((e, n))
    }
    #[test] fn plain_arrows_are_navigation_without_the_setting() {
        let Some((mut e, _)) = railed() else { return };
        e.opts.pick_arrows = false;
        assert!(!e.is_bubble_key(&Key::Arrow(Arrow::Up)));
        let o = e.on_key(Key::Arrow(Arrow::Up)); assert!(!o.swallow); assert_eq!(o.actions, vec![Action::HideBubble]); assert_eq!(e.buffer(), "");
    }
    #[test] fn left_right_before_up_move_the_caret() {
        let Some((mut e, _)) = railed() else { return };
        assert!(e.is_bubble_key(&Key::Arrow(Arrow::Up)));
        for a in [Arrow::Left, Arrow::Right, Arrow::Down] { assert!(!e.is_bubble_key(&Key::Arrow(a)), "{a:?}") }
        let o = e.on_key(Key::Arrow(Arrow::Right)); assert!(!o.swallow); assert!(!e.bubble_visible());
        assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Enter)), "never entered: Enter is an ordinary Enter");
    }
    #[test] fn up_enters_the_bubble_and_left_right_move() {
        let Some((mut e, n)) = railed() else { return };
        let o = e.on_key(Key::Arrow(Arrow::Up)); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Focus(true)]);
        assert!(e.is_bubble_key(&Key::Arrow(Arrow::Left))); assert!(e.is_bubble_key(&Key::Boundary(Boundary::Enter)));
        let o = e.on_key(Key::Arrow(Arrow::Right)); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Select(1 % n)]);
        let o = e.on_key(Key::Arrow(Arrow::Left)); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Select(0)]);
        // Down leaves; the bubble and the word stay
        let o = e.on_key(Key::Arrow(Arrow::Down)); assert!(o.swallow); assert_eq!(o.actions, vec![Action::Focus(false)]);
        assert!(e.bubble_visible()); assert_eq!(e.buffer(), "l;ylfu");
        assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Enter)), "out again: Enter is an ordinary Enter");
    }
    #[test] fn up_again_applies() {
        let Some((mut e, _)) = railed() else { return };
        e.on_key(Key::Arrow(Arrow::Up));
        let o = e.on_key(Key::Arrow(Arrow::Up)); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: None });
        assert!(!e.bubble_visible()); assert!(!e.is_bubble_key(&Key::Arrow(Arrow::Up)), "no bubble: Up moves the caret");
    }
    #[test] fn enter_in_the_bubble_applies() {
        let Some((mut e, _)) = railed() else { return };
        e.on_key(Key::Arrow(Arrow::Up));
        let o = e.on_key(Key::Boundary(Boundary::Enter)); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 6, insert: "สวัสดี".into(), then: None });
        assert!(!e.bubble_visible()); assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Enter)));
        // not entered: Enter is an ordinary Enter
        let Some((mut e, _)) = railed() else { return };
        assert!(!e.on_key(Key::Boundary(Boundary::Enter)).swallow);
    }
    #[test] fn typing_leaves_the_bubble() {
        let Some((mut e, _)) = railed() else { return };
        e.on_key(Key::Arrow(Arrow::Up));
        let o = e.on_key(Key::Char('a')); assert!(!o.swallow); assert_eq!(o.actions.first(), Some(&Action::Focus(false)));
        assert!(!e.is_bubble_key(&Key::Boundary(Boundary::Enter)));
    }
    #[test] fn tab_still_applies_inside_the_bubble() {
        let Some((mut e, n)) = railed() else { return };
        if n < 2 { return }
        e.on_key(Key::Arrow(Arrow::Up)); e.on_key(Key::Arrow(Arrow::Right));
        let o = tab(&mut e); assert!(o.swallow); assert!(o.actions.iter().any(|a| matches!(a, Action::Replace { .. }))); assert!(!e.bubble_visible());
        assert!(!e.is_bubble_key(&Key::Arrow(Arrow::Left)), "no bubble: Left moves the caret");
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
        // Enter also passes through, and closes the bubble: what was offered is dismissed
        e.reset(); let o = typ(&mut e, "l;ylfu\n"); assert!(!o.last().unwrap().swallow); assert!(!e.bubble_visible());
        assert!(o.iter().all(|x| bubble_items(x).is_none()), "no bubble flashes up at Enter");
        // a bubble already up is hidden
        e.reset(); typ(&mut e, "l;ylfu "); assert!(e.bubble_visible());
        let o = e.on_key(Key::Boundary(Boundary::Enter)); assert!(!o.swallow); assert!(o.actions.contains(&Action::HideBubble)); assert!(!e.bubble_visible());
        // reverse mode too
        e.reset(); let mut o = typ_th(&mut e, "กนืะ าืนไ"); o.push(e.on_key(Key::Boundary(Boundary::Enter))); assert!(replaced(&o).is_empty()); assert!(!e.bubble_visible()); assert!(o.iter().all(|x| !x.swallow));
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
        typ(&mut e, "hello"); let o = tab(&mut e); assert!(types_only_a_tab(&o), "{:?}", o.actions);
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
        // 'i dont know' typed on the Thai layout, then Space (passes through); the bubble offers the whole phrase
        let keys = typethai_core::reverse::th_text_to_keys("ร กนืะ าืนไ").unwrap();
        e.set_thai_active(true);
        let mut log: Vec<(Option<char>, Outcome)> = vec![];
        for c in keys.chars() { let shown = if c == ' ' { ' ' } else { en_to_th(&c.to_string()).chars().next().unwrap() }; log.push((Some(shown), e.on_key(key_of(c)))); }
        log.push((Some(' '), e.on_key(Key::Boundary(Boundary::Space))));
        assert_eq!(screen(&log), "ร กนืะ าืนไ ", "nothing replaced automatically");
        let items = e.bubble.clone(); assert_eq!(items.len(), 1); assert_eq!(items[0].label, "i dont know");
        log.push((Some('\t'), tab(&mut e)));
        assert_eq!(screen(&log), "i dont know ");
        assert!(log.last().unwrap().1.actions.contains(&Action::SwitchInput(InputLang::Latin)));
        // undo restores all three words
        log.push((None, e.on_key(Key::Hotkey(Hotkey::Undo)))); assert_eq!(screen(&log), "ร กนืะ าืนไ ");
    }
    #[test] fn reverse_real_thai_gets_no_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        for th in ["สวัสดีครับ วันนี้อากาศดี", "ไม่รู้ ไม่ชี้", "กิน ข้าว ยัง", "ครับ"] {
            e.reset(); let mut o = typ_th(&mut e, th); o.push(e.on_key(Key::Boundary(Boundary::Enter)));
            assert!(o.iter().all(|x| bubble_items(x).is_none()), "{th}");
        }
    }
    #[test] fn na_offers_ok_on_the_thai_layout() {
        let Some(mut e) = engine() else { return };
        for never in [false, true] {
            e.reset(); e.never.clear(); if never { e.never.insert("ok".into()); }
            let o = typ_th(&mut e, "นา ");
            let items = o.iter().find_map(bubble_items).unwrap_or_else(|| panic!("นา offers ok (never: {never})"));
            assert_eq!(items[0].label, "ok");
            let applied = [e.on_key(Key::Tab), e.on_key(Key::TabUp)];
            assert_eq!(replaced(&applied), ["ok "], "Tab applies ok (never: {never})");
        }
        e.reset(); e.never.clear();
        assert!(typ_th(&mut e, "ทำนา ").iter().all(|x| bubble_items(x).is_none()), "นา inside a Thai word stays");
        // Tab right after the keys, before Space, offers it too
        e.reset(); typ_th(&mut e, "นา"); e.on_key(Key::Tab); e.on_key(Key::TabUp);
        assert_eq!(label0(&e).as_deref(), Some("ok"));
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
        assert!(types_only_a_tab(&o), "{:?}", o.actions);
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
        fn guesses(&self, _: &str, word: &str) -> Option<Vec<String>> {
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
        assert_eq!(presets(&e), Preset::ALL);
        let o = tab(&mut e); let run = job_run(&o);
        assert_eq!((run.preset, run.text.as_str()), (Preset::Fix, "helo wrld"));
        assert!(matches!(e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Busy(id)) if id == run.req));
        e.on_reply(Reply::Rewritten(run.req, Ok("hello world".into())));
        assert_eq!(labels(&e), ["hello world"]);
        let o = tab(&mut e); assert!(o.swallow);
        assert_eq!(o.actions, vec![Action::HideBubble, Action::Rewrite(Job::Paste("hello world".into()))]);
        assert!(!e.bubble_visible());
        assert!(types_only_a_tab(&tab(&mut e)), "the session is over: Tab is a tab again");
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
        assert_eq!(presets(&e), Preset::ALL);
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
    fn presets(e: &Engine) -> Vec<Preset> {
        e.bubble.iter().filter_map(|b| match b.kind {
            BubbleKind::Rewrite(RewriteRow::Preset(p, _)) => Some(p),
            _ => None,
        }).collect()
    }
    fn selection_options(e: &mut Engine, text: &str, local: &[&str]) {
        options(e, Captured::Text(text.into()));
        assert_eq!(presets(e), Preset::ALL);
        let labels: Vec<_> = e.bubble.iter().filter(|b| b.reason().is_some()).map(|b| b.label.as_str()).collect();
        assert_eq!(labels, local);
    }
    fn pending_finish(e: &mut Engine, text: &str) -> Run {
        let id = start(e);
        let actions = e.on_reply(Reply::Captured { id, sel: Captured::Text(text.into()), has_key: false });
        assert!(!actions.iter().any(|a| matches!(a, Action::Rewrite(_))));
        assert_eq!(presets(e), [Preset::Finish]);
        let index = e.bubble.iter().position(|b| matches!(b.kind, BubbleKind::Rewrite(RewriteRow::Preset(Preset::Finish, _)))).unwrap();
        let run = job_run(&e.on_key(Key::Hotkey(Hotkey::Accept(index))));
        assert_eq!((run.preset, run.text.as_str()), (Preset::Finish, text));
        assert!(matches!(e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Busy(id)) if id == run.req));
        run
    }
    #[test] fn finish_requires_an_explicit_request_then_preview_acceptance() {
        let Some(mut e) = engine() else { return };
        for enter in [false, true] {
            let run = pending_finish(&mut e, "hello");
            assert_eq!(tab(&mut e), Outcome { swallow: true, actions: vec![] });
            let actions = e.on_reply(Reply::Rewritten(run.req, Ok("hello world.".into())));
            assert!(!actions.iter().any(|a| matches!(a, Action::Rewrite(_))));
            assert!(matches!(&e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Result(text)) if text == "hello world."));
            let accepted = if enter { e.on_key(Key::Boundary(Boundary::Enter)) } else { tab(&mut e) };
            assert!(accepted.swallow);
            assert_eq!(accepted.actions, vec![Action::HideBubble, Action::Rewrite(Job::Paste("hello world.".into()))]);
            assert!(!e.bubble_visible());
        }
    }
    #[test] fn finish_cancellation_ignores_late_replies_and_preserves_new_requests() {
        let Some(mut e) = engine() else { return };
        for key in [Some(Key::Esc), Some(Key::Char('x')), Some(Key::Backspace), None] {
            let old = pending_finish(&mut e, "hello");
            match key { Some(k) => { e.on_key(k); }, None => { e.reset(); } }
            assert!(!e.bubble_visible());
            assert!(e.on_reply(Reply::Rewritten(old.req, Ok("hello world.".into()))).is_empty());
            let new = pending_finish(&mut e, "hello again");
            assert_ne!(old.req, new.req);
            assert!(e.on_reply(Reply::Rewritten(old.req, Ok("hello world.".into()))).is_empty());
            assert!(matches!(e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Busy(id)) if id == new.req));
            e.on_reply(Reply::Rewritten(new.req, Ok("hello again friend.".into())));
            assert!(matches!(&e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Result(text)) if text == "hello again friend."));
            e.reset();
        }
    }
    #[test] fn disabled_ai_never_offers_finish_with_or_without_a_key() {
        let Some(mut e) = engine() else { return };
        e.rewrite.enabled = false;
        for has_key in [false, true] {
            let id = start(&mut e);
            e.on_reply(Reply::Captured { id, sel: Captured::Text("hello world".into()), has_key });
            assert!(presets(&e).is_empty());
            assert!(matches!(e.bubble.as_slice(), [BubbleItem { kind: BubbleKind::Rewrite(RewriteRow::Notice), .. }]));
            e.reset();
        }
    }
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
    #[test] fn held_tab_fixes_the_word_being_typed() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.speller = Some(Box::new(FakeSpeller));
        let mut log = vec![];
        feed(&mut e, &mut log, "is ths");
        log.push((None, e.on_key(Key::Tab)));
        let a = tick_after(&mut e, TAB_HOLD); let id = match a.iter().find_map(|a| if let Action::Rewrite(Job::Capture(id)) = a { Some(*id) } else { None }) { Some(id) => id, None => panic!("{a:?}") };
        assert!(!a.iter().any(|a| matches!(a, Action::Replace { .. })), "the hold types no Tab: {a:?}");
        log.push((None, Outcome { swallow: false, actions: a.clone() }));
        log.push((None, Outcome { swallow: false, actions: e.on_reply(captured(id, Captured::NoSelection)) }));
        assert_eq!(labels(&e), ["this", "the"]);
        log.push((None, e.on_key(Key::TabUp)));
        log.push((None, tab(&mut e)));
        assert_eq!(screen(&log), "is this", "the fix replaces the word, nothing before it");
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
        selection_options(&mut e, "l;ylfu", &["สวัสดี"]);
        let o = tab(&mut e);
        assert_eq!(o, Outcome { swallow: true, actions: vec![Action::HideBubble, Action::Rewrite(Job::Paste("สวัสดี".into()))] });
        selection_options(&mut e, &en_to_th("i dont know"), &["i dont know"]);
        e.on_key(Key::Esc);
        selection_options(&mut e, "l;ylfu world\n", &["สวัสดี world\n"]);
    }
    #[test] fn a_selection_with_typos_is_offered_fixed() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        selection_options(&mut e, "is ths", &["is this"]);
        e.on_key(Key::Esc);
        selection_options(&mut e, "รานอาหาร", &["ร้านอาหาร"]);
        e.on_key(Key::Esc);
        selection_options(&mut e, "ขอบคุนมาก ประเทษไทย\n", &["ขอบคุณมาก ประเทศไทย\n"]);
    }
    #[test] fn each_selection_row_says_what_it_fixes() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        let reasons = |e: &Engine| e.bubble.iter().map(BubbleItem::reason).collect::<Vec<_>>();
        options(&mut e, Captured::Text("l;ylfu".into()));
        assert_eq!(reasons(&e), [Some(Reason::Layout(InputLang::Thai)), None, None, None], "to Thai, then the AI rows");
        e.on_key(Key::Esc);
        options(&mut e, Captured::Text(en_to_th("i dont know")));
        assert_eq!(reasons(&e), [Some(Reason::Layout(InputLang::Latin)), None, None, None], "back to the English keys");
        e.on_key(Key::Esc);
        options(&mut e, Captured::Text("is ths".into()));
        assert_eq!(reasons(&e), [Some(Reason::Typo), None, None, None]);
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
    #[test] fn a_correct_selection_gets_ai_rows_and_finish_needs_no_key() {
        let Some(mut e) = engine_typo() else { return };
        e.speller = Some(Box::new(FakeSpeller));
        selection_options(&mut e, "hello world", &[]);
        e.on_key(Key::Esc);
        selection_options(&mut e, "สวัสดีครับ ยินดีที่ได้รู้จัก", &[]);
        e.on_key(Key::Esc);
        selection_options(&mut e, "วันนี้อากาศดีมากเลยนะ", &[]);
        e.on_key(Key::Esc);
        let id = start(&mut e); e.on_reply(Reply::Captured { id, sel: Captured::Text("hello world".into()), has_key: false });
        assert_eq!(presets(&e), [Preset::Finish]);
        assert_eq!(e.bubble.len(), 1);
        e.on_key(Key::Esc);
        let id = start(&mut e); e.on_reply(Reply::Captured { id, sel: Captured::Text("is ths".into()), has_key: false });
        assert_eq!(presets(&e), [Preset::Finish]);
        assert_eq!(e.bubble.len(), 2);
        assert!(matches!(&e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Fixed(Reason::Typo, text)) if text == "is this"));
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
        selection_options(&mut e, &long, &[]);
    }
    #[test] fn rewrite_secure_refuses_silently() {
        let Some(mut e) = engine() else { return };
        let id = start(&mut e);
        assert_eq!(e.on_reply(captured(id, Captured::Secure)), vec![]);
        assert!(types_only_a_tab(&tab(&mut e)), "no session left");
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
        assert_eq!(presets(&e), Preset::ALL);
    }
    #[test] fn rewrite_tab_while_pending_keeps_waiting() {
        let Some(mut e) = engine() else { return };
        let run = pending(&mut e, "helo");
        assert_eq!(tab(&mut e), Outcome { swallow: true, actions: vec![] });
        assert!(matches!(e.bubble[0].kind, BubbleKind::Rewrite(RewriteRow::Busy(id)) if id == run.req));
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
    /// The outcome types a Tab and changes nothing else on screen.
    fn types_only_a_tab(o: &Outcome) -> bool { o.swallow && o.actions.iter().filter(|a| matches!(a, Action::Replace { .. })).eq([&TYPE_TAB]) }
    fn hold_tab(e: &mut Engine) -> Outcome { let o = e.on_key(Key::Tab); e.tab_at = e.tab_at.map(|t| t - TAB_CHORD_MIN); o }
    fn captures(o: &Outcome) -> bool { o.actions.iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))) }
    const TYPE_TAB: Action = Action::Replace { delete: 0, insert: String::new(), then: Some(Boundary::Tab) };
    #[test] fn tab_after_a_word_is_held_and_typed_on_release() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ(&mut e, "hello"); assert!(!e.bubble_visible());
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "held back in case R follows");
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "auto-repeat while held");
        let o = e.on_key(Key::TabUp); assert!(o.swallow); assert!(o.actions.contains(&TYPE_TAB), "the release types the Tab: {:?}", o.actions);
        // nothing typed yet on the line: held too (a held Tab opens the ⌃⌥R rows on a selection)
        e.reset(); assert_eq!(e.on_key(Key::Tab), SWALLOWED);
    }
    #[test] fn tab_r_opens_the_rewrite_rows() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ(&mut e, "hello");
        hold_tab(&mut e);
        let o = e.on_key(Key::Char('r')); assert!(o.swallow); assert!(captures(&o), "Tab+R starts ⌃⌥R: {:?}", o.actions);
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "auto-repeat after R types nothing");
        assert_eq!(e.on_key(Key::TabUp), SWALLOWED, "the release types no Tab");
        // over the bubble too, instead of applying a row
        e.reset(); e.opts.live = true; typ(&mut e, "l;ylfu"); assert!(e.bubble_visible());
        hold_tab(&mut e);
        let o = e.on_key(Key::Char('R')); assert!(captures(&o)); assert!(!o.actions.iter().any(|a| matches!(a, Action::Replace { .. })), "no row applied");
    }
    /// The timer tick `after` the held Tab went down.
    fn tick_after(e: &mut Engine, after: Duration) -> Vec<Action> { let t = e.tab_at.expect("Tab is down") + after; e.on_idle(t) }
    #[test] fn holding_tab_alone_starts_a_rewrite() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        // nothing typed: the selection case. Every Tab is held
        assert_eq!(e.on_key(Key::Tab), SWALLOWED);
        assert!(!tick_after(&mut e, TAB_HOLD - Duration::from_millis(1)).iter().any(|a| matches!(a, Action::Rewrite(_))), "not yet");
        let a = tick_after(&mut e, TAB_HOLD); assert!(a.iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))), "{a:?}");
        assert!(!a.iter().any(|a| matches!(a, Action::Replace { .. })), "the hold types no Tab: {a:?}");
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "auto-repeat after the rewrite types nothing");
        assert_eq!(e.on_key(Key::TabUp), SWALLOWED, "the release types no Tab");
        // over the bubble, instead of applying a row on release
        e.reset(); e.opts.live = true; typ(&mut e, "l;ylfu"); assert!(e.bubble_visible());
        e.on_key(Key::Tab);
        let a = tick_after(&mut e, TAB_HOLD); assert!(a.iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))));
        assert!(!a.iter().any(|a| matches!(a, Action::Replace { .. })), "no row applied");
    }
    fn holds(a: &[Action]) -> Vec<&Action> { a.iter().filter(|a| matches!(a, Action::Hold(_))).collect() }
    /// The indicator for the Tab that went down at `t`.
    fn hold_from(t: Instant) -> Action { Action::Hold(Some(Hold { from: t + TAB_CHORD_MIN, full: t + TAB_HOLD })) }
    #[test] fn the_hold_indicator_shows_once_tab_is_held_past_a_tap() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        e.on_key(Key::Tab); let t = e.tab_at.unwrap();
        assert!(holds(&tick_after(&mut e, TAB_CHORD_MIN - Duration::from_millis(1))).is_empty(), "a tap shows nothing");
        assert_eq!(holds(&tick_after(&mut e, TAB_CHORD_MIN)), [&hold_from(t)], "filling from now until the rows open");
        assert!(holds(&tick_after(&mut e, (TAB_CHORD_MIN + TAB_HOLD) / 2)).is_empty(), "shown once");
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "auto-repeat keeps it");
        let a = tick_after(&mut e, TAB_HOLD);
        assert_eq!(a.first(), Some(&Action::Hold(None)), "gone as the rewrite starts: {a:?}");
        assert!(a.iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))));
        assert!(holds(&e.on_key(Key::TabUp).actions).is_empty(), "already gone");
    }
    #[test] fn the_hold_indicator_goes_on_release_or_another_key() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        e.on_key(Key::Tab); tick_after(&mut e, TAB_CHORD_MIN);
        let o = e.on_key(Key::TabUp);
        assert_eq!(o.actions.first(), Some(&Action::Hold(None)), "released early: {:?}", o.actions);
        assert!(types_only_a_tab(&o), "and the Tab is typed as before");
        typ(&mut e, "hello"); e.on_key(Key::Tab); tick_after(&mut e, TAB_CHORD_MIN);
        let o = e.on_key(Key::Char('a')); assert_eq!(o.actions.first(), Some(&Action::Hold(None)), "typed on");
        hold_tab(&mut e); tick_after(&mut e, TAB_CHORD_MIN);
        let o = e.on_key(Key::Char('r')); assert_eq!(o.actions.first(), Some(&Action::Hold(None)), "Tab+R"); assert!(captures(&o));
        e.on_key(Key::TabUp);
        e.on_key(Key::Tab); tick_after(&mut e, TAB_CHORD_MIN);
        assert_eq!(e.reset().first(), Some(&Action::Hold(None)), "focus moved");
        assert!(holds(&e.reset()).is_empty());
    }
    #[test] fn the_hold_indicator_shows_over_the_bubble_too() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "l;ylfu"); assert!(e.bubble_visible());
        e.on_key(Key::Tab); let t = e.tab_at.unwrap();
        assert_eq!(holds(&tick_after(&mut e, TAB_CHORD_MIN)), [&hold_from(t)]);
        let o = e.on_key(Key::Char('1')); assert_eq!(o.actions.first(), Some(&Action::Hold(None)), "Tab+1 applies a row");
        assert!(o.actions.iter().any(|a| matches!(a, Action::Replace { .. })));
    }
    #[test] fn hold_progress_fills_from_the_tap_limit_to_the_hold() {
        let t = Instant::now(); let h = Hold { from: t + TAB_CHORD_MIN, full: t + TAB_HOLD };
        assert_eq!((h.progress(t), h.progress(t + TAB_CHORD_MIN), h.progress(t + TAB_HOLD), h.progress(t + TAB_HOLD * 2)), (0.0, 0.0, 1.0, 1.0));
        assert!((h.progress(t + (TAB_CHORD_MIN + TAB_HOLD) / 2) - 0.5).abs() < 1e-6);
    }
    #[test] fn the_hold_follows_its_setting() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false; e.opts.tab_hold = Duration::from_millis(1000);
        e.on_key(Key::Tab); let t = e.tab_at.unwrap();
        let a = tick_after(&mut e, TAB_CHORD_MIN);
        assert_eq!(holds(&a), [&Action::Hold(Some(Hold { from: t + TAB_CHORD_MIN, full: t + Duration::from_millis(1000) }))], "fills towards the setting");
        assert!(!tick_after(&mut e, TAB_HOLD).iter().any(|a| matches!(a, Action::Rewrite(_))), "not at the default");
        assert!(tick_after(&mut e, Duration::from_millis(1000)).iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))));
    }
    #[test] fn the_quickest_hold_still_lets_a_tap_through() {
        // a tap is under the indicator's start, and the quickest hold is past it
        assert!(TAB_HOLD_CHOICES_MS.iter().all(|&ms| Duration::from_millis(ms) > TAB_CHORD_MIN));
        assert_eq!(TAB_HOLD_CHOICES_MS[0], 200);
    }
    #[test] fn a_tab_tap_is_still_a_tab() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "held until released, in case it is held");
        assert!(tick_after(&mut e, Duration::from_millis(100)).is_empty());
        assert!(types_only_a_tab(&e.on_key(Key::TabUp)));
        // AI rewrite off: the hold still opens the ⌃⌥R rows (spelling fixes)
        e.rewrite.enabled = false; e.on_key(Key::Tab);
        assert!(tick_after(&mut e, TAB_HOLD).iter().any(|a| matches!(a, Action::Rewrite(Job::Capture(_)))));
    }
    #[test] fn tab_rolled_into_r_types_both() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        typ(&mut e, "hello");
        e.on_key(Key::Tab);
        let o = e.on_key(Key::Char('r'));
        assert!(o.swallow); assert!(!captures(&o), "too quick for a chord");
        let replaces: Vec<&Action> = o.actions.iter().filter(|a| matches!(a, Action::Replace { .. })).collect();
        assert_eq!(replaces, [&TYPE_TAB, &Action::Replace { delete: 0, insert: "r".into(), then: None }], "Tab first, then r");
        assert_eq!(e.buffer(), "r");
        assert_eq!(e.on_key(Key::TabUp), Outcome::pass(), "the late release is not routed");
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
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "with the bubble gone, Tab is a tab again (held for Tab+R)");
        assert!(types_only_a_tab(&e.on_key(Key::TabUp)));
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
    #[test] fn the_input_badge_setting_is_on_until_turned_off() {
        let Some(mut e) = engine() else { return };
        assert!(e.opts.input_badge && e.settings_snapshot().input_badge);
        assert!(e.apply_settings(SettingsCmd::SetInputBadge(false)));
        assert!(!e.apply_settings(SettingsCmd::SetInputBadge(false)), "same value is not a change");
        assert!(!e.settings_snapshot().input_badge);
        assert!(SettingsCmd::SetInputBadge(true).writes_prefs());
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
        assert_eq!(e.on_key(Key::Tab), SWALLOWED, "a plain tab, held for Tab+R");
        let o = e.on_key(Key::Char('2'));
        assert_eq!(o.actions, [TYPE_TAB, Action::Replace { delete: 0, insert: "2".into(), then: None }], "the Tab, then the digit");
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
        // while typing, the confidence gate — not the number of keys — decides: `he` is already a clear favourite
        typ(&mut e, "he"); assert_eq!(labels(&e), ["help", "hello", "helmet"], "2 letters, one clear favourite");
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
        assert_eq!(idle(&mut e, COMPLETE_IDLE_DEFAULT_MS - 10), vec![]);
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
    #[test] fn a_pause_after_a_space_offers_the_next_word() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.set_thai_active(false);
        e.en_comp = Some(Completer::new([("you", 50.0), ("can", 20.0), ("are", 30.0), ("zebra", 90.0)].map(|(w, f)| (w.to_string(), f))));
        e.en_bigrams = Some(Bigrams::new([("you", "can", 9.0), ("you", "are", 5.0)].map(|(p, n, c)| (p.to_string(), n.to_string(), c))));
        typ(&mut e, "You "); e.show(vec![], &mut vec![]);
        idle(&mut e, 600);
        assert_eq!(labels(&e), ["can", "are"], "the pairs after `you`, whatever its case");
        let o = tab(&mut e); assert_eq!(o.actions[0], Action::Replace { delete: 0, insert: "can".into(), then: None });
        // a word the pairs do not know offers nothing, nor one with no clear favourite after it, nor Off
        e.reset(); typ(&mut e, "zebra "); e.show(vec![], &mut vec![]); idle(&mut e, 600); assert!(!e.bubble_visible());
        e.en_bigrams = Some(Bigrams::new(["can", "are", "zebra", "you", "x", "y"].map(|n| ("you".to_string(), n.to_string(), 1.0))));
        e.reset(); typ(&mut e, "you "); e.show(vec![], &mut vec![]); idle(&mut e, 600); assert!(!e.bubble_visible());
        e.en_bigrams = Some(Bigrams::new([("you", "can", 9.0), ("you", "are", 5.0)].map(|(p, n, c)| (p.to_string(), n.to_string(), c))));
        e.reset(); e.opts.complete = CompleteMode::Off; typ(&mut e, "you "); e.show(vec![], &mut vec![]); idle(&mut e, 600); assert!(!e.bubble_visible());
        // nor the setting turned off
        e.reset(); e.opts.complete = CompleteMode::Pause;
        assert!(e.apply_settings(SettingsCmd::SetNextWord(false))); assert!(!e.settings_snapshot().next_word);
        typ(&mut e, "you "); e.show(vec![], &mut vec![]); idle(&mut e, 600); assert!(!e.bubble_visible());
    }
    #[test] fn thumbs_down_puts_a_conversion_last_and_up_puts_it_back() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.set_thai_active(false);
        let mut m = WordMemory::default(); for w in ["lets", "lets", "cool", "cool"] { m.learn(Some("ok"), w) } e.words = Arc::new(m);
        typ(&mut e, "ok "); idle(&mut e, 600);
        assert_eq!(labels(&e), ["นา", "cool", "lets"]);
        assert_eq!(e.on_key(Key::Hotkey(Hotkey::Rate(3, false))).actions, vec![], "no option there: nothing");
        e.on_key(Key::Hotkey(Hotkey::Rate(0, false)));
        assert_eq!((labels(&e), e.selected()), (vec!["cool".to_string(), "lets".into(), "นา".into()], 0), "last now; Tab takes cool");
        assert!(e.prefs_dirty && e.offer_last.contains("ok"));
        assert!(e.personal.learned().iter().all(|l| l.word != "ok"), "the personal model is not told: นา stays on offer");
        // from now on: still offered, after the rest, however often it is rated down
        for _ in 0..5 {
            e.reset(); typ(&mut e, "ok ");
            assert_eq!(labels(&e), ["นา"], "alone, it is the one option");
            idle(&mut e, 600);
            assert_eq!(labels(&e), ["cool", "lets", "นา"]);
            e.on_key(Key::Hotkey(Hotkey::Rate(2, false)));
        }
        // up: back in front, now and next time
        e.on_key(Key::Hotkey(Hotkey::Rate(2, true)));
        assert_eq!(labels(&e), ["นา", "cool", "lets"]);
        e.reset(); typ(&mut e, "ok "); idle(&mut e, 600);
        assert_eq!(labels(&e), ["นา", "cool", "lets"]);
        assert!(e.offer_last.is_empty());
    }
    #[test] fn your_own_pairs_lead_the_next_words_and_join_a_layout_flip() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.set_thai_active(false);
        e.en_bigrams = Some(Bigrams::new([("ok", "so", 9.0), ("ok", "i", 1.0)].map(|(p, n, c)| (p.to_string(), n.to_string(), c))));
        e.en_comp = Some(Completer::new([("so", 50.0), ("i", 50.0)].map(|(w, f)| (w.to_string(), f))));
        let mine = |pairs: &[(usize, &str)]| { let mut m = WordMemory::default(); for &(n, w) in pairs { for _ in 0..n { m.learn(Some("ok"), w) } } Arc::new(m) };
        e.words = mine(&[(5, "lets"), (3, "done"), (2, "cool"), (1, "then")]);
        typ(&mut e, "ok ");
        assert_eq!(labels(&e), ["นา"], "the layout flip at Space");
        idle(&mut e, 600);
        assert_eq!(labels(&e), ["นา", "lets", "done", "cool"], "yours after it, best first; a word typed once and the corpus's have no room");
        assert_eq!(e.selected(), 0, "Tab still applies the flip");
        assert_eq!(e.on_key(Key::Hotkey(Hotkey::Accept(3))).actions[0], Action::Replace { delete: 0, insert: "cool".into(), then: None });
        // no flip: three next words, the corpus filling after yours
        e.reset(); e.words = mine(&[(2, "cool")]);
        typ(&mut e, "ok "); e.show(vec![], &mut vec![]); idle(&mut e, 600);
        assert_eq!(labels(&e), ["cool", "so"]);
        // learn_words off: the corpus alone
        e.reset(); e.opts.learn_words = false;
        typ(&mut e, "ok "); e.show(vec![], &mut vec![]); idle(&mut e, 600);
        assert_eq!(labels(&e), ["so"]);
    }
    #[test] fn a_pause_after_a_thai_word_offers_the_next_one_without_a_space() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause); e.opts.live = false;
        e.thai_comp = Some(Completer::new([("แซ่บ", 10.0), ("มาก", 50.0), ("ของ", 40.0)].map(|(w, f)| (w.to_string(), f))));
        e.thai_bigrams = Some(Bigrams::new([("แซ่บ", "มาก", 9.0), ("แซ่บ", "ของ", 1.0)].map(|(p, n, c)| (p.to_string(), n.to_string(), c))));
        typ_th(&mut e, "แซ่บ"); idle(&mut e, 600);
        assert_eq!(labels(&e), ["มาก"], "only the clear favourite, labelled with the next word");
        assert_eq!(e.bubble[0].kind, BubbleKind::Complete { typed: keys("แซ่บ"), insert: "แซ่บมาก".into() });
        assert_eq!(tab(&mut e).actions[0], Action::Replace { delete: 0, insert: "มาก".into(), then: None });
        // a run that does not end on a dictionary word offers no next word, nor does the setting turned off
        e.reset(); typ_th(&mut e, "แซ่"); idle(&mut e, 600); assert!(!labels(&e).contains(&"มาก".to_string()));
        e.reset(); e.opts.next_word = false; typ_th(&mut e, "แซ่บ"); idle(&mut e, 600); assert!(!labels(&e).contains(&"มาก".to_string()));
    }
    fn complete(typed: &str, insert: &str) -> BubbleItem { BubbleItem { label: insert.into(), kind: BubbleKind::Complete { typed: typed.into(), insert: insert.into() } } }
    /// One English letter is enough: after the pause the frequency list ranks `h` to its top words, and Tab applies the
    /// whole word to the single typed letter.
    #[test] fn one_english_letter_is_enough_for_a_completion() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        typ(&mut e, "h");
        assert_eq!(labels(&e), Vec::<String>::new(), "nothing while the keys are still coming");
        let o = idle(&mut e, 600);
        let items = bubble_items(&Outcome { swallow: false, actions: o }).cloned().unwrap_or_default();
        assert_eq!(items, vec![complete("h", "help"), complete("h", "hello"), complete("h", "helmet")], "{:?}", labels(&e));
        tab(&mut e);
        assert_eq!(e.buffer(), "", "the row was applied and the word is the one it inserted");
    }
    /// Thai asks from one key too, after a pause; while typing it still waits for three keys.
    #[test] fn one_thai_key_is_enough_after_a_pause() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.opts.live = false;
        typ_th(&mut e, "ภ");
        let o = idle(&mut e, 600);
        assert!(bubble_items(&Outcome { swallow: false, actions: o }).is_some_and(|v| v.iter().any(|b| b.label == "ภาษา")), "{:?}", labels(&e));
        // while typing, the same word needs three keys: two are not enough, the third offers it
        let Some(mut t) = engine() else { return };
        completers(&mut t, CompleteMode::Typing);
        t.opts.live = false;
        typ_th(&mut t, "ภา");
        assert_eq!(labels(&t), Vec::<String>::new(), "two keys while typing");
        typ_th(&mut t, "ษ");
        assert!(labels(&t).iter().any(|l| l == "ภาษา"), "{:?}", labels(&t));
    }
    // ---------- the contextual predictor ----------
    /// An engine with the words the test needs and no model at all: only the predictor's path is exercised, so it does
    /// not need `TYPETHAI_MODEL`.
    fn bare_engine(th_dict: &[&str], en_words: &[&str]) -> Engine {
        // the classifier's own feature counts: 29 for a token, those plus 9 context features for the second stage
        let lr = |n: usize| typethai_core::classifier::Lr { w: vec![0.0; n], b: 0.0, mu: vec![0.0; n], sd: vec![1.0; n] };
        let flip = typethai_core::classifier::FlipModel { stage1: lr(29), stage2: lr(38), threshold: 0.0, names: vec![] };
        let det = Detector {
            thai: typethai_core::ngram::Ngram::default(), english: typethai_core::ngram::Ngram::default(),
            th_dict: Arc::new(typethai_core::dict::Dict::new(th_dict.iter().map(|w| w.to_string()))),
            en_words: Arc::new(en_words.iter().map(|w| w.to_string()).collect()),
            brands: Default::default(), collapse: false, params: typethai_core::detector::Params::default(),
        };
        Engine::new(det, PersonalModel::new(flip, None, DEFAULT_PERSONAL), None, Options::default())
    }
    /// The predictor's thread over a fresh state directory, trusting `known` as real words (the host passes its own
    /// dictionaries; the model itself only ever learns what this user typed).
    fn predictor(e: &mut Engine, tag: &str, known: &[&str]) {
        let dir = std::env::temp_dir().join(format!("typethai-predict-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let words: std::collections::HashSet<String> = known.iter().map(|w| w.to_string()).collect();
        let oracle = Arc::new(typethai_core::predict::KnownWords::new(Arc::new(typethai_core::dict::Dict::new(words.iter().cloned())), Arc::new(words)));
        e.predict = Some(crate::predict::Predict::spawn(dir, oracle));
    }
    /// Tick the idle timer until `done`: the model answers from its own thread, so the row arrives a tick later.
    fn until_idle(e: &mut Engine, done: impl Fn(&Engine) -> bool) -> bool {
        for _ in 0..200 {
            e.on_idle(Instant::now() + Duration::from_millis(600));
            if done(e) { return true }
            std::thread::sleep(Duration::from_millis(5));
        }
        done(e)
    }
    /// A word the app always offers back as its English (`ok` -> นา) gets that row after the pause too — first, with the
    /// Thai completions under it — and Tab applies the English.
    #[test] fn the_always_offered_english_joins_the_pause_rows() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.thai_comp = Some(Completer::new([("นาน".to_string(), 9.0), ("นาง".to_string(), 5.0)]));
        typ_th(&mut e, "นา");
        assert_eq!(e.buffer(), "ok", "the keys typed for นา");
        idle(&mut e, 600);
        assert_eq!(labels(&e), ["ok", "นาน", "นาง"], "the English first, the Thai words under it");
        assert_eq!(replaced(&[tab(&mut e)]), ["ok"], "Tab applies the English row");
        assert_eq!(e.buffer(), "", "and the word is the one it inserted");
    }
    #[test] fn the_predictor_finishes_a_word_from_the_line_before_it() {
        let mut e = bare_engine(&[], &[]);
        // the frequency list cannot extend "pars"; only the model can
        e.en_comp = Some(Completer::new([("helmet".to_string(), 9.0)]));
        e.opts.complete = CompleteMode::Pause; e.opts.slips = None;
        predictor(&mut e, "line", &["please", "refactor", "the", "parser"]);
        // the line the user finished is what the model learns from
        typ(&mut e, "please refactor the parser\n");
        typ(&mut e, "pars");
        assert!(until_idle(&mut e, |e| labels(e).iter().any(|l| l == "parser")), "no predicted row: {:?}", labels(&e));
    }
    #[test] fn an_accepted_prediction_is_edited_as_the_word() {
        let mut e = bare_engine(&[], &[]);
        e.en_comp = Some(Completer::new([("helmet".to_string(), 9.0)]));
        e.opts.complete = CompleteMode::Typing; e.opts.slips = None;
        predictor(&mut e, "accept", &["please", "refactor", "the", "parser"]);
        typ(&mut e, "please refactor the parser\n");
        typ(&mut e, "pars");
        assert!(until_idle(&mut e, |e| labels(e).iter().any(|l| l == "parser")), "no predicted row: {:?}", labels(&e));
        // the row carries the whole word as its insert, and Tab applies it exactly like a completion: what the keys
        // typed stays on screen and the rest goes after it
        assert!(e.bubble.iter().any(|b| matches!(&b.kind, BubbleKind::Complete { typed, insert } if typed == "pars" && insert == "parser")), "{:?}", labels(&e));
        let o = tab(&mut e);
        assert!(o.actions.iter().any(|a| matches!(a, Action::Replace { delete: 0, insert, .. } if insert.as_str() == "er")), "{:?}", o.actions);
    }
    #[test] fn a_thai_prediction_keeps_the_run_in_front_of_its_word() {
        let mut e = bare_engine(&["สวัสดี", "ครับ"], &[]);
        e.thai_comp = Some(Completer::new([("สวัสดี".to_string(), 9.0), ("ครับ".to_string(), 9.0)]));
        e.opts.complete = CompleteMode::Pause; e.opts.slips = None; e.opts.live = false;
        predictor(&mut e, "thai", &["สวัสดี", "ครับ"]);
        typ_th(&mut e, "สวัสดีครับ");
        typ(&mut e, "\n");
        // the second visit: the run's last word is what the model finishes, and the row keeps the head in front of it
        e.set_thai_active(true);
        typ(&mut e, &keys("สวัสดีครั"));
        assert!(until_idle(&mut e, |e| e.bubble.iter().any(|b| matches!(&b.kind, BubbleKind::Complete { insert, .. } if insert == "สวัสดีครับ"))), "{:?}", labels(&e));
    }
    #[test] fn the_predictor_is_off_without_its_setting() {
        let mut e = bare_engine(&[], &[]);
        e.en_comp = Some(Completer::new([("helmet".to_string(), 9.0)]));
        e.opts.complete = CompleteMode::Pause; e.opts.slips = None;
        predictor(&mut e, "off", &["please", "refactor", "the", "parser"]);
        typ(&mut e, "please refactor the parser\n");
        e.opts.predict = false;
        typ(&mut e, "pars");
        // the model may still answer; the setting keeps every row off the bubble
        for _ in 0..40 { e.on_idle(Instant::now() + Duration::from_millis(600)); std::thread::sleep(Duration::from_millis(5)); }
        assert_eq!(labels(&e), Vec::<String>::new(), "a row with the setting off");
    }
    /// What one predicted word costs the predictor's thread, on a model bootstrapped from this build's word lists:
    /// `cargo test --release -p typethai-desktop predict_cost -- --ignored --nocapture`.
    #[test] #[ignore] fn predict_cost() {
        let Some(mut m) = model() else { return };
        let dir = std::env::temp_dir().join(format!("typethai-predict-cost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let words: Vec<String> = m.english_freq.keys().cloned().collect();
        let oracle = Arc::new(typethai_core::predict::KnownWords::new(
            Arc::new(typethai_core::dict::Dict::new(m.thai_words.iter().cloned())),
            Arc::new(words.iter().cloned().collect())));
        let config = typethai_predict::Config { state_dir: dir.clone(), oracle: Some(oracle), show_threshold: None };
        use typethai_predict::Predictor as _;
        let mut ngram = typethai_predict::ngram::NgramPredictor::open(&config, typethai_predict::Params::default()).expect("open");
        // 5,000 lines of 8 words: a heavy typist's history
        let lines: Vec<String> = (0..5000).map(|i| (0..8).map(|j| words[(i * 8 + j * 7919) % words.len()].clone()).collect::<Vec<_>>().join(" ")).collect();
        let t = Instant::now();
        let mut observes: Vec<u128> = lines.iter().map(|l| { let t = Instant::now(); ngram.observe(l); t.elapsed().as_nanos() }).collect();
        let boot = t.elapsed().as_secs_f64();
        observes.sort_unstable();
        let heap = ngram.heap_bytes();
        let before = "the quick brown fox jumps over the lazy dog and then ";
        for len in [1usize, 2, 4] {
            let prefixes: Vec<String> = words.iter().filter(|w| w.len() > len + 2).take(64).map(|w| w[..len].to_string()).collect();
            let mut ns: Vec<u128> = vec![];
            for p in &prefixes {
                for _ in 0..200 {
                    let t = Instant::now();
                    std::hint::black_box(ngram.complete(&typethai_predict::Query { before, prefix: p }));
                    ns.push(t.elapsed().as_nanos());
                }
            }
            ns.sort_unstable();
            println!("complete {len}-letter prefix: p50 {:.1} us p99 {:.1} us (n={})", ns[ns.len() / 2] as f64 / 1e3, ns[ns.len() * 99 / 100] as f64 / 1e3, ns.len());
        }
        println!("observe: p50 {:.0} us p99 {:.0} us; 5,000 lines bootstrapped in {boot:.2} s; heap {} MiB", observes[observes.len() / 2] as f64 / 1e3, observes[observes.len() * 99 / 100] as f64 / 1e3, heap / (1024 * 1024));
        let t = Instant::now(); ngram.persist().expect("persist"); let write = t.elapsed().as_secs_f64() * 1e3;
        let size = std::fs::metadata(dir.join(typethai_predict::ngram::SNAPSHOT_FILE)).map_or(0, |m| m.len());
        let t = Instant::now();
        let _ = typethai_predict::ngram::NgramPredictor::open(&config, typethai_predict::Params::default()).expect("restore");
        println!("snapshot: {} KB, persist {write:.0} ms, restore {:.0} ms", size / 1024, t.elapsed().as_secs_f64() * 1e3);
        let _ = std::fs::remove_dir_all(&dir);
        // what the query, the adapter and the projection add to a keystroke on the key thread (the model answers on its
        // own thread, so this is the whole cost the user can feel)
        let Some(mut e) = engine() else { return };
        e.load_completers(&mut m);
        let run = keys("วันนี้ฉันอยากไปกินข้าวที่ร้านอาห");
        for on in [false, true] {
            e.predict = None;
            if on { predictor(&mut e, "keystroke", &[]) }
            e.opts.complete = CompleteMode::Typing; e.opts.predict = on;
            let mut ns: Vec<u128> = vec![];
            for _ in 0..50 { e.reset(); e.set_thai_active(true); for c in run.chars() { let t = Instant::now(); std::hint::black_box(e.on_key(Key::Char(c))); ns.push(t.elapsed().as_nanos()) } }
            ns.sort_unstable();
            println!("on_key over a 30-char Thai run, predictor {on}: p50 {:.1} us p99 {:.1} us (n={})", ns[ns.len() / 2] as f64 / 1e3, ns[ns.len() * 99 / 100] as f64 / 1e3, ns.len());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("typethai-predict-{}-keystroke", std::process::id())));
    }
    #[test] fn the_preview_shows_when_focus_lands_in_a_text_field() {
        let Some(mut e) = engine() else { return };
        assert_eq!(e.on_focus(true), vec![Action::Badge(true)]);
        assert_eq!(e.on_focus(true), vec![], "one show while it is up");
        assert_eq!(e.on_focus(false), vec![Action::Badge(false)]);
        assert_eq!(e.on_focus(false), vec![], "already down");
        e.opts.input_badge = false;
        assert_eq!(e.on_focus(true), vec![], "setting off");
        assert_eq!(e.on_focus(false), vec![]);
    }
    #[test] fn a_key_takes_the_preview_down_and_a_long_pause_brings_it_back() {
        let Some(mut e) = engine() else { return };
        e.on_focus(true);
        let t = Instant::now();
        assert_eq!(e.on_keystroke(t), vec![Action::Badge(false)]);
        assert_eq!(e.on_keystroke(t), vec![], "already down");
        let idle = e.opts.badge_idle.expect("the default pause");
        assert_eq!(e.on_idle(t + idle - Duration::from_millis(10)), vec![], "not yet");
        assert_eq!(e.on_idle(t + idle), vec![Action::Badge(true)]);
        assert_eq!(e.on_idle(t + idle * 2), vec![], "up once per pause");
        assert_eq!(e.on_keystroke(t + idle * 2), vec![Action::Badge(false)], "typing again");
        assert_eq!(e.on_idle(t + idle * 4), vec![Action::Badge(true)], "and back after the next pause");
    }
    #[test] fn the_model_offers_the_next_word_where_the_pairs_have_nothing() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        predictor(&mut e, "next-word", &["zorble", "quux"]);
        // a line the user finished: the model learns it when the line ends
        typ(&mut e, "zorble quux"); e.on_key(Key::Boundary(Boundary::Enter));
        // then its word follows "zorble ", where the built-in pairs know nothing
        typ(&mut e, "zorble ");
        let got = until_idle(&mut e, |e| labels(e).iter().any(|l| l == "quux"));
        assert!(got, "the model's word: {:?}", labels(&e));
        // and it keeps the pairs' rows when they have any (they lead, the model's word is the last one)
        assert!(labels(&e).len() <= 3);
        // with the model off, only the pairs would be left: none for a word they do not know
        e.opts.predict = false;
        e.on_key(Key::Boundary(Boundary::Space));
        assert!(until_idle(&mut e, |e| !e.bubble_visible()));
        assert_eq!(labels(&e), Vec::<String>::new());
    }
    #[test] fn off_keeps_the_preview_to_the_focus_that_showed_it() {
        let Some(mut e) = engine() else { return };
        assert!(e.apply_settings(SettingsCmd::SetBadgeIdle(None)), "Off");
        assert_eq!(e.settings_snapshot().badge_idle, None);
        assert_eq!(e.on_focus(true), vec![Action::Badge(true)], "focus still shows it");
        let t = Instant::now();
        e.on_keystroke(t);
        assert_eq!(e.on_idle(t + Duration::from_secs(600)), vec![], "and a pause never brings it back");
        assert_eq!(e.on_focus(true), vec![Action::Badge(true)], "a new focus does");
    }
    #[test] fn the_preview_follows_its_pause_setting() {
        let Some(mut e) = engine() else { return };
        let five = Some(Duration::from_millis(5000));
        assert!(e.apply_settings(SettingsCmd::SetBadgeIdle(five)));
        assert!(!e.apply_settings(SettingsCmd::SetBadgeIdle(five)), "same delay is not a change");
        assert_eq!(e.settings_snapshot().badge_idle, five);
        e.on_focus(true);
        let t = Instant::now();
        e.on_keystroke(t);
        assert_eq!(e.on_idle(t + Duration::from_millis(2000)), vec![], "the default would have shown it");
        assert_eq!(e.on_idle(t + Duration::from_millis(5000)), vec![Action::Badge(true)]);
    }
    #[test] fn no_preview_outside_a_text_field_or_while_paused() {
        let Some(mut e) = engine() else { return };
        let t = Instant::now();
        assert_eq!(e.on_keystroke(t), vec![], "outside a text field there is nothing to take down");
        assert_eq!(e.on_idle(t + e.opts.badge_idle.expect("the default pause") * 2), vec![], "and none to bring back");
        e.paused = true;
        assert_eq!(e.on_focus(true), vec![], "paused: no preview");
        assert_eq!(e.on_idle(t + e.opts.badge_idle.expect("the default pause") * 2), vec![]);
        e.paused = false;
        assert_eq!(e.on_focus(true), vec![Action::Badge(true)], "and it is not lost: focus is still there");
    }
    #[test] fn a_look_after_a_key_leaves_the_preview_down() {
        let Some(mut e) = engine() else { return };
        e.on_focus(true);
        assert_eq!(e.set_field(false), vec![Action::Badge(false)], "focus moved off a text field: it goes");
        assert!(e.badge_shown() == false);
        assert_eq!(e.set_field(true), vec![], "a text field, but the user just typed: nothing on the spot");
        assert!(!e.badge_shown());
        let t = Instant::now();
        e.on_keystroke(t);
        assert_eq!(e.on_idle(t + e.opts.badge_idle.expect("the default pause")), vec![Action::Badge(true)], "and the pause after it shows it");
    }
    #[test] fn a_pause_takes_the_preview_down_and_the_setting_too() {
        let Some(mut e) = engine() else { return };
        e.on_focus(true);
        let o = e.on_key(Key::Hotkey(Hotkey::TogglePause));
        assert!(o.actions.contains(&Action::Badge(false)), "pausing takes it down");
        assert!(!e.on_key(Key::Hotkey(Hotkey::TogglePause)).actions.contains(&Action::Badge(true)), "unpausing does not show one by itself");
        e.on_focus(true);
        assert!(e.apply_settings(SettingsCmd::SetInputBadge(false)));
        assert_eq!(e.hide_badge(), vec![Action::Badge(false)], "the setting going off takes it down");
        assert_eq!(e.hide_badge(), vec![], "already down");
    }
    #[test] fn the_bubble_has_the_window_and_the_preview_returns_when_it_goes() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.on_focus(true);
        let t = Instant::now();
        e.on_keystroke(t);
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["help", "hello", "helmet"], "the bubble is up");
        assert_eq!(e.on_idle(t + e.opts.badge_idle.expect("the default pause") * 2), vec![], "no preview over the bubble");
        let o = e.on_key(Key::Esc); assert_eq!(o.actions, vec![Action::HideBubble]);
        assert_eq!(e.on_idle(t + e.opts.badge_idle.expect("the default pause") * 3), vec![Action::Badge(true)], "the bubble is gone");
    }
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
    /// Eight equally likely extensions: the best one is an eighth of the offer, under the 0.15 completion floor.
    fn weak_completer() -> Completer {
        Completer::new(["held", "helm", "helmet", "helper", "helps", "help", "hello", "helms"].map(|w| (w.to_string(), 10.0)))
    }
    #[test] fn a_weak_pause_completion_offer_is_kept_off_the_bubble() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.en_comp = Some(weak_completer());
        typ(&mut e, "hel");
        assert_eq!(idle(&mut e, 600), vec![], "the gate emptied the offer on an empty bubble");
        assert!(!e.bubble_visible());
        assert_eq!(e.rank.pending(), 1, "the suppressed offer is recorded, not silently dropped");
    }
    #[test] fn a_strong_pause_completion_offer_is_kept() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        typ(&mut e, "hel");
        idle(&mut e, 600);
        assert_eq!(labels(&e), ["help", "hello", "helmet"], "one word is two thirds of the offer");
        assert_eq!(e.rank.pending(), 0, "a shown offer stands until it closes");
    }
    #[test] fn the_gate_keeps_a_row_with_no_floor() {
        let Some(mut e) = engine() else { return };
        let convert = BubbleItem { label: "สวัสดี".into(), kind: BubbleKind::Convert { typed: "l;ylfu".into(), thai: "สวัสดี".into() } };
        let mut a = vec![];
        e.show_candidates(vec![Candidate::bare(convert.clone(), Src::Convert)], &mut a);
        assert_eq!(a, vec![Action::ShowBubble(vec![convert])]);
    }
    #[test] fn indices_address_the_rows_left_after_the_gate() {
        let Some(mut e) = engine() else { return };
        let complete = |label: &str, conf: f64| Candidate::scored(BubbleItem { label: label.into(), kind: BubbleKind::Complete { typed: "hel".into(), insert: label.into() } }, Src::Completion, conf);
        let convert = BubbleItem { label: "สวัสดี".into(), kind: BubbleKind::Convert { typed: "l;ylfu".into(), thai: "สวัสดี".into() } };
        e.word = "hel".into();
        let mut a = vec![];
        e.show_candidates(vec![complete("help", 0.6), complete("helmet", 0.1), Candidate::bare(convert, Src::Convert)], &mut a);
        assert_eq!(labels(&e), ["สวัสดี", "help"], "the weak completion is gone, the rest keep display order");
        assert_eq!(e.shown.iter().map(|c| c.item.clone()).collect::<Vec<_>>(), e.bubble, "shown is the bubble, score for score");
        assert_eq!(e.on_key(Key::Hotkey(Hotkey::Accept(2))), Outcome::pass(), "no row at the old index");
        assert!(e.bubble_visible());
        let o = e.on_key(Key::Hotkey(Hotkey::Accept(1)));
        assert_eq!(o.actions[0], Action::Replace { delete: 0, insert: "p".into(), then: None }, "row 1 is help");
        assert!(!e.bubble_visible());
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
        fn guesses(&self, _: &str, _: &str) -> Option<Vec<String>> { None }
        fn completions(&self, _: &str, p: &str) -> Vec<String> { self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst); if p.starts_with("hel") { vec!["helium".into()] } else { vec![] } }
    }
    #[test] fn a_key_keeps_spell_checker_rows_that_still_fit_without_calling_it() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        e.en_comp = Some(Completer::new([("help".to_string(), 30.0)])); e.speller = Some(Box::new(CountingSpeller(calls.clone())));
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["helium", "help"]);
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
    impl Speller for FixingSpeller { fn guesses(&self, _: &str, w: &str) -> Option<Vec<String>> { matches!(w, "ths" | "thiss").then(|| vec!["this".into()]) } }
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
        fn guesses(&self, _: &str, _: &str) -> Option<Vec<String>> { None }
        fn completions(&self, _: &str, p: &str) -> Vec<String> { if p == "hel" { vec!["helium".into()] } else { vec![] } }
    }
    #[test] fn the_spell_checker_tops_up_only_after_a_pause() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Typing);
        e.en_comp = Some(Completer::new([("help".to_string(), 30.0)])); e.speller = Some(Box::new(CompletingSpeller));
        typ(&mut e, "hel"); assert_eq!(labels(&e), ["help"], "no spell checker on a keystroke");
        e.reset(); e.opts.complete = CompleteMode::Pause;
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["helium", "help"], "its best word leads");
    }
    /// Completes `hel` to `helpless` and fixes `ths`, and keeps the line it was given in front of each word.
    struct ContextSpeller(Arc<std::sync::Mutex<Vec<String>>>);
    impl Speller for ContextSpeller {
        fn guesses(&self, before: &str, w: &str) -> Option<Vec<String>> { self.0.lock().unwrap().push(before.into()); (w == "ths").then(|| vec!["this".into()]) }
        fn completions(&self, before: &str, p: &str) -> Vec<String> { self.0.lock().unwrap().push(before.into()); if p == "hel" { vec!["helpless".into(), "helper".into()] } else { vec![] } }
    }
    #[test] fn the_spell_checker_reads_the_line_in_front_of_the_word() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause); e.opts.live = false;
        let seen = Arc::new(std::sync::Mutex::new(vec![]));
        e.speller = Some(Box::new(ContextSpeller(seen.clone())));
        typ(&mut e, "can you hel"); idle(&mut e, 600);
        assert_eq!(seen.lock().unwrap().last().unwrap(), "can you ");
        e.on_key(Key::Boundary(Boundary::Space)); typ(&mut e, "ths ");
        assert_eq!(seen.lock().unwrap().last().unwrap(), "can you hel ", "a finished word's guesses");
        e.on_key(Key::Boundary(Boundary::Enter)); typ(&mut e, "hel"); idle(&mut e, 600);
        assert_eq!(seen.lock().unwrap().last().unwrap(), "", "a new line starts over");
    }
    #[test] fn a_key_keeps_the_spell_checkers_row_first() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.en_comp = Some(Completer::new([("help".to_string(), 30.0), ("helper".to_string(), 20.0), ("helpful".to_string(), 10.0)]));
        e.speller = Some(Box::new(ContextSpeller(Arc::default())));
        typ(&mut e, "hel"); idle(&mut e, 600); assert_eq!(labels(&e), ["helpless", "help", "helper"]);
        typ(&mut e, "p"); assert_eq!(labels(&e), ["helpless", "helper", "helpful"], "the model's rows follow the new prefix");
        typ(&mut e, "l"); assert_eq!(labels(&e), ["helpless"]);
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
            let t = Instant::now(); let g = e.speller.as_ref().and_then(|s| s.guesses("", w)).map_or(0, |g| g.len());
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
    impl Speller for CandelSpeller { fn guesses(&self, _: &str, w: &str) -> Option<Vec<String>> { (w == "candel").then(|| ["cancel", "candle", "Handel"].map(String::from).to_vec()) } }
    #[test] fn a_slipped_key_still_reaches_the_word_meant() {
        let (Some(mut m), Some(mut e)) = (model(), engine_typo()) else { return };
        e.opts.complete = CompleteMode::Pause; e.load_completers(&mut m); e.speller = Some(Box::new(CandelSpeller));
        typ(&mut e, "candel"); idle(&mut e, 600);
        assert!(!e.bubble_visible(), "the slipped reading is 0.085 of the offer, under the completion floor");
        // the slip machinery itself, with the trigger out of the way
        e.opts.floors.completion = 0.0;
        e.reset(); typ(&mut e, "candel"); idle(&mut e, 600);
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
    #[test] fn the_rank_log_keeps_the_rows_offered_and_the_row_applied() {
        let Some(mut e) = engine_typo() else { return };
        e.opts.pick_ctrl = true;
        // a typo of the word just finished: the corrector's fixes, in its order
        typ(&mut e, &format!("{} ", keys("โรงเรยีน")));
        let rows = e.bubble.len();
        assert!(rows >= 2, "more than one row to choose between");
        e.on_key(Key::PickRow(1));
        let offers = e.rank.take_pending();
        assert_eq!(offers.len(), 1, "one offer, one line");
        assert_eq!((offers[0].rows.len(), offers[0].w.as_str()), (rows, keys("โรงเรยีน").as_str()));
        let pick = offers[0].pick.clone().expect("a row was applied");
        assert_eq!((pick.i, pick.how), (1, How::Digit), "the second row, picked with a number");
    }
    #[test] fn one_word_is_one_offer_however_many_keys_refresh_it() {
        let Some(mut e) = engine() else { return };
        typ(&mut e, "l;ylfu");
        assert!(e.bubble_visible(), "the live row came up while the word was still being typed");
        e.on_key(Key::Esc);
        assert_eq!(e.rank.take_pending().len(), 1, "five keystrokes, one word: one line");
    }
    #[test] fn a_dismissed_bubble_is_logged_with_no_pick() {
        let Some(mut e) = engine_typo() else { return };
        typ(&mut e, &format!("{} ", keys("โรงเรยีน")));
        assert!(!e.bubble.is_empty());
        e.on_key(Key::Esc);
        let offers = e.rank.take_pending();
        assert_eq!((offers.len(), offers[0].pick.clone()), (1, None), "nothing was applied, and that is the measurement");
    }
    #[test] fn nothing_is_logged_while_learn_from_my_typing_is_off() {
        let Some(mut e) = engine_typo() else { return };
        e.opts.learn_words = false;
        typ(&mut e, &format!("{} ", keys("โรงเรยีน")));
        assert!(!e.bubble.is_empty(), "the bubble still offers it");
        e.on_key(Key::Hotkey(Hotkey::Accept(0)));
        assert!(e.rank.is_empty(), "the log goes with the setting");
    }
    #[test] fn resetting_learning_clears_the_rank_log() {
        let Some(mut e) = engine_typo() else { return };
        typ(&mut e, &format!("{} ", keys("โรงเรยีน")));
        e.on_key(Key::Hotkey(Hotkey::Accept(0)));
        assert_eq!(e.rank.pending(), 1);
        assert!(e.apply_settings(SettingsCmd::ResetLearning));
        assert_eq!((e.rank.pending(), e.rank.cleared, e.rank.is_empty()), (0, true, true), "and the file goes with it");
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
    #[test] fn english_on_the_thai_layout_below_the_live_bar_is_offered_after_a_pause() {
        let Some(mut e) = engine() else { return };
        completers(&mut e, CompleteMode::Pause);
        e.thai_comp = Some(Completer::new([("ฟิวเจอร์".to_string(), 9.0), ("ฟิต".to_string(), 5.0)]));
        e.en_comp = Some(Completer::new([("stab".to_string(), 3.0), ("stable".to_string(), 9.0)]));
        typ_th(&mut e, "หะฟิ");
        assert!(!e.bubble_visible(), "stab scores under the live bar while typing");
        idle(&mut e, 600);
        assert_eq!(labels(&e), ["stab"], "the English word, not ฟิวเจอร์");
        assert_eq!(replaced(&[tab(&mut e)]), ["stab"]);
        e.reset(); typ_th(&mut e, "หะฟิ ");
        assert_eq!(labels(&e), ["stab"], "offered after Space too");
        e.reset(); typ_th(&mut e, "ฟิ"); idle(&mut e, 600);
        assert_eq!(labels(&e), ["ฟิวเจอร์", "ฟิต"], "Thai still completes");
        e.reset(); e.en_comp = Some(Completer::new([("stable".to_string(), 9.0)]));
        typ_th(&mut e, "หะฟิ"); idle(&mut e, 600);
        assert!(!labels(&e).contains(&"stab".to_string()), "only words the English list knows");
    }
}
