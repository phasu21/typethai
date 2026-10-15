
## v9 bubble / anchoring (untested by agent)
- [ ] `./typethai-desktop --demo-bubble --secs 4`: Thai renders correctly (vowels and tone marks), and the bubble follows light/dark mode.
- [ ] Live: the bubble appears below the caret in TextEdit, Safari and Notes, and flips above near the bottom of the screen and in chat boxes in the lower part.
- [ ] The bubble never steals focus, and typing continues while it is shown.
- [ ] Clicking a row accepts it, and the click doesn't reach the app underneath.
- [ ] Multi-monitor: the bubble stays on the caret's monitor, including an external display left or above. Retina is sharp.
- [ ] Apps without an AX caret (some Electron apps) → falls back to the mouse.
- [ ] Windows: same checks in Notepad and a browser, including mixed DPI.

## v9.1 bubble polish (fixes from the first --demo-bubble screenshot; untested on screen by agent)
- [ ] `--demo-bubble`: no system frame, empty rounded border or dark area outside the panel; the window is exactly the panel size, with rounded corners and a soft shadow only.
- [ ] The width fits the widest row, and a very long row ends in "…" instead of being clipped (demo row 3 should fit fully unless it is over the max width).
- [ ] All three labels start at the same x (shared column after the ⌃⌥N chips).
- [ ] The bottom-of-screen demo point still flips above the caret with the gap kept (placement now uses the measured size).
- [ ] Hovering or clicking a row still works, and the bubble still doesn't take focus.

## v9.2 (agent-verified via --demo-bubble --screenshot on the Mac; still check by eye)
- [x] 1, 2 and 3 rows and a long row: every row is fully visible with even padding, the window is exactly the panel, and the corners are transparent (screenshots in typethai-shots/).
- [x] A long row is cut with "…" at max width 560.
- [ ] The shadow is subtle and visible on a light background (the screenshots only show a dark background).
- [ ] Light mode looks right (only dark mode was captured).
- [ ] Placement is correct near the bottom of the screen with the real size (not captured).

## v10 bubble keys / input source / live bubble (not run live by the agent)
- [ ] With a bubble visible: Tab applies the highlighted row, and the app does NOT receive the Tab.
- [ ] With no bubble: Tab works normally (indent, focus change), and Shift+Up/Down still select text.
- [ ] Shift+Up/Down move the highlight (wrapping) and the app receives nothing. Esc closes the bubble and isn't passed to the app; with no bubble, Esc reaches the app.
- [ ] ⌃⌥1–3 still apply rows 1–3.
- [ ] After auto-convert or Tab-apply, the menu-bar input source changes to Thai. After undo, it changes back to ABC. `--no-switch-input` keeps it unchanged.
- [ ] After switching to Thai, the next Thai keys are not converted again (engine bypassed).
- [ ] Mid-word: after typing `l;yl` the bubble shows "สวัส…"; Tab replaces it, and the rest of the word types as Thai.
- [ ] English typing doesn't flash a bubble (eval: roughly 0.3–0.6 per 1,000 words).
- [ ] The highlight on the selected row is visible enough in light and dark modes (only dark was captured: typethai-shots/select-row2*.png).

## v11 reverse direction (not run live by the agent; restart the app after updating the binary)
- [ ] With Thai Kedmanee active, type `i dont know` + space: it becomes "i dont know" (the "i" flips once "dont" or "know" follows), and the input source changes to ABC.
- [ ] Tab on the undo bubble restores the Thai text and switches back to Thai.
- [ ] Normal Thai typing (chat, slang, 555-free text, typos) is never flipped. Report any false flip with the exact text.
- [ ] The live bubble shows an English word mid-word (e.g. `าืนไ` → "know…"), Tab applies it, and the rest of the word comes in as English.
- [ ] `--no-reverse`: Thai typing is ignored as before.

## v12
- [ ] The selected row shows a Tab `Kbd` on the right (other rows are text only); the highlight is clear in light and dark mode.
- [ ] ⌃⌥1/2/3 and Tab / Shift+Up/Down / Esc still work.
- [ ] A very long suggestion wraps (no "…"), bubble stays on screen, flips above the caret near the bottom edge.
- [ ] Clicking any line of a wrapped row picks that row.
- [ ] `--demo-bubble --screenshot` self-check lines all say OK (pad L = R).

## v13
- [ ] Space/Enter never change text (forward and reverse); the bubble shows for the finished word; Tab converts it, ⌃Z undoes.
- [ ] Reverse: type `i dont know` on the Thai layout + Enter -> bubble `i dont know`; Tab replaces all three words.
- [ ] Bubble disappears when typing the next word, on arrow keys or a click.
- [ ] `--probe-caret --secs 20` (from a terminal that has Accessibility): red marker on the caret in TextEdit, Notes, Safari, Chrome, Slack, VS Code, Discord, Cursor; log says source=caret.
- [ ] Bubble's first-row text starts at the caret x; follows the caret while typing without flicker; flips above near the bottom; external monitor.
- [ ] v13.1: Backspace mid-word updates the bubble without flicker; Backspace over the space re-enters the word (bubble stays); ⌥⌫ drops one word of a reverse phrase; ⌘⌫ closes.

## Word completion (needs a model with English frequencies for English rows; not run live by the agent)
- [ ] Settings → Word suggestions shows Off / After a pause / While typing, with the current one highlighted; a click switches it and survives a restart (`prefs.json` has `"completion"`).
- [ ] After a pause (default): type `hel` and stop. About half a second later the bubble shows help / hello / hell at the caret; Tab inserts the highlighted word. Typing on without stopping shows nothing.
- [ ] After a pause, wrong layout: type the keys for `ภาษ` on ABC (`4kK`). The live row ภาษ is first, ภาษา / ภาษี appear under it after the pause; Shift+↓ then Tab inserts ภาษา and switches the input source to Thai. ⌃⌥Z gives back `4kK`.
- [ ] After a pause, Thai layout: type ฉันชอบภาษ and stop. Rows show ภาษา / ภาษี / ภาษาอังกฤษ; Tab leaves ฉันชอบภาษา.
- [ ] While typing: `beca` shows because without a pause; common whole words (`you`, `not`, ฉัน) do not pop a bubble; typing feels no slower.
- [ ] Off: no completion rows in any of the above; the live conversion row and Space suggestions still work.
- [ ] Esc on a completion bubble hides it, and no completion comes back for that word even after a pause; the next word gets completions again.
- [ ] Space or Enter after a word never shows completions, only the usual conversion / spelling rows.
