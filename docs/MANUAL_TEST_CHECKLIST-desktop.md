
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

## Tab+digit (not run live by the agent; engine tests and --simulate only)
- [ ] With 3 rows up, hold Tab and press 2: row 2 is applied, and the app receives neither the Tab nor the 2.
- [ ] Hold Tab and press 1: same result as tapping Tab.
- [ ] Hold Tab past the key-repeat delay, then release: the highlighted row is applied once and no tabs are typed.
- [ ] With a 1-row bubble, hold Tab and press 3: nothing happens and the bubble stays; releasing Tab does nothing.
- [ ] Roll Tab straight into Space or the next letter (press it before Tab is up): the row is applied first, then the key, in that order. After a Thai row the letter comes out as Thai.
- [ ] With no bubble, hold Tab and press 2: a tab and then 2 are typed, as before.
- [ ] Windows (Notepad): the same checks.

## Space accept (not run live by the agent; engine tests and --simulate only)
- [ ] Default (Accept with: Tab): type `l;ylfu` and Space with the live row up: the space is typed and nothing converts; the bubble then offers สวัสดี as before.
- [ ] Settings → Word suggestions → Accept with: Tab or Space. Type `l;ylfu` + Space: the text becomes `สวัสดี ` with one space, and the input source changes to Thai. ⌃⌥Z restores `l;ylfu `.
- [ ] Same setting, Thai Kedmanee active: type `าืนไ` (know) + Space: it becomes `know ` and the input source changes to ABC.
- [ ] Same setting, English completions up (`hel` and a pause): Space types a space and applies nothing.
- [ ] Same setting, live row with completions under it (`ภาษ` typed on ABC): Shift+Down to a completion, then Space: a plain space, nothing applied.
- [ ] Same setting: Tab, Tab held + 1..3, and Space rolled over a held Tab still apply rows as before.
- [ ] Quit and relaunch: the setting is kept. A prefs.json from an older version opens as Tab.
- [ ] Windows (Notepad): the same checks.

## v10 bubble keys / input source / live bubble (not run live by the agent)
- [ ] With a bubble visible: Tab applies the highlighted row when released, and the app does NOT receive the Tab.
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
- [ ] Bubble disappears when typing the next word, on Left / Right / Down or a click; Up moves into it (Pick with arrow keys, on by default).
- [ ] `--probe-caret --secs 20` (from a terminal that has Accessibility): red marker on the caret in TextEdit, Notes, Safari, Chrome, Slack, VS Code, Discord, Cursor; log says source=caret.
- [ ] Bubble's first-row text starts at the caret x; follows the caret while typing without flicker; flips above near the bottom; external monitor.
- [ ] v13.1: Backspace mid-word updates the bubble without flicker; Backspace over the space re-enters the word (bubble stays); ⌥⌫ drops one word of a reverse phrase; ⌘⌫ closes.

## AI rewrite (⌃⌥R / Ctrl+Alt+R). The agent ran only the macOS TextEdit capture and paste path, not the hotkey in the app.
- [ ] Settings → AI rewrite: Save key with an OpenRouter key shows "Key saved in the Keychain"; reopening Settings still shows it; Save key with an empty field shows "No key saved yet".
- [ ] Base URL `http://example.com/v1` + Save shows the inline error and saves nothing; `http://localhost:11434/v1` is accepted. Turn off: ⌃⌥R reaches the app untouched.
- [ ] No key: ⌃⌥R on a selection shows "Add an API key in Settings". Nothing selected: "No selected text found".
- [ ] TextEdit: select `helo wrld`, ⌃⌥R shows "Fix typos" / "Translate TH↔EN"; Tab shows "Rewriting…", then the result; Tab replaces the selection. ⌘V afterwards pastes what was on the clipboard before.
- [ ] Translate: a Thai sentence becomes English and back; no ครับ/ค่ะ is added to a casual sentence.
- [ ] Instructions `Answer in UPPERCASE.` + Save: Fix typos and Translate both return uppercase text; reopening Settings still shows the instructions; clearing them + Save goes back to normal output.
- [ ] Chrome (a textarea and a contenteditable such as Gmail compose), and Slack or VS Code: the rows appear and Tab replaces the selection. VS Code with nothing selected says "No selected text found" and the clipboard is unchanged (no whole-line copy).
- [ ] Terminal (clipboard fallback when Accessibility gives no selection): the selected output is read and the clipboard is restored afterwards.
- [ ] A password field (Safari or Chrome login) with its text selected: ⌃⌥R shows nothing.
- [ ] Cancel: Esc, typing a key, a click elsewhere, or switching apps at the preset rows, while "Rewriting…", and at the result: the bubble closes, the text is unchanged, and a late result never appears.
- [ ] A clipboard manager (Maccy, Raycast, Paste) does not record the pasted result.
- [ ] Windows Notepad and Edge: select text, Ctrl+Alt+R, Tab, Tab replaces it; the menu bar is not activated by the Alt release; Win+V history has no entry for the result; the previous clipboard comes back.

## Word completion (needs a model with English frequencies for English rows; not run live by the agent)
- [ ] Settings → Word suggestions shows Off / After a pause / While typing, with the current one highlighted; a click switches it and survives a restart (`prefs.json` has `"completion"`).
- [ ] After a pause (default): type `hel` and stop. The bubble shows help / hello / hell at the caret; Tab inserts the highlighted word. At the 50 ms default it may come up between slower keys; at 250 ms or more, typing on without stopping shows nothing.
- [ ] Settings → Word suggestions → Pause before suggesting (shown only for After a pause) offers 50 / 100 / 150 / 250 / 400 / 600 ms, 50 by default. Pick 600 and the bubble waits visibly longer; at 50 it comes up as soon as typing stops. The choice survives a restart (`prefs.json` has `"complete_idle_ms"`). Switching to While typing or Off hides the row. Windows: the 150 ms step is not smoothed by the old 100 ms tick (the timer now ticks every 25 ms).
- [ ] After a pause, wrong layout: type the keys for `ภาษ` on ABC (`4kK`). The live row ภาษ is first, ภาษา / ภาษี appear under it after the pause; Shift+↓ then Tab inserts ภาษา and switches the input source to Thai. ⌃⌥Z gives back `4kK`.
- [ ] After a pause, Thai layout: type ฉันชอบภาษ and stop. Rows show ภาษา / ภาษี / ภาษาอังกฤษ; Tab leaves ฉันชอบภาษา.
- [ ] While typing: `beca` shows because without a pause; common whole words (`you`, `not`, ฉัน) do not pop a bubble; typing feels no slower.
- [ ] Off: no completion rows in any of the above; the live conversion row and Space suggestions still work.
- [ ] Esc on a completion bubble hides it, and no completion comes back for that word even after a pause; the next word gets completions again.
- [ ] Space or Enter after a word never shows completions, only the usual conversion / spelling rows.

## Horizontal bubble (agent-verified with --demo-bubble --screenshot on the Mac, dark and light; not run live)
- [x] `--demo-bubble --items "ภาษา|ภาษี|ภาษาอังกฤษ" --screenshot x.png`: one strip of three buttons, an outlined `Tab` keycap on the first and filled `2` / `3` on the others; the self-check lines say OK and the panel matches the layout.
- [x] `--select 1`: the `Tab` keycap moves to the second button and the strip keeps its width. `--light` looks right.
- [x] One option over 240 px (`--items "สวัสดีครับ|ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋ ภาษาอังกฤษ|l;ylfu"`) stacks the bubble as rows. `--vertical` stacks short options too.
- [ ] Live, After a pause: type `hel` and stop. The strip appears with the first label at the caret's x; keep typing `m`: the options update in place without a flash or a jump, and the strip narrows from its left edge.
- [ ] Click each button: that option is applied. Click the padding or the gap between two buttons: nothing is applied, the click does not reach the app, and the bubble stays.
- [ ] Shift+↓ / Shift+↑ move the highlight along the strip; clicks still land on the right button afterwards.
- [ ] Near the bottom of the screen the strip flips above the caret; near the right edge it stays on screen.
- [ ] ⌃⌥R on a selection, and AI rewrite presets, "Rewriting…" and the result, are always rows.
- [ ] Settings → Word suggestions → Bubble layout: Vertical gives the old rows on the next bubble and survives a restart (`prefs.json` has `"bubble_layout": "vertical"`); an older prefs.json opens as Horizontal.
- [ ] Windows (Notepad, 100% and 150% scaling): the strip window is exactly the panel, nothing is clipped, and clicks hit the right button.
