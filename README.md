# typeThai

**Fix Thai typed on the wrong keyboard layout, system-wide.** You type `l;ylfu` because the keyboard was on English,
and typeThai offers `สวัสดี`. It also works the other way: `ร กนืะ าืนไ` typed on the Thai layout becomes `i dont know`.

typeThai runs in the menu bar (macOS) or the system tray (Windows). When it recognises a word typed on the wrong layout,
it shows a small suggestion bubble at your text caret. Press **Tab** to apply the suggestion. It never changes your text
unless you press a key.

> **Status: v0.1.0-beta.** The macOS builds work on the author's machine. **Windows builds are untested beta.**
> Please report issues.

## Features

- **Both directions.** English keys meant as Thai (Kedmanee), and English typed while the Thai layout is active.
  Multi-word phrases work too (`i dont know`).
- **You decide when to convert.** Space, Enter and punctuation always pass through unchanged. The bubble offers the
  conversion for the word you are typing or the one you just finished. **Tab** applies it, and **⌃⌥Z** undoes it.
- **Keyboard-driven bubble.** **Shift+↑ / Shift+↓** choose an option, **Esc** or **Enter** dismisses it, **⌃⌥1 / 2 / 3** apply option 1–3.
  **Backspace** edits the word and keeps the bubble, and stepping back over a space re-opens the finished word.
- **Mid-word suggestions.** The bubble appears while you are still typing, from the third character, once the word
  is clearly Thai. You do not have to press Space first. Two-key words are offered after Space.
- **Input-source switching.** After a conversion the OS input source switches to match: Thai (Kedmanee) after
  converting to Thai, ABC/US after converting to English or undoing.
- **Why each option is offered.** A badge marks every fix: the flag of the language a wrong-layout word becomes
  (🇹🇭 Thai, 🇺🇸 English, as the keys are the US layout's), or a red spell-check mark for a typo fix. Finishing the
  word has no badge. Conversions come first, then typo fixes, then completions, so Tab is the conversion when there is one.
- **Horizontal or vertical bubble.** By default the options sit side by side in one strip, numbered left to right.
  A bubble with a long option (over about 28 letters) or one too wide for the screen is stacked instead, and so are
  ⌃⌥R selection and AI rewrite rows. **Settings… → Word suggestions → Bubble layout: Vertical** always stacks them.
- **Caret anchoring.** The bubble sits right under the text caret (via Accessibility / UI Automation). It flips above
  the caret near the bottom of the screen and works across monitors. Chromium/Electron apps are supported.
- **On-device learning.** Undos and manual conversions adjust a small personal model on your machine.
- **Word completion.** The bubble can finish the word you are typing: a Thai word (also the last word of an unspaced
  Thai run, or a word typed on the wrong layout, which is converted as it is completed) or an English word. By
  default completions appear after a short pause in typing (a quarter of a second; choose 150 to 600 ms in Settings), and
once they are up they follow the word as you keep typing. **Settings… → Word suggestions** switches them to
  **While typing** (shown as you type, only when one word is a clear favourite) or **Off**. **Tab** accepts the
  highlighted word, and **Esc** hides completions until the next word. Completions are ranked by the word before the
  one you are typing and by the words you type yourself: a word you use often comes first, and a word the built-in
  list lacks is offered once you have typed it three times. **Learn from my typing** turns this off, and **Forget
  learned words** clears it.
- **Thai typo suggestions.** For a converted word with a likely typo, extra rows offer corrected spellings.
- **English spelling suggestions.** A misspelled word left as English (`is ths`) gets the system spell checker's
  corrections (`this`, `the`); when the word is also offered as Thai, the best English spelling is the second row.
- **⌃⌥R for a selection or the last word.** With text selected, the bubble offers it in the other layout
  (`l;ylfu` → สวัสดี, or English typed on the Thai layout back to English) and with its typos fixed, both worked
  out on your computer, then the AI rewrite actions. With nothing selected it offers the conversion and spelling
  fixes for the word before the caret, the same rows the bubble shows after Space, also after you dismissed them.
- **AI rewrite (optional).** The ⌃⌥R bubble adds *Fix typos* and *Translate TH↔EN* for selected text, through an
  OpenAI-compatible service (OpenRouter by default) with your own API key.
- **Password fields are skipped** (macOS secure input / AXSecureTextField, Windows ES_PASSWORD / UIA IsPassword).

## Download and install

Get the latest build from [Releases](https://github.com/phasu21/typethai/releases).

### macOS (Apple silicon and Intel)

1. Download `typeThai-<version>-macos-universal.dmg`, open it and drag **typeThai** onto **Applications**.
2. The app is signed with typeThai's own certificate but **not notarized** by Apple. The first time, **right-click →
   Open → Open**. Or, in Terminal: `xattr -dr com.apple.quarantine /Applications/typeThai.app`
3. Grant **Accessibility** when asked, in System Settings → Privacy & Security → Accessibility. Add typeThai there if
   it isn't listed, then **relaunch**. typeThai needs this to see keystrokes, read the caret position and insert
   text. Some macOS versions also ask for **Input Monitoring**.
4. For input-source switching, add the **Thai (Kedmanee)** input source in System Settings → Keyboard → Input
   Sources. Without it, conversion still works, but the layout won't switch for you.

Coming from 0.4.0 or earlier (ad-hoc signed)? macOS sees the signed app as a new one: remove the old typeThai entry
from Accessibility and add the app again, once. Later updates keep the permission.

### Windows (x86_64, **beta / untested**)

1. Download `typeThai-<version>-windows-x86_64-setup.exe` and run it. It installs for your user only (no admin
   prompt), adds a Start menu entry and can start typeThai when you sign in. Uninstall from Settings → Apps.
   Prefer no installer? Unzip `typeThai-<version>-windows-x86_64.zip` instead and keep `typethai-desktop.exe` and
   `typethai.ttm` in the same folder.
2. The installer and exe are unsigned, so SmartScreen will warn: **More info → Run anyway**.
3. Add the Thai Kedmanee keyboard layout for switching (Settings → Time & language → Language & region).

### Updates

typeThai looks for a new release on GitHub at launch and every 6 hours, downloads it in the background and checks it
against typeThai's signing key. The tray menu then shows **Restart to update to …**: nothing changes until you pick
it. typeThai quits, the new version replaces it, and it starts again. **Check for updates** in the tray, or Settings →
Updates, checks right away; the automatic check can be turned off there.

Updating in place works for the app in `/Applications` (or any folder you can write to) and for the Windows
installer. A copy that can't replace itself (the Windows zip, a build from source) shows **Get typeThai …** instead,
which opens the release page.

## Using it

Each bubble option shows the keys that apply it. In the horizontal strip each button wears its key on its top-left
corner: `Tab` on the highlighted option (`↵` once the arrow keys are in the strip) and the others their number (hold Tab
and press it). Stacked rows show `Tab` on the highlighted row and `Tab` `2` and `Tab` `3`
on the others. Options are numbered from the left in the strip and from the top in rows.

| Key | Action |
|---|---|
| **Tab** (bubble visible) | Apply the highlighted option when Tab is released |
| **Hold Tab + 1 / 2 / 3** | Apply option 1 / 2 / 3 (the digit is not typed) |
| **Click** an option | Apply that option |
| **Space** (Accept with: Tab or Space) | Apply the highlighted mid-word conversion (`l;ylfu` → สวัสดี), then type the space. Over a completion or any other row, Space is a normal space |
| **Shift+↑ / Shift+↓** | Move the highlight (in the strip too; Shift+←/→ still select text in the app) |
| **↑**, then **← / →**, then **↑** or **Enter** (Pick with arrow keys on) | ↑ moves into the bubble, ←/→ pick, ↑ again or Enter applies; ↓, Esc or typing leaves. Off by default; until ↑ the arrow keys move the cursor |
| **Esc** | Dismiss the bubble |
| **⌃⌥1 / ⌃⌥2 / ⌃⌥3** | Apply row 1 / 2 / 3 (Ctrl+Alt on Windows) |
| **Ctrl+1 / 2 / 3** (Pick with Ctrl+1–3 on) | Apply row 1 / 2 / 3. Off by default; with no bubble up the keys reach the app |
| **⌃⌥Z** | Undo the last conversion |
| **⌃⌥X** | Force-convert the current or last word |
| **⌃⌥G** | Guess-convert: flip layout and apply the top typo suggestion in one step |
| **⌃⌥P** | Pause / resume (also in the tray menu) |
| **⌃⌥R** | Options for the selected text: other layout, typos fixed, then AI *Fix typos* / *Translate TH↔EN*; Tab replaces the selection. With nothing selected: options for the word before the caret |
| **Hold Tab** (half a second) | The ⌃⌥R options, for the selected text or the word before the caret. A quick tap is a normal Tab, sent when you release it; holding Tab no longer repeats it |
| **Tab+R** | Hold Tab and press R: the ⌃⌥R options, without waiting. Typing on from Tab quickly types the Tab as usual |

The menu-bar icon also has **Settings…** (or launch with `--settings` when the icon is hidden behind the notch). From
there you can choose when word suggestions show (Off, After a pause, While typing), how long the pause is and whether Space also
accepts a word typed in the wrong layout (**Accept with**: Tab, or Tab or Space), the **Bubble layout** (Horizontal by
default, or Vertical; prefs.json `"bubble_layout"`), whether **Ctrl+1–3** picks a row
(**Pick with Ctrl+1–3**, off by default; prefs.json `"pick_with_ctrl": true`), list words that stay as you type
them (`ok` is not offered as `นา`) and review or reset what typeThai has learned.

**AI rewrite setup.** Open **Settings… → AI rewrite**, paste an API key (an [OpenRouter](https://openrouter.ai) key works
with the defaults) and press **Save key**. The key is kept in the macOS Keychain or Windows Credential Manager, never in
a file. Model and base URL can point at any OpenAI-compatible `/chat/completions` server; plain `http://` is accepted
only for `localhost` (for example Ollama at `http://localhost:11434/v1`). **Instructions** adds your own text to the
prompt for both actions, for example a preferred tone. Esc, typing, a click elsewhere or switching apps cancels a
rewrite at any step. Without a key, or with **Offer AI rewrite** off, ⌃⌥R still offers the layout and spelling rows. Long
selections (over 300 characters) get only the AI actions.

When no bubble is shown, Tab, Shift+arrows and Esc behave normally. Press **Tab** twice in a row (without typing
in between) to run the guess-convert on the current or last word instead of using the `⌃⌥G` hotkey.

### Command-line flags (`typethai-desktop` / `typeThai.app/Contents/MacOS/typethai-desktop`)

| Flag | Meaning |
|---|---|
| `--model FILE.ttm` | Use another model file (default: next to the exe, `Contents/Resources`, or the config dir) |
| `--state FILE.json` | Personal-model state file |
| `--no-switch-input` | Don't change the OS input source after applying |
| `--no-reverse` | Disable the Thai-layout → English direction |
| `--no-typo` | No typo-suggestion rows (Thai or English) |
| `--auto-convert` | Legacy mode: convert automatically at Space/Enter when confident |
| `--simulate` | No key capture: read lines from stdin and print what would happen (try `l;ylfu <Tab>`; `<Idle>` is a typing pause; `^R` is ⌃⌥R, answered with the text of a preceding `<SEL:text>` or as nothing selected; `<APP:com.mitchellh.ghostty>` types into a terminal, which deletes a grapheme per Backspace) |
| `--list-input-sources` | List the enabled input sources (read-only) |
| `--probe-caret --secs N` | Diagnostic: log where the caret is detected and mark it on screen |
| `--demo-bubble [--items "a\|b" ...] [--select N] [--vertical] [--light] [--screenshot PNG]` | Show the bubble with sample options (no key capture). Repeated `--items` update it in place; `--screenshot` prints a layout self-check |
| `--demo-settings [--page N] [--search TEXT] [--light] [--screenshot PNG]` | Show only the Settings window with sample words (no key capture, prefs or keychain) |

## Privacy

- **Conversion and learning run locally.** Your typing is never uploaded.
- **AI rewrite is the one exception, and only when you press ⌃⌥R and pick *Fix typos* or *Translate TH↔EN*.** The
  layout and spelling rows are worked out on your computer. The selected text is sent to
  the service set in Settings (OpenRouter by default, with its data collection opt-out set). Nothing is sent without a
  key, and the feature can be turned off in Settings.
- **Update checks** ask GitHub for the latest typeThai release (the GitHub API and its download servers), at launch
  and every 6 hours. Nothing about you or your typing is sent. Turn it off in Settings → Updates.
- To work, typeThai reads your keystrokes system-wide. It keeps only the current word and line in memory, and drops
  them when you move the caret or switch apps. Password fields are skipped.
- The personal model is stored on your machine:
  - macOS: `~/Library/Application Support/typethai/personal.json`
  - Windows: `%APPDATA%\typethai\personal.json`

  It holds small weight adjustments plus the words you undid or converted manually. Delete the file to reset learning.
- Words you asked not to translate, and the word-suggestion settings, are stored next to it, in `prefs.json`. Deleting
  `personal.json` leaves them in place.
- With **Learn from my typing** on (the default), the words you finish and which word came before each are counted in
  `words.json`, next to `personal.json`, to rank word suggestions. Text in password fields is never seen. Turn the
  setting off to stop counting, and use **Forget learned words** or delete the file to clear it.

## Accuracy (measured on held-out data; summary)

Forward direction (English keys → Thai), classifier at its calibrated threshold:
- **Thai Wikipedia (test split): 99.8% of mistyped words detected.**
- **Pure English prose: 0 false suggestions** in 30,018 test tokens. On a later fresh sample: 16 in 152,021 (0.011%).
- **Source code: 0.068% false suggestions.**
- **Thai social-media text (held-out tweets): 99.6% detected.** English words embedded in Thai chat are the weak
  spot, at about 0.5–1.1% false suggestions (mostly emoticons and short brand names).

Reverse direction (Thai layout → English):
- **97.6% of English words detected (test).**
- **False suggestions on real Thai: 0.025% (2 in 8,088 Wikipedia words), and 0 on held-out tweets.**

Typo suggestions (synthetic typos in held-out reviews): **correct word first 81.7%, in the top 3 89.0%.**

Very short slang and abbreviations (2–3 letters) are still often missed. Because conversions now need Tab, a false
detection only means an unwanted bubble, never changed text.

## Build from source

Requirements: Rust (stable). macOS needs the Xcode command-line tools. Windows needs the Windows SDK (`fxc.exe` for
the gpui shaders) and MSVC.

```sh
cd rust
TYPETHAI_MODEL=path/to/typethai.ttm cargo test --release -p typethai-core -p typethai-desktop   # without a model they skip
cargo build --release -p typethai-desktop
# get the model (or build your own from data/model with tt-pack):
gh release download model-v3 -R phasu21/typethai -p typethai.ttm -D target/release
./target/release/typethai-desktop --simulate <<< 'l;ylfu <Tab>'
```

macOS universal app, DMG and updater zip: `scripts/package-macos.sh <version> path/to/typethai.ttm` (needs the
`aarch64-apple-darwin` and `x86_64-apple-darwin` Rust targets; set `MACOS_SIGN_P12` and `MACOS_SIGN_P12_PASSWORD` to
sign with the release certificate, otherwise it is ad-hoc signed). Windows installer: `packaging/typethai.iss` (Inno
Setup 6, see its header for the `iscc` arguments).

Releases: pushing a `v*` tag runs `.github/workflows/release.yml`, which needs three repository secrets:
`MACOS_SIGN_P12` (the code-signing certificate as a base64 `.p12`), `MACOS_SIGN_P12_PASSWORD`, and
`UPDATE_SIGNING_KEY` (the ed25519 private key, PEM, whose public half is `PUBLIC_KEY` in
`rust/crates/typethai-desktop/src/update.rs`). Losing either key means installed copies stop accepting updates (the
update key) or ask for Accessibility again (the certificate), so keep a backup outside GitHub.

Run from a checkout on macOS: `scripts/typethai.sh` builds the release binary, stops any running typeThai (including
the installed app) and starts it in the background (`stop`, `status`, `log`, `fg` also work). Link it onto your PATH to
start it with one word:

```sh
ln -sf "$PWD/scripts/typethai.sh" ~/.local/bin/typethai
typethai          # build + (re)start; log in ~/Library/Logs/typethai.log
```

Word pair counts for context-aware completion (model sections 12 and 13) are rebuilt from the corpora in
`DATA_LICENSES.md`. The first `min:top` level goes into the output model, and the others are only evaluated. The
script prints the `.ttm` size and held-out top-3 completion hit rates per level.

```sh
scripts/bigrams/fetch.sh "$DATA"                       # pinned URLs; writes $DATA/SHA256SUMS
python3 -m venv "$VENV" && "$VENV/bin/pip" install pyarrow
PYTHON="$VENV/bin/python" scripts/bigrams/build.sh "$DATA" "$WORK" typethai-en.ttm typethai-ctx.ttm 3:all 5:all 2:all   # model-v3 ships 3:all
```

The TypeScript prototype (`src/`, `scripts/`, `test/`, Node ≥ 23.6) holds the training and evaluation pipeline. The
corpora are not included; see `DATA_LICENSES.md`. `docs/` holds the manual test checklists.

## Licenses and data attribution

- Code: **MIT**, © 2026 phasu21 (`LICENSE`).
- Model (`typethai.ttm`): derived from Thai and English **Wikipedia** (CC BY-SA 4.0), the **PyThaiNLP** Thai word
  list (CC0), the **Wisesight Sentiment** corpus (CC0), the **FrequencyWords** English frequency list (CC BY-SA
  4.0), and, for word pair counts, **scb-mt-en-th-2020** (CC BY-SA 4.0, including Taskmaster-1, CC BY 4.0) and
  **Tatoeba** (CC BY 2.0 FR). It is distributed under CC BY-SA 4.0.
- Evaluation-only data (not in the model): Wongnai reviews (LGPL-3.0) and the Thai Toxicity Tweet Corpus
  (CC BY-NC 3.0).

Details are in [`DATA_LICENSES.md`](DATA_LICENSES.md).
