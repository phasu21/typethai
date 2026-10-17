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
