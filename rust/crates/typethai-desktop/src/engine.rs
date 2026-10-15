//! Platform-independent typing engine: per-word key buffer -> decisions -> actions for the platform layer.
//! The platform layer feeds `Key`s (already mapped to the US-QWERTY character the key would produce), and executes the
//! returned `Action`s (delete N chars + inject text, show/hide the suggestion bubble). Fully unit-testable on Linux.
use crate::prefs::{normalize_word, SettingsCmd, SettingsSnap};
use std::collections::BTreeSet;
use typethai_core::{reverse::Reverse, corrector::Corrector, detector::Detector, keymap::en_to_th, personal::{Feedback, PersonalModel}, TokenDecision};

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
pub enum Hotkey { Undo, ForceConvert, Guess, Accept(usize), Dismiss, TogglePause }
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
    /// Tab (no modifiers): applies the selected bubble item while the bubble is visible, otherwise a word boundary.
    Tab,
    /// Shift+Up / Shift+Down: move the bubble selection while it is visible, otherwise plain navigation (text selection).
    SelectPrev, SelectNext,
    /// Esc: dismisses a visible bubble (swallowed), otherwise navigation.
    Esc,
}
/// Which kind of OS input source fits the text just inserted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputLang { Thai, Latin }
pub fn lang_of(s: &str) -> InputLang { if s.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c)) { InputLang::Thai } else { InputLang::Latin } }

/// Bubble selection: index into the visible items, wrapping. Pure state machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection { pub len: usize, pub idx: usize }
impl Selection {
    pub fn new(len: usize) -> Self { Selection { len, idx: 0 } }
    pub fn next(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + 1) % self.len } self.idx }
    pub fn prev(&mut self) -> usize { if self.len > 0 { self.idx = (self.idx + self.len - 1) % self.len } self.idx }
}
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Delete `delete` chars before the caret (backspaces), then type `insert`; then re-send the swallowed boundary key.
    Replace { delete: usize, insert: String, then: Option<Boundary> },
    ShowBubble(Vec<BubbleItem>),
    HideBubble,
    /// Move the bubble highlight to this row.
    Select(usize),
    /// Switch the OS input source to fit the text just inserted (platform: no-op if already that kind).
    SwitchInput(InputLang),
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
    Guess { k: usize, typed: Option<String>, targets: Vec<String> } }
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
    extra: String }
impl Committed {
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
}
impl Default for Options { fn default() -> Self { Options { auto_convert: false, switch_input: true, suggest_band: 0.5, typo_suggestions: true, context_words: 8,
    live: true, live_show: LIVE_SHOW, live_min_len: 3, live_stable: 1, rev_auto: REV_AUTO, rev_live: REV_LIVE, rev_prev: REV_PREV } } }
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
}

impl Engine {
    pub fn new(det: Detector, personal: PersonalModel, corrector: Option<Corrector>, opts: Options) -> Self {
        Engine { rev: None, thai_active: false, det, personal, corrector, opts, paused: false, word: String::new(), tab_pending: false, line: vec![], line_start: 0, bubble: vec![], sel: Selection::default(), live_streak: 0, live: false, phrase: 0, events_since_save: 0, never: BTreeSet::new(), speller: None }
    }
    pub fn buffer(&self) -> &str { &self.word }
    pub fn thai_active(&self) -> bool { self.thai_active }
    /// Platform reports the active input source before each key. A change starts a new word (the line and a visible
    /// bubble are kept, so undo / Tab still work right after a conversion switched the layout).
    pub fn set_thai_active(&mut self, on: bool) { if on != self.thai_active { self.thai_active = on; self.word.clear(); self.live_streak = 0; } }
    /// What `typed` keys put on screen in the current mode, and what a conversion would replace it with.
    fn screen_of(&self, typed: &str) -> String { if self.thai_active { en_to_th(typed) } else { typed.to_string() } }
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
    /// field check): the bubble controls, while the bubble is up.
    pub fn is_bubble_key(&self, k: &Key) -> bool { self.bubble_visible() && matches!(k, Key::Tab | Key::SelectPrev | Key::SelectNext | Key::Esc) }
    /// Focus change, mouse click, secure field, layout change: forget everything.
    pub fn reset(&mut self) -> Vec<Action> {
        self.word.clear(); self.tab_pending = false; self.line.clear(); self.line_start = 0; self.live_streak = 0; self.live = false;
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
    /// English spelling suggestions for a finished word (`ths` -> this). Punctuation around the word is kept.
    fn en_items(&self, typed: &str) -> Vec<BubbleItem> {
        let Some(sp) = &self.speller else { return vec![] };
        if !self.opts.typo_suggestions { return vec![] }
        let core = typed.trim_matches(|c: char| !c.is_ascii_alphabetic());
        if core.len() < 2 || !core.chars().all(|c| c.is_ascii_alphabetic() || c == '\'') { return vec![] }
        let at = typed.find(core).unwrap_or(0);
        let (pre, post) = (&typed[..at], &typed[at + core.len()..]);
        sp.guesses(core).unwrap_or_default().into_iter().filter(|g| g != core).take(3)
            .map(|g| { let word = format!("{pre}{g}{post}"); BubbleItem { label: word.clone(), kind: BubbleKind::Typo { word } } }).collect()
    }
    fn typo_items(&self, thai: &str) -> Vec<BubbleItem> {
        let Some(c) = &self.corrector else { return vec![] };
        if !self.opts.typo_suggestions { return vec![] }
        let r = c.suggest(thai, 3, &self.det.thai);
        if r.in_dict && r.apply.is_none() { return vec![] }
        let mut out: Vec<BubbleItem> = r.apply.iter().filter(|a| a.as_str() != thai).map(|a| BubbleItem { label: a.clone(), kind: BubbleKind::Typo { word: a.clone() } }).collect();
        for s in r.suggestions { if out.len() >= 3 { break } if !out.iter().any(|o| o.label == s.word) { out.push(BubbleItem { label: s.word.clone(), kind: BubbleKind::Typo { word: s.word } }) } }
        out
    }
    pub fn live_visible(&self) -> bool { self.live && self.bubble_visible() }
    fn show(&mut self, items: Vec<BubbleItem>, acts: &mut Vec<Action>) {
        self.live = matches!(items.first(), Some(BubbleItem { kind: BubbleKind::Live { .. }, .. }));
        if items.is_empty() { self.phrase = 0; if !self.bubble.is_empty() { self.bubble.clear(); acts.push(Action::HideBubble) } }
        else { self.sel = Selection::new(items.len()); self.bubble = items.clone(); acts.push(Action::ShowBubble(items)) }
    }
    fn feedback(&mut self, typed: &str, kind: Feedback) {
        let ds = self.decisions(typed); let i = ds.len() - 1; self.personal.feedback(&ds, i, kind); self.events_since_save += 1;
    }

    pub fn on_key(&mut self, k: Key) -> Outcome {
        let mut o = self.on_key_inner(k);
        // after inserting text, switch the OS input source to match it (Thai after a conversion, Latin after an undo)
        if self.opts.switch_input {
            if let Some(Action::Replace { insert, .. }) = o.actions.iter().rev().find(|a| matches!(a, Action::Replace { .. })) {
                let l = lang_of(insert); o.actions.push(Action::SwitchInput(l));
            }
        }
        o
    }
    fn on_key_inner(&mut self, k: Key) -> Outcome {
        if let Key::Hotkey(Hotkey::TogglePause) = k { self.paused = !self.paused; let a = self.reset(); return Outcome { swallow: true, actions: a } }
        if self.paused { return Outcome::pass() }
        match k {
            Key::Char(c) => {
                self.tab_pending = false;
                // a new word starts: the bubble was about the previous word and can no longer be applied
                let mut a = vec![]; if self.word.is_empty() { self.show(vec![], &mut a); self.live_streak = 0; }
                else if matches!(self.bubble.first(), Some(BubbleItem { kind: BubbleKind::Guess { .. }, .. })) { self.show(vec![], &mut a); }
                self.word.push(c);
                self.live_update(&mut a);
                Outcome { swallow: false, actions: a }
            }
            Key::Tab => if self.bubble_visible() {
                self.tab_pending = false;
                let mut o = self.on_hotkey(Hotkey::Accept(self.sel.idx)); o.swallow = true; o
            } else if let Some(item) = self.tab_pending.then(|| self.guess()).flatten() {
                // second Tab in a row: show the proposal (a third Tab applies it); with nothing to propose it is a tab
                self.tab_pending = false;
                let mut a = vec![]; self.show(vec![item], &mut a);
                Outcome { swallow: true, actions: a }
            } else {
                let o = self.on_boundary(Boundary::Tab);
                self.tab_pending = true;
                o
            },
            Key::SelectPrev | Key::SelectNext => if self.bubble_visible() {
                let i = if k == Key::SelectNext { self.sel.next() } else { self.sel.prev() };
                Outcome { swallow: true, actions: vec![Action::Select(i)] }
            } else { Outcome { swallow: false, actions: self.reset() } },
            Key::Esc => if self.bubble_visible() { let mut a = vec![]; self.show(vec![], &mut a); Outcome { swallow: true, actions: a } }
                else { Outcome { swallow: false, actions: self.reset() } },
            Key::Backspace => { self.tab_pending = false; self.on_backspace() }
            Key::WordDelete => self.on_word_delete(),
            // Cmd+Backspace deletes the whole line: nothing of it is left to offer
            Key::LineDelete => { self.tab_pending = false; Outcome { swallow: false, actions: self.reset() } }
            Key::Nav => { self.tab_pending = false; Outcome { swallow: false, actions: self.reset() } }
            Key::Boundary(b) => self.on_boundary(b),
            Key::Hotkey(h) => self.on_hotkey(h),
        }
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
        }
    }
    pub fn settings_snapshot(&self) -> SettingsSnap {
        SettingsSnap { never: self.never.iter().cloned().collect(), learned: self.personal.learned() }
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
    fn reoffer(&mut self, was_shown: bool, phrase: usize, acts: &mut Vec<Action>) {
        if !self.word.is_empty() {
            if phrase > 1 { self.show_live(phrase - 1, acts) }                    // rest of a reverse phrase: keep it
            else if was_shown && self.live_confident() { self.live_streak = self.opts.live_stable; let prev = self.live_prev(); self.show_live(prev, acts) }
            else if was_shown { self.live_streak = 0; self.show(vec![], acts) }
            else { self.live_update(acts) }                                      // nothing shown: normal stability rules
        } else if phrase > 0 && was_shown {
            // buffer gone: the words still left on the line (reverse phrase) stay offered
            self.offer_committed(phrase, acts);
        } else { self.show(vec![], acts) }
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
    /// Backspace: edit the buffer; at a word's end (only its boundary/extra spaces left) step back into that word.
    fn on_backspace(&mut self) -> Outcome {
        let mut a = vec![]; let shown = self.bubble_visible(); let phrase = self.phrase;
        if self.word.pop().is_some() {
            // word fully deleted: one fewer word left in the phrase
            let phrase = if self.word.is_empty() { phrase.saturating_sub(1) } else { phrase };
            self.reoffer(shown, phrase, &mut a);
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
                self.reoffer(shown, phrase, &mut a);
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
        self.live_streak = 0;
        let typed = std::mem::take(&mut self.word);
        let mut acts = vec![]; let mut swallow = false;
        if typed.is_empty() {
            // extra spaces / Enters after a word: pass through; the bubble for the last word stays applicable
            if let Some(c) = self.line.last_mut() { c.extra.push_str(b.text()); }
            if b == Boundary::Enter { self.new_line(); }
            return Outcome::pass()
        }
        if self.thai_active { return self.on_boundary_rev(typed, b) }
        let (p, ds) = self.prob(&typed);
        let d = ds.last().unwrap();
        let thai = d.candidate.clone();
        let mut converted = None;
        if self.blocked(&typed) {
            self.show(vec![], &mut acts);
            self.line.push(Committed { orig: typed.clone(), typed, converted: None, boundary: Some(b), rev: false, with_prev: 0, extra: String::new() });
            if b == Boundary::Enter { self.new_line(); }
            return Outcome { swallow: false, actions: acts };
        }
        if self.opts.auto_convert && p > self.personal.base.threshold && d.valid {
            // legacy (--auto-convert): replace at the boundary
            swallow = true; converted = Some(thai.clone());
            acts.push(Action::Replace { delete: typed.chars().count(), insert: thai.clone(), then: Some(b) });
            let mut items = vec![BubbleItem { label: typed.clone(), kind: BubbleKind::UndoConversion }];
            items.extend(self.typo_items(&thai));
            self.show(items, &mut acts);
        } else if d.valid && p > self.band(&thai, self.opts.suggest_band) {
            // default: the boundary passes through untouched; the bubble offers the conversion (Tab / hotkey applies)
            let mut items = vec![BubbleItem { label: thai.clone(), kind: BubbleKind::Convert { typed: typed.clone(), thai: thai.clone() } }];
            // the best English spelling stays in reach right under the Thai reading
            let en = self.en_items(&typed);
            items.extend(en.first().cloned());
            for t in self.typo_items(&thai) { if t.label != thai { items.push(t) } }
            items.extend(en.into_iter().skip(1));
            items.truncate(3);
            self.show(items, &mut acts); self.phrase = 1;
        } else {
            // left as English: offer spelling corrections
            let en = self.en_items(&typed);
            self.show(en, &mut acts); self.phrase = 0;
        }
        self.line.push(Committed { orig: typed.clone(), typed, converted, boundary: Some(b), rev: false, with_prev: 0, extra: String::new() });
        if b == Boundary::Enter { self.new_line(); }
        Outcome { swallow, actions: acts }
    }
    /// Reverse direction at a word boundary: flip the Thai reading back to the typed English only at very high confidence.
    fn on_boundary_rev(&mut self, typed: String, b: Boundary) -> Outcome {
        let mut acts = vec![]; let mut swallow = false; let mut converted = None;
        let thai = en_to_th(&typed);
        if self.blocked(&typed) {
            self.show(vec![], &mut acts);
            self.line.push(Committed { typed, orig: thai, converted: None, boundary: Some(b), rev: true, with_prev: 0, extra: String::new() });
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
                let (mut del, mut ins, mut was) = (thai.chars().count(), String::new(), String::new());
                for c in &self.line[start..] { del += c.orig.chars().count() + c.after().chars().count(); ins += &c.typed; ins += &c.after(); was += &c.orig; was += &c.after(); }
                for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
                acts.push(Action::Replace { delete: del, insert: format!("{ins}{typed}"), then: Some(b) });
                self.show(vec![BubbleItem { label: format!("{was}{thai}"), kind: BubbleKind::UndoConversion }], &mut acts);
            } else {
                let start = self.line.len() - k;
                let mut label = String::new(); for c in &self.line[start..] { label += &c.typed; label += &c.after(); }
                label += &typed;
                self.show(vec![BubbleItem { label, kind: BubbleKind::ConvertPrev { k } }], &mut acts); self.phrase = k + 1;
            }
        } else { self.show(vec![], &mut acts); }
        self.line.push(Committed { typed, orig: thai, converted, boundary: Some(b), rev: true, with_prev, extra: String::new() });
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
        let (mut del, mut ins) = (0usize, String::new());
        for c in &self.line[start..] { if c.converted.is_some() || !c.rev { return } del += c.orig.chars().count() + c.after().chars().count(); ins += &c.typed; ins += &c.after(); }
        acts.push(Action::Replace { delete: del, insert: ins, then: None });
        for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
        if let Some(c) = self.line.last_mut() { c.with_prev = k; }
    }
    /// Length (chars) of the last committed word as it is on screen, plus its boundary.
    fn last_on_screen(&self) -> Option<(usize, &Committed)> {
        let c = self.line.last()?; if !self.word.is_empty() { return None }
        Some((c.converted.as_deref().unwrap_or(&c.orig).chars().count() + c.after().chars().count(), c))
    }
    /// The buffer `typed` was just replaced mid-word by `converted`: record it with no boundary, so the next Tab /
    /// Undo / Guess measure from it instead of from the word before it.
    fn commit_glued(&mut self, typed: String, converted: String, with_prev: usize) {
        let orig = self.screen_of(&typed);
        self.line.push(Committed { orig, typed, converted: Some(converted), boundary: None, rev: self.thai_active, with_prev, extra: String::new() });
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
        let (mut del, mut ins) = (0usize, String::new());
        for (c, t) in self.line[start..].iter().zip(&targets) { del += c.orig.chars().count() + c.after().chars().count(); ins += t; ins += &c.after(); }
        if let Some(t) = &typed { del += self.screen_of(t).chars().count(); ins += &targets[k]; }
        acts.push(Action::Replace { delete: del, insert: ins, then: None });
        for (c, t) in self.line[start..].iter_mut().zip(&targets) { c.converted = Some(t.clone()); }
        if let Some(t) = typed { self.commit_glued(t, targets[k].clone(), k); }
        else if let Some(c) = self.line.last_mut() { c.with_prev = k - 1; }
    }
    fn on_hotkey(&mut self, h: Hotkey) -> Outcome {
        self.tab_pending = false;
        let mut acts = vec![];
        match h {
            Hotkey::Undo => {
                let Some((n, c)) = self.last_on_screen() else { return Outcome { swallow: true, actions: acts } };
                let c = c.clone(); if c.converted.is_none() { return Outcome { swallow: true, actions: acts } }
                if c.with_prev > 0 && self.line.len() > c.with_prev {
                    let start = self.line.len() - 1 - c.with_prev;
                    let (mut del, mut ins) = (n, String::new());
                    for pc in &self.line[start..self.line.len() - 1] { del += pc.converted.as_deref().unwrap_or(&pc.orig).chars().count() + pc.after().chars().count(); ins += &pc.orig; ins += &pc.after(); }
                    acts.push(Action::Replace { delete: del, insert: format!("{ins}{}{}", c.orig, c.after()), then: None });
                    let end = self.line.len() - 1; for pc in &mut self.line[start..end] { pc.converted = None; }
                } else {
                    acts.push(Action::Replace { delete: n, insert: format!("{}{}", c.orig, c.after()), then: None });
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
                    acts.push(Action::Replace { delete: shown.chars().count(), insert: target.clone(), then: None });
                    if !self.thai_active { self.feedback(&typed, Feedback::Retype); } // personal model: forward only
                    self.commit_glued(typed, target, 0);
                } else if let Some((n, c)) = self.last_on_screen() {
                    let c = c.clone(); if c.converted.is_some() { return Outcome { swallow: true, actions: acts } }
                    let target = if c.rev { c.typed.clone() } else { en_to_th(&c.typed) };
                    acts.push(Action::Replace { delete: n, insert: format!("{target}{}", c.after()), then: None });
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
                        if self.word != typed || self.line.len() < prev { return Outcome::pass() }
                        // replace the typed prefix (and, for a reverse phrase, the words before it); the rest of the
                        // word arrives in the new layout (input source switched)
                        let start = self.line.len() - prev;
                        let (mut del, mut ins) = (self.screen_of(&typed).chars().count(), String::new());
                        for c in &self.line[start..] { del += c.converted.as_deref().unwrap_or(&c.orig).chars().count() + c.after().chars().count(); ins += &c.typed; ins += &c.after(); }
                        for c in &mut self.line[start..] { c.converted = Some(c.typed.clone()); }
                        acts.push(Action::Replace { delete: del, insert: format!("{ins}{thai}"), then: None });
                        self.commit_glued(typed, thai, prev);
                    }
                    BubbleKind::UndoConversion => return self.on_hotkey(Hotkey::Undo),
                    BubbleKind::Convert { .. } => return self.on_hotkey(Hotkey::ForceConvert),
                    BubbleKind::ConvertPrev { k } => self.flip_prev(k, &mut acts),
                    BubbleKind::Guess { k, typed, targets } => self.apply_guess(k, typed, targets, &mut acts),
                    BubbleKind::Typo { word } => if let Some((n, c)) = self.last_on_screen() {
                        let c = c.clone();
                        acts.push(Action::Replace { delete: n, insert: format!("{word}{}", c.after()), then: None });
                        self.line.pop(); self.line.push(Committed { converted: Some(word), ..c });
                    },
                }
                self.show(vec![], &mut acts);
            }
            Hotkey::Dismiss => { self.show(vec![], &mut acts); return Outcome { swallow: false, actions: acts } }
            Hotkey::TogglePause => unreachable!(),
        }
        Outcome { swallow: true, actions: acts }
    }
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
        typ(&mut e, "hello"); let o = e.on_key(Key::Tab); assert!(!o.swallow); assert!(!e.bubble_visible());
    }
    #[test] fn legacy_tab_converts_as_boundary_when_no_bubble() {
        let Some(mut e) = engine() else { return };
        e.opts.auto_convert = true;
        e.opts.live = false;
        typ(&mut e, "l;ylfu"); let o = e.on_key(Key::Tab);
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
        let o = e.on_key(Key::Tab); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "l;ylfu ".into(), then: None });
        assert!(o.actions.contains(&Action::HideBubble)); assert!(o.actions.contains(&Action::SwitchInput(InputLang::Latin)));
        assert!(!e.bubble_visible());
        // with the bubble gone, Tab is an ordinary key again
        let o = e.on_key(Key::Tab); assert!(!o.swallow);
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
        let o = e.on_key(Key::Tab); assert!(o.swallow);
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
        let o = e.on_key(Key::Tab); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 5, insert: "าืนไ ".into(), then: None });
        assert!(o.actions.contains(&Action::SwitchInput(InputLang::Thai)));
    }
    #[test] fn reverse_live_bubble_and_tab() {
        let Some(mut e) = engine() else { return };
        e.opts.rev_live = 0.5;
        let o = typ_th(&mut e, "าืนไ"); // "know"
        assert!(o.iter().any(|x| bubble_items(x).is_some_and(|v| matches!(v[0].kind, BubbleKind::Live { .. }))), "live bubble mid-word");
        let o = e.on_key(Key::Tab); assert!(o.swallow);
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
    fn screen(o: &[(Option<char>, Outcome)]) -> String {
        let mut sc = vec![]; for (t, x) in o { crate::platform::sim::apply(&mut sc, &x.actions, x.swallow, *t); } sc.into_iter().collect()
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
        let o = e.on_key(Key::Tab); assert!(o.swallow);
        assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "สวัสดี\n".into(), then: None });
        // reverse mode too
        e.reset(); let mut o = typ_th(&mut e, "กนืะ าืนไ"); o.push(e.on_key(Key::Boundary(Boundary::Enter))); assert!(replaced(&o).is_empty()); assert!(e.bubble_visible()); assert!(o.iter().all(|x| !x.swallow));
    }
    #[test] fn tab_after_space_converts_previous_word() {
        let Some(mut e) = engine() else { return };
        e.opts.live = false;
        let mut log: Vec<(Option<char>, Outcome)> = "l;ylfu  ".chars().map(|c| (Some(c), e.on_key(key_of(c)))).collect(); // two spaces
        assert!(e.bubble_visible(), "extra space keeps the bubble");
        log.push((Some('\t'), e.on_key(Key::Tab)));
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
        typ(&mut e, "hello"); let o = e.on_key(Key::Tab); assert!(!o.swallow); assert!(o.actions.iter().all(|a| !matches!(a, Action::Replace { .. })));
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
        log.push((Some('\t'), e.on_key(Key::Tab)));
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
        log.push((Some('\t'), e.on_key(Key::Tab))); assert_eq!(screen(&log), en_to_th("l;ylf"));
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
        let o = e.on_key(Key::Tab);
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
        let o = e.on_key(Key::Tab); assert_eq!(o.actions[0], Action::Replace { delete: 7, insert: "i dont ".into(), then: None });
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
        for c in text.chars() { log.push((Some(c), e.on_key(if c == '\t' { Key::Tab } else { key_of(c) }))); }
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
        let o = e.on_key(Key::Tab);
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
}
