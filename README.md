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
- **Keyboard-driven bubble.** **Shift+↑ / Shift+↓** choose a row, **Esc** dismisses it, **⌃⌥1 / 2 / 3** apply row 1–3.
  **Backspace** edits the word and keeps the bubble, and stepping back over a space re-opens the finished word.
- **Mid-word suggestions.** The bubble appears while you are still typing, from the third character, once the word
  is clearly Thai. You do not have to press Space first. Two-key words are offered after Space.
- **Input-source switching.** After a conversion the OS input source switches to match: Thai (Kedmanee) after
  converting to Thai, ABC/US after converting to English or undoing.
- **Caret anchoring.** The bubble sits right under the text caret (via Accessibility / UI Automation). It flips above
  the caret near the bottom of the screen and works across monitors. Chromium/Electron apps are supported.
- **On-device learning.** Undos and manual conversions adjust a small personal model on your machine.
- **Word completion.** The bubble can finish the word you are typing: a Thai word (also the last word of an unspaced
  Thai run, or a word typed on the wrong layout, which is converted as it is completed) or an English word. By
  default completions appear after a short pause in typing. **Settings… → Word suggestions** switches them to
  **While typing** (shown as you type, only when one word is a clear favourite) or **Off**. **Tab** accepts the
  highlighted word, and **Esc** hides completions until the next word.
- **Thai typo suggestions.** For a converted word with a likely typo, extra rows offer corrected spellings.
- **English spelling suggestions.** A misspelled word left as English (`is ths`) gets the system spell checker's
  corrections (`this`, `the`); when the word is also offered as Thai, the best English spelling is the second row.
- **AI rewrite (optional).** Select text and press **⌃⌥R** to fix its typos or translate it between Thai and
  English, through an OpenAI-compatible service (OpenRouter by default) with your own API key.
- **Password fields are skipped** (macOS secure input / AXSecureTextField, Windows ES_PASSWORD / UIA IsPassword).

## Download and install

Get the latest build from [Releases](https://github.com/phasu21/typethai/releases).

### macOS (Apple silicon: `aarch64`, Intel: `x86_64`)

1. Download `typeThai-<version>-macos-<arch>.zip` and unzip it. Move `typeThai.app` wherever you like.
2. The app is **not notarized**, only ad-hoc signed. The first time, **right-click → Open → Open**. Or, in Terminal:
   `xattr -dr com.apple.quarantine /path/to/typeThai.app`
3. Grant **Accessibility** when asked, in System Settings → Privacy & Security → Accessibility. Add typeThai there if
   it isn't listed, then **relaunch**. typeThai needs this to see keystrokes, read the caret position and insert
   text. Some macOS versions also ask for **Input Monitoring**.
4. For input-source switching, add the **Thai (Kedmanee)** input source in System Settings → Keyboard → Input
   Sources. Without it, conversion still works, but the layout won't switch for you.

Each new version is a new ad-hoc signature, so macOS may ask for Accessibility again after an update. If it stops
reacting, remove the old entry and re-add the app.

### Windows (x86_64, **beta / untested**)

1. Download `typeThai-<version>-windows-x86_64.zip` and unzip it. Keep `typethai-desktop.exe` and `typethai.ttm` in
   the same folder.
2. The exe is unsigned, so SmartScreen will warn: **More info → Run anyway**.
3. Add the Thai Kedmanee keyboard layout for switching (Settings → Time & language → Language & region).

## Using it

| Key | Action |
|---|---|
| **Tab** (bubble visible) | Apply the selected row |
| **Shift+↑ / Shift+↓** | Move the selection |
| **Esc** | Dismiss the bubble |
| **⌃⌥1 / ⌃⌥2 / ⌃⌥3** | Apply row 1 / 2 / 3 (Ctrl+Alt on Windows) |
| **⌃⌥Z** | Undo the last conversion |
| **⌃⌥X** | Force-convert the current or last word |
| **⌃⌥G** | Guess-convert: flip layout and apply the top typo suggestion in one step |
| **⌃⌥P** | Pause / resume (also in the tray menu) |
| **⌃⌥R** | AI rewrite of the selected text: pick *Fix typos* or *Translate TH↔EN*, Tab again to replace the selection |

The menu-bar icon also has **Settings…** (or launch with `--settings` when the icon is hidden behind the notch). From
there you can choose when word suggestions show (Off, After a pause, While typing), list words that stay as you type
them (`ok` is not offered as `นา`) and review or reset what typeThai has learned.

**AI rewrite setup.** Open **Settings… → AI rewrite**, paste an API key (an [OpenRouter](https://openrouter.ai) key works
with the defaults) and press **Save key**. The key is kept in the macOS Keychain or Windows Credential Manager, never in
a file. Model and base URL can point at any OpenAI-compatible `/chat/completions` server; plain `http://` is accepted
only for `localhost` (for example Ollama at `http://localhost:11434/v1`). **Instructions** adds your own text to the
prompt for both actions, for example a preferred tone. Esc, typing, a click elsewhere or switching apps cancels a
rewrite at any step.

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
| `--simulate` | No key capture: read lines from stdin and print what would happen (try `l;ylfu <Tab>`; `<Idle>` is a typing pause) |
| `--list-input-sources` | List the enabled input sources (read-only) |
| `--probe-caret --secs N` | Diagnostic: log where the caret is detected and mark it on screen |
| `--demo-bubble [--items "a\|b"] [--screenshot PNG]` | Show the bubble with sample rows (no key capture) |
| `--demo-settings [--page N] [--search TEXT] [--light] [--screenshot PNG]` | Show only the Settings window with sample words (no key capture, prefs or keychain) |

## Privacy

- **Conversion and learning run locally.** Your typing is never uploaded.
- **AI rewrite is the one exception, and only when you press ⌃⌥R and pick an action.** The selected text is sent to
  the service set in Settings (OpenRouter by default, with its data collection opt-out set). Nothing is sent without a
  key, and the feature can be turned off in Settings.
- To work, typeThai reads your keystrokes system-wide. It keeps only the current word and line in memory, and drops
  them when you move the caret or switch apps. Password fields are skipped.
- The personal model is stored on your machine:
  - macOS: `~/Library/Application Support/typethai/personal.json`
  - Windows: `%APPDATA%\typethai\personal.json`

  It holds small weight adjustments plus the words you undid or converted manually. Delete the file to reset learning.
- Words you asked not to translate, and the word-suggestion setting, are stored next to it, in `prefs.json`. Deleting
  `personal.json` leaves them in place.

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
gh release download model-v1 -R phasu21/typethai -p typethai.ttm -D target/release
./target/release/typethai-desktop --simulate <<< 'l;ylfu <Tab>'
```

macOS app bundle: `scripts/package-macos.sh <target-triple> <version> path/to/typethai.ttm`.

Run from a checkout on macOS: `scripts/typethai.sh` builds the release binary, stops any running typeThai (including
the installed app) and starts it in the background (`stop`, `status`, `log`, `fg` also work). Link it onto your PATH to
start it with one word:

```sh
ln -sf "$PWD/scripts/typethai.sh" ~/.local/bin/typethai
typethai          # build + (re)start; log in ~/Library/Logs/typethai.log
```

The TypeScript prototype (`src/`, `scripts/`, `test/`, Node ≥ 23.6) holds the training and evaluation pipeline. The
corpora are not included; see `DATA_LICENSES.md`. `docs/` holds the manual test checklists.

## Licenses and data attribution

- Code: **MIT**, © 2026 phasu21 (`LICENSE`).
- Model (`typethai.ttm`): derived from Thai and English **Wikipedia** (CC BY-SA 4.0), the **PyThaiNLP** Thai word
  list (CC0), the **Wisesight Sentiment** corpus (CC0) and the **FrequencyWords** English frequency list (CC BY-SA
  4.0). It is distributed under CC BY-SA 4.0.
- Evaluation-only data (not in the model): Wongnai reviews (LGPL-3.0) and the Thai Toxicity Tweet Corpus
  (CC BY-NC 3.0).

Details are in [`DATA_LICENSES.md`](DATA_LICENSES.md).
