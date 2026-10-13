First public beta of **typeThai**: it fixes Thai typed on the wrong keyboard layout, in both directions, from a
suggestion bubble at your caret.

**Downloads**
- `typeThai-*-macos-aarch64.zip`: Apple silicon
- `typeThai-*-macos-x86_64.zip`: Intel Macs
- `typeThai-*-windows-x86_64.zip`: Windows (**untested beta**)
- `typethai.ttm`: the model on its own (already bundled in each package). CC BY-SA 4.0, see DATA_LICENSES.md

**macOS:** the app isn't notarized. Right-click → Open, or run `xattr -dr com.apple.quarantine typeThai.app`. Then grant
Accessibility and relaunch. Add the Thai (Kedmanee) input source to get automatic layout switching.
**Windows:** SmartScreen will warn (More info → Run anyway).

**How it works:** nothing changes until you press **Tab**. Shift+↑/↓ select a row, Esc dismisses, ⌃⌥1–3 apply a row,
⌃⌥Z undoes. Everything runs locally. Password fields are skipped.

This is a beta. Very short slang and abbreviations are often missed, and English words inside Thai chat sometimes get a
needless bubble.
