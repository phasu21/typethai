**typeThai** fixes Thai typed on the wrong keyboard layout, in both directions, from a suggestion bubble at your caret.

**New in 0.3.0**
- **Smarter word suggestions.** Completions now look at the word before the one you are typing, so `I want to bu`
  offers *buy* first and `what time is it no` offers *now*. In held-out text, the right word is in the top 3 after two
  letters 80% of the time in Thai (was 65%) and 69% in English (was 48%).
- **Learns your words.** Words you finish rank higher over time, and a new word is offered once you have typed it
  three times. This stays on your computer in `words.json`. Turn it off or forget everything in Settings → Word
  suggestions.
- **Tab + 1, 2 or 3** applies that bubble row, and every row now shows its keys. Tab on its own still applies the
  highlighted row, now when you release it. Clicking a row applies it too.
- **Faster suggestions.** The pause before suggestions appear is now 250 ms (was 500 ms) and can be changed in
  Settings → Word suggestions → Pause before suggesting.
- **Space can accept (optional).** Settings → Word suggestions → Accept with: *Tab or Space* lets Space apply a word
  typed in the wrong layout (`l;ylfu` → สวัสดี), then type the space. Over completions Space stays a normal space.

**Fixed in 0.3.0**
- Applying a Thai suggestion in a terminal (for example Claude Code in Orca, Ghostty or Terminal) no longer deletes
  the letter or space before the word. Completions now type only the missing letters, and terminals get one Backspace
  per visible Thai letter.
- After accepting a suggestion, Backspace or more typing no longer starts the word from scratch.

**Fixed in 0.2.1**
- macOS: typeThai could quit on its own after a while. It read the keyboard input source from its key-capture
  thread, and macOS 26 stops apps that do that. Input-source reads and switches now happen on the main thread.

**New in 0.2.0**
- **AI rewrite (optional).** Select text and press **⌃⌥R** (Ctrl+Alt+R on Windows), then pick *Fix typos* or
  *Translate TH↔EN*. Tab shows the result, and Tab again replaces the selection. It uses your own API key with
  OpenRouter (default model `google/gemma-4-26b-a4b-it`) or any OpenAI-compatible server, Ollama included. Add the key in
  Settings → AI rewrite. It is stored in the macOS Keychain or Windows Credential Manager.
- **Word completion.** The bubble can finish the word you are typing, in Thai or English. Choose Off, After a pause
  (default) or While typing in Settings → Word suggestions.
- **English typo suggestions** for words left as typed.
- **New Settings window** with a sidebar and search. Launch with `--settings` to open it when the menu-bar icon is
  hidden behind the notch.

**Downloads**
- `typeThai-*-macos-aarch64.zip`: Apple silicon
- `typeThai-*-macos-x86_64.zip`: Intel Macs
- `typeThai-*-windows-x86_64.zip`: Windows (**untested beta**)
- `typethai.ttm`: the model on its own (already bundled in each package). It now carries word pair counts for
  context-aware completion. CC BY-SA 4.0, see DATA_LICENSES.md

**macOS:** the app isn't notarized. Right-click → Open, or run `xattr -dr com.apple.quarantine typeThai.app`. Then grant
Accessibility and relaunch. Add the Thai (Kedmanee) input source to get automatic layout switching.
**Windows:** SmartScreen will warn (More info → Run anyway).

**How it works:** nothing changes until you press **Tab**. Shift+↑/↓ select a row, Esc dismisses, ⌃⌥1–3 apply a row,
⌃⌥Z undoes. Typing correction runs locally. AI rewrite is the one exception: the selected text goes to the service set
in Settings, and only after you press ⌃⌥R and pick an action. Password fields are skipped.

This is a beta. Very short slang and abbreviations are often missed, and English words inside Thai chat sometimes get a
needless bubble.
