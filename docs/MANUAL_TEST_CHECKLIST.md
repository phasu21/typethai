# typeThai desktop: manual test checklist

Nothing in the native key-capture paths has run on real Windows or macOS yet. The Windows exe was driven once under
Wine 10 on Linux (Notepad, xdotool). This checklist is for real machines. Mark each item pass/fail and add notes (app, OS version).

## Setup
- [ ] Build the model pack: `cd rust && cargo run --release --bin tt-pack -- ../data/model typethai.ttm`. Put `typethai.ttm` next to the exe, or pass `--model`.
- [ ] Windows: run `typethai-desktop.exe`. Use a debug build to get console logs, and set `TYPETHAI_DEBUG=1` for per-key logs.
- [ ] macOS: run the binary. On first run it prompts for **Accessibility**. Grant it in System Settings → Privacy & Security → Accessibility (and Input Monitoring if asked), then relaunch.
- [ ] A tray / menu-bar icon (blue "T") appears with "Pause typeThai" and "Quit".

## Core auto-convert (English layout active)
- [ ] Type `l;ylfu ` in Notepad / TextEdit. It becomes `สวัสดี ` and the bubble appears near the caret.
- [ ] Type `hello world `. Nothing changes.
- [ ] Type `l;ylfu` then press **Enter** in a chat app (LINE, Slack, Discord, Teams). The message goes out as `สวัสดี`, not as `l;ylfu`. Enter is swallowed and re-sent after the conversion.
- [ ] **Tab** as the boundary converts, and focus then moves on.
- [ ] Mixed line `ihbjk iPhone 15 ` (`ihbjk` = ทำงาน). Only the Thai token converts. Brand, model and number stay as typed.
- [ ] Shift / CapsLock letters (`L;YLFU`) behave the same as on the TS engine. Compare with `--simulate`.
- [ ] Fast typing (>10 keys/s) right after a conversion: no characters land in the wrong order or get lost.
- [ ] Apps to try: Notepad, Word, Chrome (textarea and contenteditable, e.g. Gmail), Firefox, VS Code (Electron), Terminal / Windows Terminal, LINE, Slack, Telegram. macOS: TextEdit, Notes, Safari, Chrome, Pages, iTerm.
- [ ] Backspace count: in each app above, the original text is removed exactly. Watch for apps whose Backspace deletes a whole grapheme cluster. This matters for **undo**, which deletes Thai text (e.g. `สวัสดี` is 6 code points).

## Buffer reset
- [ ] Type `l;y`, click elsewhere in the same field, then type `lfu `. No conversion, because the buffer was reset by the click.
- [ ] Type `l;y`, Alt-Tab / Cmd-Tab to another app and back, then type `lfu `. No conversion.
- [ ] Arrow keys, Home/End, PgUp/PgDn, Delete and Esc reset the buffer.
- [ ] Ctrl/Cmd shortcuts (Ctrl+C, Cmd+V) reset the buffer and are never swallowed.
- [ ] Known gap on macOS: focus changes *within one app* (tab between two fields in a browser) only reset on a click or Tab. Same-pid focus change is not detected.

## Secure fields
- [ ] Windows: in a classic password box (e.g. a Win32 login dialog) type `l;ylfu `. No conversion and no bubble.
- [ ] Windows: in a browser `<input type=password>` (UIA IsPassword) type `l;ylfu `. No conversion.
- [ ] macOS: in a Safari/Chrome password field (secure input or AXSecureTextField) type `l;ylfu `. No conversion.
- [ ] macOS: Terminal with "Secure Keyboard Entry" enabled. No conversion.

## Thai layout already active
- [ ] Switch the OS layout to Thai Kedmanee and type normally. typeThai does nothing.

## Undo / force-convert / learning
- [ ] After an auto-conversion press **Ctrl+Alt+Z** (macOS **⌃⌥Z**). The original `l;ylfu ` is restored.
- [ ] Repeat undo on the same word about 3 times in new lines. The word stops auto-converting (personal memory). On a fresh state it converted.
- [ ] Type `hello ` then **Ctrl+Alt+X**. It is converted to Thai, and repeated force-converts teach typeThai to convert that token.
- [ ] Mid-word Ctrl+Alt+X (`l;yl` then the hotkey) converts the partial word.
- [ ] The personal state persists: `%APPDATA%\typethai\personal.json` / `~/Library/Application Support/typethai/personal.json` is written every 10 feedback events and on Quit, and is reloaded on restart.
- [ ] AltGr layouts (e.g. Polish, German) on Windows: AltGr = Ctrl+Alt. Check that AltGr+Z / AltGr+X don't trigger undo or force-convert unexpectedly.

## Suggestion bubble
- [ ] It appears at the caret in Notepad (GetGUIThreadInfo), Chrome / Word (UIA TextPattern2) and TextEdit / Safari (AXBoundsForRange).
- [ ] Where no caret is available (some Electron apps, terminals), it appears near the mouse pointer.
- [ ] It never takes focus: typing continues into the app while it is visible.
- [ ] It stays on top of full-screen apps (macOS Spaces / full screen; collection behaviour is not set yet, so this may fail).
- [ ] Thai renders correctly. Windows uses the "Leelawadee UI" font; under Wine it showed tofu boxes because that font is missing there.
- [ ] Low-confidence token (classifier probability 50–95%): the bubble shows `สวัสดี? (85%)`. **Ctrl+Alt+1** or a click converts it.
- [ ] Typo suggestion after a conversion (e.g. `l;yfu` → `สวสดี`): bubble rows 2–3 show corrector suggestions, and Ctrl+Alt+2/3 or a click replaces the word.
- [ ] Clicking the bubble does not reset the buffer or focus the bubble. Clicking elsewhere hides it on the next key.
- [ ] Multi-monitor and HiDPI (150% / Retina): the bubble sits next to the caret, not offset.

## Tray
- [ ] Pause: the icon turns grey and nothing converts. Ctrl+Alt+P toggles the same state.
- [ ] Quit: the process exits and the personal state is saved.

## Optional layout switch (`--switch-layout`)
- [ ] Windows, with Thai Kedmanee installed: after a conversion the layout of the foreground window switches to Thai.
- [ ] macOS, with the Thai input source enabled: after a conversion the input source switches to Thai.

## Performance / robustness
- [ ] Startup takes about 1 s (corrector index build). Memory is about 85 MB after load. Check Task Manager / Activity Monitor.
- [ ] Windows: no "hook timeout" (the hook is silently removed if a callback takes more than about 300 ms). Typing still converts after 10 minutes of use.
- [ ] macOS: if the tap is disabled by timeout it re-enables itself (TapDisabledByTimeout handler).
