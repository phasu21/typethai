# Handoff: guess-convert and double-Tab suggestion

## Status

Uncommitted work in progress. Run tests with `TYPETHAI_MODEL=/tmp/ttmodel/typethai.ttm cargo test` (from `rust/`).
Without that variable, every model-backed test prints `SKIP` and passes vacuously; `engine()` and core `model()` now
fall back to it, so all desktop and core tests really run. Run once with `/tmp/ttmodel/typethai-en.ttm` too (English
completion frequencies).

## Nonsense-suggestion fix (2026-10-14)

- Cause 1, personal model: `max_drift` capped each weight, not the total shift. Feature 37 (one-word line) has
  standardized z ~ 6.5, so 9 lone-word conversions moved every first word in a field by +3.4 logit (`it` 0.13 -> 0.95,
  `Hmm` 0.03 -> 0.64). `shift()` now caps `db + sum(dw * z)` at `max_drift` in scoring and learning (Rust + TS).
  Regression: `personal_shift_is_capped` in `typethai-core/tests/core.rs`.
- Cause 2, live bubble: `0b199ff` dropped `live_min_len` to 2. Two-key English prefixes score high (`wo` -> ไน 0.95,
  `fu` -> ดี 0.997), so it is 3 again; 2-key words are offered after Space.
- Non-dictionary conversions (prefixes, phrases, typos) need `STRICT_BAND` (0.9) live and at the boundary; dictionary
  words keep the 0.5 band. Replaces the hardcoded 0.9 in `on_boundary`.

## User-visible behavior

- New hotkey `Ctrl+Alt/Ctrl+Option+G` (`Hotkey::Guess`) and double-Tab when no bubble is up.
- First trigger only **shows** the proposed full walkback conversion in the bubble; a second Tab applies it.
- The proposal walks backward through consecutive words typed on the same (wrong) layout, not just the last word.
- Forward phrases get the top typo correction for each word; reverse phrases are laid out as English.
- Plain English-looking inputs are suppressed so the trigger does not propose a language switch.

## Key files changed

- `rust/crates/typethai-desktop/src/engine.rs`
  - Added `Hotkey::Guess`.
  - `tab_pending`: a Tab that typed itself; the next consecutive Tab shows the Guess proposal, or is a plain tab
    when there is nothing to propose.
  - `BubbleKind::Guess { k, typed, targets }` carries the proposal (`guess()`); `apply_guess()` checks the buffer and
    words are unchanged, inserts the targets, and records them as converted to exactly what was inserted.
  - Mid-word conversions (Guess on the buffer, live accept, ForceConvert) are committed with `boundary: None`
    (`commit_glued`), so Tab/Undo/Guess afterwards measure from the converted word; Undo gives the buffer back.
  - Tests: `guess_typo_correction_undoes_cleanly`, `guess_phrase_with_buffer_undoes_as_one`,
    `double_tab_guesses_the_last_word`, `guess_skips_english_and_goes_stale`,
    `guess_after_live_accept_measures_from_the_converted_word`.
  - Forward-only, stricter band attempted for out-of-dictionary conversions; rare false positives such as `riht` remain.
- `rust/crates/typethai-desktop/src/app.rs`
  - Mapped `'g'` to `Hotkey::Guess`.
- `README.md`
  - Documented `⌃⌥G` and the double-Tab shortcut.

## English spelling suggestions

- `engine::Speller` trait; `Engine::speller` is set from `platform::spell::system()` in `App::load` (off with
  `--no-typo`). macOS: `NSSpellChecker` (language `en`) called on the capture thread. Windows: `ISpellChecker`
  (`en-US`) on its own MTA worker; the hook thread waits at most 80 ms. The model's English word list (8.6k wiki
  words, no frequencies, no `hello`) was too thin to spell-check with.
- `en_items()`: ASCII words (apostrophes allowed, surrounding punctuation kept) at a forward boundary. Left as English:
  rows are the guesses. Offered as Thai: row 2 is the best English guess. Accept is a `Typo` row, so Undo works.
- The Windows module only type-checks (isolated crate, `windows` 0.62.2); not run on Windows yet. Full
  `--target x86_64-pc-windows-msvc` check fails in gpui's build script on macOS.

## Word completion (branch `feat/word-completion`)

- Core: `typethai_core::completer` (`Completer`, `thai_tail`); `ModelFiles::{thai_completer, english_completer}`.
  English frequencies are TTM section 11 (FrequencyWords `en_50k`); older `.ttm` files have none, and then English
  completion is off while Thai still works. `Completions.confidence` counts the prefix itself when it is a word, so
  `you` / `ฉัน` do not read as confident prefixes.
- Engine: `Options::{complete, complete_conf (0.4), complete_idle (50 ms)}`, `CompleteMode::{Off, Pause, Typing}`
  (default Pause). `completion_rows` picks the text: a Thai layout run or a wrong-layout word under a live row
  completes its last word (`thai_tail`) and keeps the head; otherwise an ASCII word from `en_comp`, topped up by
  `Speller::completions` (NSSpellChecker; none on Windows). English typed on the Thai layout keeps only its reverse
  live row. `BubbleKind::Complete { typed, insert }` accepts like a live accept (`commit_glued`, Undo gives the keys
  back). Esc / Dismiss mute until the next boundary. `show()` keeps one bubble action per key.
- Pause mode: `Engine::on_idle(now)` from the macOS 25 ms `timer_cb` and the Windows 100 ms `control` timer; it fires
  once per pause (`last_key` is taken). `--simulate` has an `<Idle>` token.
- Settings: `prefs.json` `"completion": "off" | "pause" | "typing"` (missing -> pause), `SettingsCmd::SetCompletion`,
  a Word suggestions section in the Settings window (not checked on screen yet).
- Cost (release, `cargo test --release -p typethai-desktop completion_cost -- --ignored --nocapture`): rows p50
  0.6 us for an English prefix, 8 us for a 30-char Thai run; a whole keystroke 4.4 us (Off) vs 9 us (Typing). The
  NSSpellChecker top-up costs about 0.85 ms per call, so it runs only for Pause-mode rows (`on_idle`), never inside
  the key hook (Typing mode uses the frequency list alone); it fills in when the list has fewer than 3 words.
- Data: `tt-pack --enfreq` drops the `n't` stems the subtitle list splits off (`wasn`, `aren`, ... `NT_STEMS`; `won` stays).
- Ask length: a pause and the keys that follow it ask from the **first** key or letter, so `t` offers the list's `the`
  and `to` after the pause (a code editor's ghost does the same); `Ask::Typing` still waits for **3** keys for a Thai
  run (nobody waited for those rows) and English Typing rows stay away by themselves under `complete_conf`.
- Reverse rows: English typed on the Thai layout keeps its reverse row as the first row of the pause bubble. A word the
  app always offers back (`REV_ALWAYS`: `ok` ← นา, and every don't-translate word) gets that row after a pause as well,
  not only at the word boundary, and its Thai completions stay under it (`completion_rows` skips the live-row
  suppression for those words), so the same bubble holds both readings.

## Contextual word prediction (next word from your own typing)

- Core: `crates/typethai-predict`, a port of oh-my-pi's `pi-predict` n-gram engine (MIT; the crate carries
  `LICENSE-THIRD-PARTY.md`). An interpolated absolute-discounting **word trigram** over the words this user typed,
  ranked under the typed prefix with a finished-word-aware posterior (the prefix itself competes, so `the|re` shows
  only when *there* clearly beats stopping at *the*), a prompt-local and a session cache, vocabulary hygiene (rare
  misspellings fold into a real edit-distance-1 word, a habitual slip of a word the user also types is never offered)
  and a zstd `PINGRAM` snapshot written atomically. The Norvig web prior the upstream engine ships is **not** ported:
  nothing outside this user's own typing reaches the model, and the words a dictionary knows are injected through
  `WordOracle` instead. `Method`, the SmolLM2-135M engine and the macOS `NSSpellChecker` engine are dropped
  (`docs/research/embedded-llm-next-word-prediction.md` §12 explains why an embedded LLM is not the default here).
- Adapter: `typethai_core::predict` reads a line the way the model does — English lowercased, Thai runs segmented with
  the dictionary, and every run of non-word characters rewritten as the one character the model reads it by: `" "`
  (join: space, comma, dash), `"\n"` (sentence: `.` `!` `?` `:` newline) or `"|"` (break: a symbol, a digit, Thai the
  dictionary does not cover, a script switch with no separator). `KnownWords` is the oracle: the model's Thai
  dictionary plus its English word list.
- Thread: `typethai_desktop::predict::Predict` owns the model off the key tap callback for the same reason `words.json`
  has a saver thread. `Engine::learn_line` sends the finished line once (on Enter, or when the line is forgotten),
  gated by `learn_words`; `Engine::predicted` asks once per word state and reads the answer on a later tick, so the
  published row can lag one keystroke, which `pred_shown` papers over by keeping the shown word up while the user types
  into it. The snapshot is written 5 min after the last line, 15 min at the latest, and on quit (`App::save_and_wait`).
- Rows: one `BubbleKind::Complete { typed, insert }` row, first under the spell checker's lead row, and it stands on
  its own — a word the frequency list cannot extend but the user has typed before is exactly what the model is for.
  It follows the same `Ask` rules, `k` cap, dedupe and accept path as the dictionary rows (`Tab` completes the word,
  what the keys typed stays on screen). A word neither dictionary knows is offered after it was typed twice
  (`Params::min_count`, upstream's rule); a word they know is offered after one line.
- Settings: `prefs.json` `"predict"` (missing -> on), `SettingsCmd::SetPredict`, "Predict the next word" in Word
  suggestions. Off means the model is not even started: no state file is read or written. **Forget learned words**
  clears it too.
- State: `<state dir>/ngram.snapshot` beside `words.json`; a snapshot from another version or a damaged one is dropped
  and the model re-learns from then on. There is no hard vocabulary cap, as upstream: the model compacts its learned
  vocabulary into the sorted base every 4,096 new words and drops paste-only trigrams on every persist, and a typist's
  vocabulary saturates — 5,000 finished lines of 8 words hold 5 MiB of heap (upstream's 52,950-prompt model held
  52 MiB, most of it the web prior this port does not ship).
- Cost (`cargo test --release -p typethai-desktop predict_cost -- --ignored --nocapture`, M4 Max, a model
  bootstrapped from this build's 46k-word English list with 5,000 lines of 8 words): `complete` p50 **2.4 us** at one
  letter, **0.9 us** at two, **0.6 us** at four (p99 4.5 us); `observe` p50 12 us per finished line; heap 5 MiB;
  snapshot 570 KB, persist 16 ms, restore 9 ms. On the key thread the query, the adapter and the projection add
  **+3.8 us p50** to a keystroke over a 30-char Thai run (15.7 us -> 19.5 us; p99 45.3 -> 55.5 us), which is inside
  the ~1 ms the macOS tap allows. The µs query fits the key hook, the ms persist does not, which is why the model
  lives on its own thread.

## One next-word feature, and the model behind the word you type

- 0.5.9 shipped main's *Suggest the next word* (built-in corpus pairs, offered when nothing is being typed); the local
  work added the personal word n-gram (`typethai-predict`). They look like two features because both were called
  "next word": the model's `complete` is prefix-constrained (its `Query::prefix` is at least 2 letters of the word being
  typed, and `Engine::predicted` refuses an empty tail), so it *cannot* answer "what word comes next".
- Merged as one story with two names that say what they do: **Suggest the next word** (the pairs; unchanged) and
  **Finish words from my typing** (the model, renamed from "Predict the next word").
- To have the model also propose the *next* word, `typethai-predict` needs a `next_word(&Query)`-style call over its
  trigram (`Model::top_k` / `followers_of` are private to the completion path today); then the pairs become the
  fallback. Not done: it is a change to the ported crate, with its own confidence semantics.

## Input-language preview while a field is untouched

- The badge (🇹🇭 ไทย / 🇺🇸 EN over the caret) now also shows when focus lands in a text field nothing has been typed in
  yet, and *stays* until the first keystroke; it comes back `Options::badge_idle` after typing stops (Settings → Word
  suggestions → **Show it again after**: `BADGE_IDLE_CHOICES_MS` 1/2/3/5/10 s, default 2 s, prefs `"badge_idle_ms"`).
  A switch still shows it for `SHOW_FOR` (1.2 s). Everything else that owns the window still wins: the bubble, the hold
  indicator, pause, our own Settings window, a password field.
- Policy in the engine (`focus_moved` / `on_keystroke` / `on_idle`, `Action::Badge(bool)`), so it is unit-tested;
  `Engine::reset()` deliberately does *not* touch it (a click re-asks focus a moment later, and hiding first would blink).
  `Shown::pinned` / `unpin` hold the state on the UI side; `UiMsg::Badge { …, pin }` and `UiMsg::UnpinBadge` carry it.
- Focus verdicts: macOS does one synchronous `focus_check()` in the timer, `field::SETTLE` (40 ms) after a click or an
  app switch (`arm_focus_look` → `focus_look`); a look armed by a *key* (Tab, ⌘K, Enter) only calls
  `Engine::set_field`: the caret under it is not laid out yet and the keystroke is typing, so the preview waits for the
  pause instead of pinning at a stale line (a terminal scrolling on Enter showed exactly that, off by a line).
  `offer_badge` looks again inside its `blocked` closure.
- The preview follows the caret: `Action::Badge(true)` arms the same 3-sample `arm_caret_follow` the bubble uses, and
  the timer sends `UiMsg::MoveBadge` while `Engine::badge_shown()` (a click or a switch arms it too, so focusing
  another field moves the pill there). `badge_anchor()` logs `typethai: preview anchor …` whenever that rect changes.
  Windows has no keystroke-driven follow; its window switch and mouse hook re-place the pill directly.
  Windows uses cheap signals (caret / known text app / the gate's cached verdict) and full UIA only in `offer_badge`.
  The Windows path type-checks (`cargo check --target x86_64-pc-windows-msvc`) but is not run on Windows yet.
- `--simulate` has `<Focus>` / `<Blur>` and any key now counts as a keystroke, so the whole policy is smoke-testable;
  `--demo-badge --pin` shows the pinned badge (`--secs 2` proves it outlives `SHOW_FOR`), and `--demo-settings` shows
  the two controls on the Word suggestions page.
- Verified: `cargo test -p typethai-desktop` (245 pass, model-backed), the CI `--simulate` line, `--demo-badge --pin`
  and `--demo-badge` on the Mac. Not verified live: the AX focus look after a real click (see the manual checklist).

## Open limitations

- `riht` is 0.82 on a fresh model (hidden now) but 0.93 with the +1 logit the current personal state allows.

## Run

```sh
typethai            # scripts/typethai.sh, linked at ~/.local/bin/typethai: release build + background (re)start
typethai stop | status | log | fg
```

Simulate:

```sh
printf 'l;ylfu <Tab><Tab>\n' | /Users/alkanepk/orca/typethai/rust/target/debug/typethai-desktop --simulate --model /tmp/ttmodel/typethai.ttm --no-switch-input
```
