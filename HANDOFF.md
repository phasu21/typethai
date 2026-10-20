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
- The Windows module only type-checks (isolated crate, `windows` 0.62.2); not run on Windows yet. The whole crate
  type-checks for the Windows target on macOS with `scripts/check-windows.sh` (llvm-rc for gpui's build script, and a
  stub pkg-config so zstd-sys skips `cc`); running it still needs a Windows machine.

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
- Merged as one story with two names that say what they do: **Suggest the next word** (the pairs) and **Finish words
  from my typing** (the model, renamed from "Predict the next word").
- The model now answers the next word too: `Predictor::next_word(&mut self, before)` (crate) scores the saved word's
  followers of the line's last word with the completion chain and returns the best one's share; the engine asks once per
  line state (`ask_next_word`, keyed with `NEXT_WORD_KEY` so an answer is told apart from a completion's) and appends
  the word to the pair rows when it is not already one of them. The pairs stay the feature (`Suggest the next word`);
  the model adds what they cannot know (a name, jargon, a habit).
- The model answers from its own thread, so `offer_next_words` runs twice: below the pause guard (the pairs, once per
  pause, as before) and above it for a tick where `pred_next_answer` has landed since (`last_key` is already `None`).

## Permission self-repair (0.5.12)

- The tap is made by `arm_tap` (at capture-thread start, and again from the timer once a second while `State.tap_port`
  is null), so a grant takes effect without a relaunch. A refused tap no longer sends `UiMsg::Stopped`: the app stays
  up, sends `UiMsg::Permission(true)`, and the tray says `Grant Accessibility…` (`Accessibility: on` once granted).
- `repair_permission_once` — only when `__CFBundleIdentifier` is set, i.e. the installed bundle — runs
  `tccutil reset Accessibility <own bundle id>` once per version (marker beside the state file). A record saved by an
  older copy (ad-hoc, or the pre-0.5 certificate) otherwise keeps answering for the build in front of the user, which
  is the reported "the switch is on, nothing works, and every launch asks again".
- `TYPETHAI_FAKE_UNTRUSTED=1` walks the no-permission path by hand (like `TYPETHAI_DEBUG`).

## Input-language preview while a field is untouched

- The badge (🇹🇭 ไทย / 🇺🇸 EN over the caret) now also shows when focus lands in a text field nothing has been typed in
  yet, and *stays* until the first keystroke; it comes back `Options::badge_idle` after typing stops (Settings → Word
  suggestions → **Show it again after**: `BADGE_IDLE_CHOICES_MS` 1/2/3/5/10 s, default 2 s, prefs `"badge_idle_ms"`).
  A switch still shows it for `SHOW_FOR` (1.2 s). Everything else that owns the window still wins: the bubble, the hold
  indicator, pause, our own Settings window, a password field.
- `Options::badge_idle` is an `Option<Duration>`: `Some(pause)` brings the preview back on a pause, `None` ("Off" in
  Settings, `"badge_idle_ms": 0` in prefs) keeps it to the focus that showed it. A new focus still shows it either way.
- Policy in the engine (`focus_moved` / `on_keystroke` / `on_idle`, `Action::Badge(bool)`), so it is unit-tested;
  `Engine::reset()` deliberately does *not* touch it (a click re-asks focus a moment later, and hiding first would blink).
  `Shown::pinned` / `unpin` hold the state on the UI side; `UiMsg::Badge { …, pin }` and `UiMsg::UnpinBadge` carry it.
- Focus verdicts: macOS does one synchronous `focus_check()` in the timer, `field::SETTLE` (40 ms) after a click or an
  app switch (`arm_focus_look` → `focus_look`); a look armed by a *key* (Tab, ⌘K, Enter) only calls
  `Engine::set_field`: the caret under it is not laid out yet and the keystroke is typing, so the preview waits for the
  pause instead of pinning at a stale line (a terminal scrolling on Enter showed exactly that, off by a line).
  `offer_badge` looks again inside its `blocked` closure.
- The preview follows the caret: `Action::Badge(true)` arms the same 3-sample caret follow the bubble uses
  (`platform::follow::Follow`), and the timer sends `UiMsg::MoveBadge` while `Engine::badge_shown()` (a click or a
  switch arms it too, so focusing another field moves the pill there). `badge_anchor()` logs
  `typethai: preview anchor …` whenever that rect changes.
  Windows arms the same follow — on a key, on `Action::ShowBubble` and in `offer_badge` — and its 25 ms timer sends
  `Move` / `MoveBadge`; a click or a window switch still re-places the pill on the spot.
  Windows uses cheap signals (caret / known text app / the gate's cached verdict) and full UIA only in `offer_badge`.
  The Windows path type-checks with `scripts/check-windows.sh` but is not run on Windows yet.
- `--simulate` has `<Focus>` / `<Blur>` and any key now counts as a keystroke, so the whole policy is smoke-testable;
  `--demo-badge --pin` shows the pinned badge (`--secs 2` proves it outlives `SHOW_FOR`), and `--demo-settings` shows
  the two controls on the Word suggestions page.
- Verified: `cargo test -p typethai-desktop` (245 pass, model-backed), the CI `--simulate` line, `--demo-badge --pin`
  and `--demo-badge` on the Mac. Not verified live: the AX focus look after a real click (see the manual checklist).

## Local sentence completion (⌃⌥R → *Finish sentence locally*, 2026-10-18)

- `completion.rs` is the only module that knows Ollama's `/api/chat`: `think:false`, `stream:false`, temperature 0,
  `num_predict` 512, a decoding schema for `{decision, full_text}`, 60 s timeout, a 512-character input bound, a 16 KiB
  response cap, and proxies/redirects off so the selection cannot leave the machine. The endpoint is fixed
  (`127.0.0.1:11434`); the Ollama tag is `rewrite::Config::local_model`, passed into `completion::run(text, model)`.
  The system prompt and four few-shot pairs ask for the exact selection back with at most 160 new characters, or an
  empty-text abstention.
- Choosing a model (`2026-10-18`, after the 4B/8B comparison): `rewrite::LOCAL_MODELS` is the Settings dropdown
  (`gemma4:latest` default, `hf.co/typhoon-ai/typhoon2.5-qwen3-4b-gguf:Q4_K_M` the faster one), `with_local_model`
  validates one tag and `from_json` falls back to the default for a hand-edited `prefs.json`. Nothing else in the app
  changed for it: the choice lives in the existing `rewrite` config, so `prefs.json` (and the Settings save/key/gate
  plumbing) needs no new key or command.
- Setup: `local_setup.rs` finds `ollama` (PATH, then Homebrew/`/Applications/Ollama.app`/Windows install dirs), reads
  `/api/version` and `/api/tags`, and presses on: `open -a Ollama` on macOS (else `ollama serve`, detached) when the
  server is down, then `ollama pull <tag>` with its progress lines (stdout+stderr, `\r`-repainted) streamed into the
  Settings row. `plan`/`has_tag`/`describe` are pure; `check` and `ensure` are the thin live wrappers. The row is
  **Settings → AI rewrite → Local model setup** (button + live note), and the bubble's failure lines now point there
  instead of naming a tag.
- Validation (`rewrite.rs` `Failure::Local*`) refuses everything else: a suffix that is blank, over 160 characters, holds
  control characters or U+FFFD, contains any numeric character, repeats the selection, or leaves an unbalanced quote.
  Abstention has its own message. Cloud config is ignored and a failure never falls back to a cloud call
  (`rewrite::execute` dispatches on `run.preset`); the row is offered without an API key.
- Engine: `Preset::Finish` is last in `Preset::ALL`, the busy row reads "Finishing locally…", and a reply still lands
  only while the bubble holds its `Busy(id)`, so Esc or typing during a request inserts nothing.
- `platform/sim.rs` runs the same `completion::run` for a Finish job, so the path is smoke-testable:
  `printf '<SEL:Please let me>^R<TabDown>3<TabUp>\n' | TYPETHAI_MODEL=/tmp/ttmodel/typethai.ttm rust/target/debug/typethai-desktop --simulate --state /tmp/ttsim/personal.json`.
- Exercised 2026-10-18 with that simulator (see `docs/research/llm-sentence-completion-in-app.md` and the transcript
  beside it): five plausible continuations (English phrase, partial word, Thai, mixed, and the `…keep in` boundary case
  the raw prompt got wrong), both identifier traps abstained, an unreachable Ollama produced the "Start Ollama…" notice,
  Tab pasted the result and Esc cancelled it. Warm requests ran about 1.1 s per completion for repeated identical
  prompts (simulator wall-time difference, not a UI measurement); varying the prefix costs 1,285 ms p50 for the 4B and
  2,210 ms for `gemma4:latest`, and a cold model 2,645 ms / 5,671 ms
  (`docs/research/llm-sentence-completion-app-contract.md`, which re-ran the 4B under this same request and kept
  `gemma4:latest` as the default).
- Verified after the option and setup landed: `cargo test -p typethai-desktop` (282 pass), the simulator with each tag
  in `prefs.json` returned that model's own measured answer (`…keep in touch!` for Gemma, `…keep in touch.` for the 4B),
  `--demo-settings --page 3` showed the real status ("gemma4:latest is ready" with a disabled Ready button), and
  `cargo test -p typethai-desktop local_setup_live -- --ignored --nocapture` drove a real `ollama pull` of an installed
  tag to `success`. Not verified live: the Start path (this machine's server was up), the Get Ollama path, and a real
  multi-GB download's progress rendering in the row.
- Settings page shape (2026-10-18): the AI rewrite page is split into **Local** and **Cloud** tabs (`RewriteTab`, a
  `TabBar::segmented` as the first item of the page's second group; the switch and Hold Tab rows stay above it and apply
  to both, and the Save button lives on the Cloud tab because the local rows apply at once). Each row carries one short
  line at most; the long explanations sit in a
  hover card behind the ⓘ (kit `HoverCard`) and the setup control is one stateful button — `Checking…` / `Get Ollama` /
  `Start Ollama` / `Download model` / `Downloading 42%` (spinning) / `Ready` (disabled) / `Try again`, from
  `ui::settings::local_face`, which is unit-tested per state. The row shows a note only when setup failed. Hovering
  could not be driven from an agent shell here (synthetic mouse moves do not reach gpui's hover, and synthetic clicks do
  not open even the shipped dropdown menus), so the ⓘ card's *opening* is the one part reviewed by hand; its content and
  every button face were checked in screenshots.
- Not verified live: the real ⌃⌥R key capture, selection read and paste into another app; Windows; the cold-model wait in
  the bubble. No user trial, so acceptance, retained text and task time are unmeasured.
- Reviewing the row without moving Ollama around: `typethai-desktop --demo-settings --page 3 --local-state failed`
  (`checking`, `install`, `start`, `pull`, `downloading`, `ready`, `failed`; `--local-cycle` walks them every 1.6 s and a
  click steps, `--screenshot PNG` captures one). The demo never touches Ollama. `--settings --settings-page 3` opens the
  real app on the AI rewrite page; with `TYPETHAI_FAKE_UNTRUSTED=1` it runs without the event tap, so it can be shown
  beside an installed copy without both acting on the same keystrokes.

## The rail at the caret in Windows Electron / Chromium apps (2026-10-18)

- Reported: on Windows, in an Electron app, the bubble did not follow the caret. Two causes, both Windows-side:
  - Electron and Chromium answer no `UIA_TextPattern2Id` (Chromium's `GetPatternProviderFactoryMethod` returns nothing
    for it; `ui/accessibility/platform/ax_platform_node_win.cc`), and they have no Win32 caret either
    (`GetGUIThreadInfo` -> `hwndCaret` null). So `uia_caret` found no caret and the bubble fell back to the focused
    element's frame: its left edge, with nothing to follow.
  - `uia_caret` now asks TextPattern2 first (Word, WinUI) and then TextPattern: the caret is the collapsed selection,
    and Chromium gives that degenerate range as a 1-px insertion-point rect for UIA (`AXRange::GetRects` builds one on
    purpose), the same way `marker_caret` reads their caret on macOS. A provider that answers neither falls back to the
    character the caret sits on (its left edge, or its right edge at the end of the text).
- Windows had no keystroke-driven caret follow at all (macOS has had one). The state machine is now shared as
  `platform::follow` (`Follow::arm` / `take`: three reads after a key, 30 ms then 70 ms apart, no faster than 50 ms),
  driving the macOS tap and the Windows hooks. `arm_caret_follow` fires on `Action::ShowBubble`, on a key while the
  bubble is up, and whenever `offer_badge` shows; `control()`'s 25 ms timer re-reads and sends `UiMsg::Move` while the
  bubble is up, `MoveBadge` while the preview is.
- `TYPETHAI_DEBUG` now says what placed the bubble, on change: `typethai: anchor source=caret|field|mouse caret=… field=…`
  and `typethai: focus hwnd=… password=… field=Text|NotText|Unknown` (the gate's verdict). The Windows counterpart of
  macOS's `--probe-caret` / `typethai: preview anchor …`.
- Verified on the Mac: `cargo test -p typethai-desktop` (297 pass, 3 ignored — including the new `platform::follow`
  tests, which cover a single arming's three reads, the spacing, and a run spent when nothing is up), and a live run
  typing `l;ylfu` then `l;ylfukl` into TextEdit (screenshots: the rail sat at the caret both times, moved right with
  the two characters typed, above the caret line).
- Verified on a real Windows 11 machine (2026-10-18, over SSH; `sshd` runs in session 0, so every desktop action went
  through a scheduled task with an interactive token — `schtasks`/`Register-ScheduledTask -LogonType Interactive`):
  `C:\tmp\typethai-build` (b86bf29 + these files) built with `cargo build -p typethai-desktop`, then typing `l;ylfu`
  into one `<textarea>` on a `file://` page in Edge logged, per key,
  `typethai: focus … field=Text` and `typethai: anchor source=caret caret=Some(Rect { x: 981 → 996 → 1010 → 1025 →
  1040, y: 1098, w: 1, h: 32 })` — the caret read on its line, 1 px wide (Chromium's degenerate-range caret, so the
  TextPattern path, not `GetGUIThreadInfo`), advancing with each character; the screenshot shows the rail above the
  caret with its first label at the caret's x. The same typing with the installed 0.5.14 put the rail hundreds of
  pixels away from the caret (the mouse fallback) — the reported bug. Grok Bot (Electron) also answered `field=Text`
  and a caret.
- Not covered: a Chromium element that exposes no caret at all (VS Code's inline prompt widget did that) still falls
  back to its field or the mouse, and a caret read is a UI Automation round trip, so the 60 ms cap can drop one
  (`anchor source=field caret=None` on some VS Code keys). The follow's later samples are what correct that.

## The bubble anchored to the caret on Windows (2026-10-20)

- Reported: on Windows the bubble sometimes floats somewhere else than the caret. Six causes, all in the anchor the
  bubble is placed from (`platform::windows::anchor`) and in the fallback order (`placement::place`), none of them in
  the placement itself:
  - The Win32 caret rect was taken as it came. `GetGUIThreadInfo`'s `rcCaret` right and bottom carry text-direction
    and padding information, not the caret's edges (measured: `rcCaret` of a caret made with `CreateCaret` came back
    as `40 30 -1043475384 14`), and a window that has no caret reports a 1x1 placeholder at the client's origin —
    anchoring there is the corner of the screen, not the text (the app logged `caret=Some(Rect { x: 0.0, y: 0.0, w: 1.0,
    h: 1.0 })` with a remote-desktop window in front). `placement::win32_caret` now takes the position and a height
    that looks like a line's; a 1-px or an inverted rect is not a caret.
  - The UI Automation caret could be the *selection's* box: `GetSelection()` answers the selection, and its rect is
    the selected text, so the bubble sat at the selection's edge instead of where the next character goes. The range is
    collapsed to its end (the caret) before it is measured, the same end macOS reads for an AX selection.
    TextPattern2's `active` flag is honoured as well: a range that is not the active caret is nobody's caret.
  - A read that came back without a caret threw the bubble (and the badge) to the field's left edge or to the mouse.
    `placement::keep_caret` keeps the caret in use when a re-read finds none; the bubble (`Ui::show`, `Ui::reanchor`)
    and the badge (`BadgeWin::move_to`) use it. A hidden bubble forgets its anchor, so a caret from another window
    cannot survive a hide.
  - The caret read shared its worker with the focus read, which is the slow one (the cached element carries five
    properties, each a round trip to the app), so a focus read in flight could make the caret read miss its 60 ms cap
    — the `anchor source=field caret=None` the previous section left open. The two now have a worker each, and the
    focused element and its caret come back in one round trip instead of two. Measured in an idle Edge (60 s, 298
    words): the caret read is p50 2 ms / p90 6 ms / p99 19 ms / max 28 ms over 2093 reads, the focus read p50 1 ms /
    max 19 ms over 599 reads — so the cap is only at risk when the app itself stalls, which is when the drops were
    seen (one in a 45 s session of continuous typing).
  - The UI Automation cache request was made once at startup. When it failed (it does, in a fresh thread: the first
    `CreateCacheRequest` on a new automation client can come back empty) every focus read answered "unknown" and the
    gate never opened, so the app did nothing at all. It is made per read now.
  - The Windows path now applies the plausibility filter macOS has always had (`placement::plausible_caret`): a caret
    that does not sit in the focused element is not the caret.
  - With no caret at all (an app that exposes none), the fallback is the pointer instead of the field's left edge:
    the field's edge is the far left of the input, away from the text, while the pointer is where the text is. The
    pointer counts only when it is in the field; a pointer somewhere else (a menu bar) still leaves the field as the
    closer guess, and the pointer stays the last resort. `placement::mouse_line` + the order in `place` (and in
    `source_of`, so the log agrees) are the change.
  - On Windows that pointer is the click that put the focus in the field, not the live pointer: the click is where the
    caret went, and the pointer moves on. The mouse thread already sees every click's place, so it keeps it
    (`State::last_click`), and the anchor takes it when it landed in the focused window (`placement::pointer_at`, the
    same helper macOS uses).
- `TYPETHAI_DEBUG` says more about the reads: `typethai: uia focus|caret read dropped after N ms` (it missed its cap)
  and `typethai: uia focus|caret read took N ms` (over 20 ms).
- Verified on this Windows 11 machine (2026-10-20):
  - A classic Win32 `EDIT` (focused, typed into with real key events): the app's anchor is the caret's own place and
    matches `GetGUIThreadInfo` exactly (`rcCaret` client `38 0 39 20` -> screen `259 255`; the app logged
    `anchor source=caret caret=Some(Rect { x: 259.0, y: 255.0, w: 1.0, h: 20.0 })`), and the caret advances with the
    text (`231 -> 241 -> 245 -> 249 -> 259`).
  - An Edge page with a `<textarea>`: `field=Text`, and the caret read advances per key (1 px wide, the insertion point
    Chromium builds) — 454 caret anchors in a 45 s typing marathon of 189 words, no dropped read, no field or mouse
    fallback.
  - `cargo test -p typethai-desktop`: 305 pass (the `prefs` failures are pre-existing and flaky: those tests share a
    temp directory named after the process id and clobber each other when they run in parallel — each one passes on
    its own).
  - New tests in `placement`: `win32_caret_is_the_position_and_the_line_height`,
    `win32_caret_placeholder_is_not_a_caret`, `win32_caret_rejects_padding_junk`,
    `a_read_without_a_caret_keeps_the_one_in_use`, `a_read_with_a_caret_replaces_the_one_in_use`,
    `the_pointer_in_the_field_beats_the_field_edge`, `the_pointer_outside_the_field_leaves_the_field_edge`.

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
