**typeThai** fixes Thai typed on the wrong keyboard layout, in both directions, from a suggestion bubble at your caret.

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
- `typethai.ttm`: the model on its own (already bundled in each package). It now carries English word frequencies for
  completion. CC BY-SA 4.0, see DATA_LICENSES.md

**macOS:** the app isn't notarized. Right-click → Open, or run `xattr -dr com.apple.quarantine typeThai.app`. Then grant
Accessibility and relaunch. Add the Thai (Kedmanee) input source to get automatic layout switching.
**Windows:** SmartScreen will warn (More info → Run anyway).

**How it works:** nothing changes until you press **Tab**. Shift+↑/↓ select a row, Esc dismisses, ⌃⌥1–3 apply a row,
⌃⌥Z undoes. Typing correction runs locally. AI rewrite is the one exception: the selected text goes to the service set
in Settings, and only after you press ⌃⌥R and pick an action. Password fields are skipped.

This is a beta. Very short slang and abbreviations are often missed, and English words inside Thai chat sometimes get a
needless bubble.
