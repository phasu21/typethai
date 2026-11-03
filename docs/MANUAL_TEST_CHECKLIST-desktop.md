> **Tab from 0.5.24.** While a word is being typed, Space applies the highlighted option and a Tab tap moves to the
> next (Settings → Word suggestions → Tab key: *Space applies, Tab moves*, the default). Where an older section below
> says "Tab applies / replaces / inserts", run it with **Tab key: Accept right away**, or with the default read it as
> "press Space" mid-word, and "tap Tab, then press Space" elsewhere (Enter where a space must not be typed). See "Space
> applies, Tab moves", "Tab cycles, Space confirms" and "Esc closes the bubble".


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

## Space applies, Tab moves (default from 0.5.24; engine tests only, not run live by the agent)
Key map with Settings → Word suggestions → **Tab key: Space applies, Tab moves** (default, prefs.json `"tab_key":
"space_applies"`): mid-word, with an option that finishes or fixes the word highlighted (its key hint `␣`), Space = apply
it and type the space · Tab tap = move the highlight to the next option (first tap: option 2), wrapping · Shift+Tab =
previous (after a Tab tap) · Space / Enter / ↑ after a tap = apply the highlight · Esc = close the bubble, the next Space is
a space · hold Tab + 1..3 = apply that option · hold Tab = the ⌃⌥R options, never applies.
Space is a plain space when: no bubble; after Esc; the word typed is already a whole word (`help` with helpful up); the
option is the word as typed; next-word options (after `ok ` and a pause, or a Thai run ending on a word with the next
word offered); the ⌃⌥R options (select text, hold Tab, let go, Space types over the selection).
- [ ] Type `hel` and pause: help / hello / helmet, the first with `␣` on it (strip and rows). Press Space: `help `, one
  space, the bubble gone; the next Space is a space.
- [ ] Type `hel`, pause, tap Tab: hello is highlighted (no tab typed, text unchanged); tap again: helmet; again: help
  (wraps). Shift+Tab goes back. Press Space: that option, then one space. Enter or ↑ instead: no space or newline.
- [ ] Type `hel`, pause, Esc: the bubble goes; Space types `hel `.
- [ ] Type `help` (whole word) while helpful is offered: Space types `help `; Tab, Space gives `helpful `.
- [ ] Wrong layout: type `l;ylfu` (live สวัสดี up): Space gives `สวัสดี ` with one space and the input source changes to
  Thai; ⌃⌥Z restores `l;ylfu `.
- [ ] Thai: type `ภาษ` on the Thai layout, pause: ภาษา highlighted with `␣`; Space gives `ภาษา `.
- [ ] Next word: type `ok `, pause: the next words show with `Tab` (not `␣`) on the first; Space types a space. Tab
  highlights the first, Space inserts it with no extra space.
- [ ] Select text, hold Tab, let go: the options show `Tab`; Space types over the selection. Tap Tab (option 1), Space
  pastes it.
- [ ] Typing a letter, ↓ after a tap, a click elsewhere or switching apps: nothing applied, the bubble follows or goes.
- [ ] Settings → Word suggestions → Tab key lists Space applies, Tab moves / Highlight, then Space / Accept right away;
  the page description changes with each. Quit and relaunch: kept. A prefs.json from 0.5.23 (`"tab_cycles": true`)
  opens on Space applies, Tab moves; `"tab_cycles": false` opens on Accept right away.
- [ ] Windows (Notepad, a browser): the same checks.

## Tab cycles, Space confirms (default in 0.5.23, now Tab key: Highlight, then Space; engine tests and `--simulate` only, not run live by the agent)
Key map with Settings → Word suggestions → **Tab key: Highlight, then Space** (prefs.json `"tab_key": "highlight"`):
Tab tap = highlight (first tap: the highlighted option, usually 1; each further tap: the next, wrapping) · Shift+Tab =
previous (only after a Tab tap) · Space / Enter / ↑ = apply the highlight · ←/→ = move it · ↓, any other key, a click
elsewhere, switching apps = drop it, no change · Esc = close the bubble, no change · hold Tab + 1..3 = apply that option ·
hold Tab = the ⌃⌥R options, never applies · click = apply that option.
- [ ] Type `hel` and pause: help / hello / helmet. Tap Tab: nothing changes in the text, no tab is typed, and the first
  option's key hint turns into `␣` (strip and rows). Tap Tab twice more: the highlight moves to hello, then helmet; a
  fourth tap wraps to help. Shift+Tab goes back (help → helmet → hello).
- [ ] With hello highlighted press Space: the text is `hello ` (one space), the bubble is gone, the next Space is a space.
- [ ] Without tapping Tab first, Space over the same bubble is just a space: `hel ` and nothing applied. Shift+Tab with
  no Tab tap reaches the app (reverse focus in a form).
- [ ] After a Tab tap, Enter applies with no newline and no space; ↑ applies too; ← / → move the highlight.
- [ ] After a Tab tap, typing a letter drops the highlight and types the letter (`help` stays `help` + letter, nothing
  applied); ↓ drops the highlight and keeps the bubble; clicking elsewhere or switching apps hides it with no change.
- [ ] Wrong layout: type `l;ylfu`, tap Tab, Space: `สวัสดี ` with one space, the input source changes to Thai, ⌃⌥Z
  restores `l;ylfu `. After `l;ylfu ` (space already typed) tap Tab and Space: `สวัสดี ` still with one space.
- [ ] Next word: type `ok ` and pause; tap Tab, Space: the next word is inserted, no extra space added.
- [ ] Roll Tab straight into Space (Space down before Tab is up): the first option is applied, then the space. Roll Tab
  into a letter: nothing applied, the letter is typed.
- [ ] Hold Tab + 2 still applies option 2 at once (also after a Tab tap); holding Tab with a word bubble up still opens
  the ⌃⌥R options and applies nothing.
- [ ] Settings → Word suggestions → Tab key: **Accept right away**: a Tab tap applies the highlighted option as in
  0.5.22, Space is a space (or, with Accept with: Tab or Space, applies a live conversion as before). The page's
  description changes with it. Quit and relaunch: the choice is kept; a prefs.json from 0.5.22 opens as Space applies, Tab moves.
- [ ] Guess: Tab Tab with no bubble shows the guess; tap Tab and press Space to apply it.
- [ ] Windows (Notepad, a browser): the same checks; Shift+Tab with no bubble still moves focus backwards.

## Esc closes the bubble (0.5.23; engine tests only, not run live by the agent)
- [ ] With a word bubble up (nothing highlighted) press Esc: the bubble goes, the text is unchanged, and the app does
  NOT get the Esc (a dialog or a search field stays open). Press Esc again: now the app gets it.
- [ ] Same after a Tab tap (an option highlighted), and on the ⌃⌥R options (select text, hold Tab, let go, Esc).
- [ ] After Esc, wait a few seconds without typing: the bubble does not come back for the same word (also on the Thai
  layout with English typed, e.g. `าืนไ` → know). Type the next letter: suggestions may show again.
- [ ] Windows: the same checks.

## Tab+digit (not run live by the agent; engine tests and --simulate only)
- [ ] With 3 rows up, hold Tab and press 2: row 2 is applied, and the app receives neither the Tab nor the 2.
- [ ] Hold Tab and press 1: option 1 is applied (with Tab key: Accept right away, the same as tapping Tab).
- [ ] Hold Tab past the key-repeat delay, then release: the highlighted row is applied once and no tabs are typed.
- [ ] With a 1-row bubble, hold Tab and press 3: nothing happens and the bubble stays; releasing Tab does nothing.
- [ ] Tab key: Accept right away — roll Tab straight into Space or the next letter (press it before Tab is up): the row is applied first, then the key, in that order. After a Thai row the letter comes out as Thai. (Default Space applies, Tab moves, and Highlight, then Space: see the sections above.)
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
- [ ] With a bubble visible: a Tab tap moves or sets the highlight (default) or, with Tab key: Accept right away, applies the highlighted row when released; either way the app does NOT receive the Tab.
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
- [ ] Bubble disappears when typing the next word, on Left / Right / Down or a click; Up moves into it, then Left / Right pick (Pick with: Arrow keys, the default).
- [ ] `--probe-caret --secs 20` (from a terminal that has Accessibility): red marker on the caret in TextEdit, Notes, Safari, Chrome, Slack, VS Code, Discord, Cursor; log says source=caret.
- [ ] Bubble's first-row text starts at the caret x; follows the caret while typing without flicker; flips above near the bottom; external monitor.
- [ ] v13.1: Backspace mid-word updates the bubble without flicker; Backspace over the space re-enters the word (bubble stays); ⌥⌫ drops one word of a reverse phrase; ⌘⌫ closes.

## AI rewrite (⌃⌥R / Ctrl+Alt+R). The agent ran only the macOS TextEdit capture and paste path, not the hotkey in the app.
- [ ] Settings → AI rewrite: Save key with an OpenRouter key shows "Key saved in the Keychain"; reopening Settings still shows it; Save key with an empty field shows "No key saved yet".
- [ ] Base URL `http://example.com/v1` + Save shows the inline error and saves nothing; `http://localhost:11434/v1` is accepted. Turn off: ⌃⌥R reaches the app untouched.
- [ ] No key: ⌃⌥R on a selection shows "Add an API key in Settings". Nothing selected: "No selected text found".
- [ ] TextEdit: select `helo wrld`, ⌃⌥R shows the spelling fix and the layout swap, then "Fix typos" / "Translate TH↔EN"; highlight "Fix typos" (Shift+Down, or Up then →) and Tab shows "Rewriting…", then the result; Tab replaces the selection. ⌘V afterwards pastes what was on the clipboard before.
- [ ] Translate: a Thai sentence becomes English and back; no ครับ/ค่ะ is added to a casual sentence.
- [ ] Instructions `Answer in UPPERCASE.` + Save: Fix typos and Translate both return uppercase text; reopening Settings still shows the instructions; clearing them + Save goes back to normal output.
- [ ] Chrome (a textarea and a contenteditable such as Gmail compose), and Slack or VS Code: the rows appear and Tab replaces the selection. VS Code with nothing selected says "No selected text found" and the clipboard is unchanged (no whole-line copy).
- [ ] Terminal (clipboard fallback when Accessibility gives no selection): the selected output is read and the clipboard is restored afterwards.
- [ ] A password field (Safari or Chrome login) with its text selected: ⌃⌥R shows nothing.
- [ ] Cancel: Esc, typing a key, a click elsewhere, or switching apps at the preset rows, while "Rewriting…", and at the result: the bubble closes, the text is unchanged, and a late result never appears.
- [ ] A clipboard manager (Maccy, Raycast, Paste) does not record the pasted result.
- [ ] Windows Notepad and Edge: select text, Ctrl+Alt+R, Tab, Tab replaces it; the menu bar is not activated by the Alt release; Win+V history has no entry for the result; the previous clipboard comes back.

## Finish sentence locally switch (agent-verified with --demo-settings and engine tests; not run live)
- [ ] Settings → AI rewrite → Local: a "Finish sentence locally" switch above the model, off on a new install and after
  updating from a version without it (prefs.json `"rewrite"` → `"local_finish": false`).
- [ ] Off: select text and hold Tab: no "Finish sentence locally" row, even with Ollama running and the model downloaded.
  Fix typos / Translate still show when a key is saved.
- [ ] On, Ollama running with the model: the row shows. Quit Ollama (or pick a model not downloaded): it goes again.
- [ ] Quit and relaunch: the switch keeps its state; Save on the Cloud tab does not turn it off.

## Select text, then hold Tab to rewrite (engine tests and `--demo-bubble` only; not run live by the agent)
- [ ] Always the layout swap: select `l;ylfu` and hold Tab: สวัสดี is offered first. Select `สวัสดี`: `l;ylfu`. Select
  gibberish such as `asdfjkl;` or `qwpzxv`, or Thai no dictionary knows: the whole selection re-typed on the other
  layout is still offered, with the flag of the language it becomes. Select `l;ylfu world`: สวัสดี world first, then
  every word swapped. Select `helo wrld` (with the spell checker): the fix first, then the swap, then the AI rows.
  Select only numbers (`1234`): no swap row.
- [ ] The rail: with Settings → bubble layout on the strip (the default), the rewrite options show as the same
  horizontal rail as word suggestions, at the caret / selection, with Tab on the highlighted button and 2, 3 on the
  next ones. A long selection's buttons end in "…" and Tab still pastes the whole text. With the setting on rows, they
  stack as before.
- [ ] Arrow keys on the rail: Up goes into it, ← / → move the highlight, Enter or Up again applies; Shift+Down/Up also
  move it, and Down leaves it.
- [ ] Holding Tab only opens the rows: select text and keep Tab down well past the key-repeat delay (2–3 s): the rows
  open once and stay; nothing is pasted, nothing flickers or reopens, and no tab is typed. Let Tab go: still nothing
  applied.
- [ ] Confirm: after letting go, Enter applies the highlighted row; so do Up-into-the-rail then Up, Ctrl+1..3 (when on)
  and a click on a button. A fresh Tab tap highlights (the hint turns into `␣`), further taps move along, Shift+Tab
  goes back, and Space or Enter pastes; Space without a Tab tap types over the selection (rows go, nothing pasted).
  With Tab key: Accept right away, a fresh Tab tap applies the highlighted row as before.
- [ ] Cancel: Esc (also while Tab is still down), typing, a click elsewhere or switching apps: the rows go and the text
  is unchanged.
- [ ] Windows (Notepad, Edge): the same checks.

## Local sentence completion (⌃⌥R → *Finish sentence locally*; exercised in `--simulate` only, see `docs/research/llm-sentence-completion-in-app.md`)
- [ ] With no API key saved and **Offer AI rewrite** on, ⌃⌥R on a selection shows *Finish sentence locally* (and no *Fix typos* / *Translate TH↔EN*); with **Offer AI rewrite** off it is gone too.
- [ ] Settings → AI rewrite: the **Local** / **Cloud** tabs switch the rows below them (Local: model + setup; Cloud: API key, Model, Base URL, Instructions, Save); the switch and Hold Tab rows stay above and apply to both.
- [ ] Settings → AI rewrite → **Local model** offers Gemma 4 8B (default) and Typhoon 2.5 4B; picking one saves at once (`prefs.json` `"rewrite": {"local_model": …}`) and **Local model setup** re-checks.
- [ ] Same selection, two models: `It was great catching up with you. Let's keep in` gives `…touch!` with Gemma and `…touch.` with the 4B (both were measured through the settings). A hand-edited `local_model` with a space in it falls back to the default instead of breaking the row.
- [ ] Hovering an ⓘ on the AI rewrite page opens its card with the row's explanation, and moving off closes it (the one thing the agent could not drive: synthetic hover does not reach the app).
- [ ] The **Local model setup** button is the state: `Checking…` (spinning) → `Get Ollama` / `Start Ollama` / `Download model` → `Downloading NN%` (spinning, percentage from the pull) → `Ready` (disabled); a failed pull leaves `Try again` with the error under the title.
- [ ] Ollama installed but not running: **Set up** starts it (menu-bar app on macOS) and finishes the download; the row ends at "… is ready".
- [ ] Ollama not installed (or `PATH` and the install dirs empty): the row says "Ollama is not installed" and the button is **Get Ollama**, which opens ollama.com/download.
- [ ] Settings → AI rewrite shows the Ollama note on **Offer AI rewrite**; the Instructions text does not change the local result.
- [ ] TextEdit: select `Please let me`, ⌃⌥R, *Finish sentence locally* shows "Finishing locally…" and then a continuation; Tab replaces the selection (⌘Z in the app restores the previous text).
- [ ] Real selections: a partial Thai word (`ขอแสดงความยิน`), a mixed line (`Before merging, ช่วยเช็กว่า`), and a text needing a leading space (`…Let's keep in`) compose into one sentence with no doubled or glued words.
- [ ] A trap: select `My bank account number is` — the bubble says "The local model has no suggestion for this selection" and nothing is inserted.
- [ ] Ollama stopped: the same row says "Ollama is not running. Start it, or press Set up in Settings → AI rewrite" and nothing is inserted; starting Ollama and picking the row again works without a restart.
- [ ] Cold model (after `ollama stop gemma4:latest`): the first request takes seconds; the bubble stays on "Finishing locally…" and the app keeps taking keys.
- [ ] Esc, typing a key, a click elsewhere or switching apps while "Finishing locally…": the bubble closes, the text is unchanged and a late result never appears.
- [ ] Chrome/Slack/a terminal: the same read-and-paste path as the cloud rows replaces the selection, and the previous clipboard comes back.
- [ ] Windows: the same rows, with Ollama on the same machine; not run on Windows yet.

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
- [ ] Next-word prediction (on by default, `prefs.json` `"predict"`): type the same short line three or four times, ending each with Enter — e.g. `ship the zqorbit` then Enter, twice. On a new line type `zqor` and stop: the bubble offers `zqorbit` (a word no dictionary has, learned from the lines you finished) as the first row, and Tab completes it to `zqorbit`. The same works in Thai for a phrase you type: type `สวัสดีครับ` + Enter, then on a new line type `สวัสดีครั` and stop — the row keeps the whole run (`สวัสดีครับ`).
- [ ] Next-word prediction off (Settings → Word suggestions → Predict the next word): the rows above disappear, and `<state dir>/ngram.snapshot` is not touched at all (no file is written while it stays off). Turning it back on brings the rows back from what is already in the snapshot.
- [ ] Forget learned words: the predicted rows stop being offered (the model was cleared with `words.json`), and typing a line again re-learns it.
- [ ] While typing (not only after a pause) the predicted row can appear one keystroke after the one that asked for it — it must never move under your fingers: keep typing into a predicted word and the rest of it stays visible until what you type diverges from it.
- [ ] `--simulate` (no model needed for the row itself, but the model file is): `printf '<EN>ship the zqorbit\n<EN>ship the zqorbit\n<EN>ship the zqor<Idle><Idle>\n' | typethai-desktop --simulate --state /tmp/tt-smoke/personal.json` prints a `[bubble: >zqorbit | …]` line.
- [ ] Rank log: `printf '<EN>l;ylfu <Tab>\n<EN>l;y <C-1>\n' | typethai-desktop --simulate --state /tmp/tt-smoke/personal.json` (with `{"pick_with_ctrl":true,"tab_key":"accept"}` in `prefs.json` beside it), then `typethai-desktop --rank-report --state /tmp/tt-smoke/personal.json`: `/tmp/tt-smoke/rank.jsonl` holds one line per word the bubble offered, each with the rows that were on it and the row picked (`"pick":{"i":0,"how":"tab"}` and `"how":"digit"`), and the report says how often the first row was picked, with the reflex keys split out. Nothing is logged with **Learn from my typing** off, and **Forget learned words** deletes `rank.jsonl`.

## Horizontal bubble (agent-verified with --demo-bubble --screenshot on the Mac, dark and light; not run live)
- [x] `--demo-bubble --items "ภาษา|ภาษี|ภาษาอังกฤษ" --screenshot x.png`: one strip of three buttons, an outlined `Tab` keycap on the first and filled `2` / `3` on the others; the self-check lines say OK and the panel matches the layout.
- [x] `--select 1`: the `Tab` keycap moves to the second button, which grows (34 px tall, 16 px label) while the others are 30 px tall and dimmed, all on the same bottom edge and at their slot's width, so every gap between buttons is the same; no box around the buttons, no edge moves sideways and the strip keeps its width. `--light` looks right.
- [x] One option over 240 px (`--items "สวัสดีครับ|ภาษาไทย ทดสอบ วรรณยุกต์ ก่ ก้ ก๊ ก๋ ภาษาอังกฤษ|l;ylfu"`) stacks the bubble as rows. `--vertical` stacks short options too.
- [ ] Live, After a pause: type `hel` and stop. The strip appears with the first label at the caret's x; keep typing `m`: the options update in place without a flash or a jump, and the strip narrows from its left edge.
- [ ] Next word, model beside pairs: type a line with a rare word pair (a name, jargon) and finish it with Enter twice
  or three times, then type the first word and pause: the pair rows come from the built-in list and the last row is the
  model's own follower. Settings → *Finish words from my typing* off: the model's row goes, the built-in rows stay.
- [ ] Next word, your habits: type `ok cool` (Space between, Enter after) twice, then `ok ` and pause. The strip shows
  นา first (Tab applies it), then your followers of `ok` best first; with four options the fourth has no digit
  keycap, Up then arrows or a click picks it. Settings → *Learn from my typing* off: only the built-in pairs remain.
- [ ] Click each button, or just above a smaller one: that option is applied. Click the gap between two buttons: the click reaches the app, as anywhere outside the buttons.
- [ ] Shift+↓ / Shift+↑ move the highlight along the strip; clicks still land on the right button afterwards.
- [ ] Thumbs: type `ok ` (นา offered) and hold Shift: นา's key hint turns into 👍 👎, and the other options keep theirs. Release Shift: the key hint is back. Shift held, click 👎: นา moves behind the next words and Tab applies the first of them; `prefs.json` lists `ok` under `offer_last`. Type `ok ` again: นา shows alone, then lands last after the pause, however many 👎. 👍 puts it back in front. Without Shift a click there applies the option.
- [ ] The strip sits above the caret line (rows stay below); at the top of the screen it drops below the caret; near the right edge it stays on screen.
- [ ] ⌃⌥R on a selection, and AI rewrite presets, "Rewriting…" and the result, are always rows.
- [ ] Settings → Word suggestions → Bubble layout: Vertical gives the old rows on the next bubble and survives a restart (`prefs.json` has `"bubble_layout": "vertical"`); an older prefs.json opens as Horizontal.
- [ ] Windows (Notepad, 100% and 150% scaling): the strip window is exactly the panel, nothing is clipped, and clicks hit the right button.

## Accessibility permission self-repair (agent-verified with TYPETHAI_FAKE_UNTRUSTED; the live revoke is not run)
- [x] `TYPETHAI_FAKE_UNTRUSTED=1 typethai-desktop --model …`: the app stays up and logs `no event tap yet` (a refused tap used to quit the app), and the tray shows *Grant Accessibility…*.
- [ ] Installed app, fresh machine: the record is cleared once per version (`permission-repair` beside `prefs.json`), the prompt appears, and granting it starts correction without relaunching.
- [ ] Permission revoked while running: the tap keeps working for that session (macOS does not kill it), and the next launch repairs and re-prompts once.
- [ ] Tray reads `Accessibility: on` once granted, with the tooltip no longer saying the permission is needed.

## The bubble goes when focus leaves the field (agent-verified with --simulate; the live look at focus is not run)
- [x] `--simulate`: `<Focus>hel<Idle><Blur>` prints `ShowBubble(...)` after the pause and `HideBubble` on `<Blur>`.
- [ ] Type `hel` in TextEdit and wait for the rows. Then three-finger swipe to another Space or app, or let another app
  come forward by itself (a notification you click, a dialog): the bubble goes at once, not on the next key.
- [ ] Type `hel` in a chat box (Messages, Slack), wait for the rows, then press Enter so the app sends and moves focus
  off the box: the bubble goes within ~40 ms.
- [ ] Type `hel`, wait for the rows, then close the window with the mouse on its red button, or let the app move focus
  away on its own: the bubble goes within about a second.
- [ ] Open Settings from the tray while the bubble is up: the bubble goes.
- [ ] Normal typing is unchanged: the rows still come after the pause, Tab / the arrows / a click on a row still apply
  them, and a bubble left up in a field you are still in stays (no flicker every second).
- [ ] Windows (Notepad, a browser): the same, including Alt+Tab and a toast taking focus.

## Input-language preview on focus (agent-verified with --demo-badge --pin; the live look at focus is not run)
- [x] `--demo-badge --pin --secs 2` keeps the pill up for the whole 2 s (`still up … true`), where `--demo-badge --secs 2` hides it after `SHOW_FOR` (1.2 s).
- [x] `--simulate`: `ab<Focus>x<Idle>y<Idle><Blur>` prints `Badge(true)` on `<Focus>`, `Badge(false)` on the key, `Badge(true)` after the pause; inside a bubble the pause shows completions instead and no `Badge`.
- [ ] Make sure focus is in a text field (TextEdit, Safari search box, a chat box) and click into it: the pill shows over the caret within ~40 ms and stays. It must sit clear of the caret line and of macOS's own input-source indicator.
- [ ] Type: the pill goes on the first keystroke, and does not come back while typing.
- [ ] Stop typing for ~2 s while still in the field: the pill comes back and stays until the next keystroke.
- [ ] Settings → Word suggestions → Show it again after: pick 5 s, stop typing in the same field: the pill stays away until ~5 s. Quit and relaunch: the choice is kept (prefs.json `"badge_idle_ms": 5000`), and an older prefs.json opens at 2 s.
- [ ] Click on a page, a canvas or another app: the pill goes (no pill where there is no text field, and none in a password field).
- [ ] Clicking a second field in the same window moves it to the new caret.
- [ ] With the pill up, click into another text field (another app too): the pill jumps to that caret within ~a tenth of a second, without a flash.
- [ ] In a terminal (TUI app), press Enter while the pill is up: it goes on the key and comes back after the pause, on the *new* prompt line. `typethai log` prints `typethai: preview anchor ...` whenever the pill moves: compare its caret rect with the real one if a pill still reads as misplaced.
- [ ] The pill never takes focus and clicks pass through it (macOS).
- [ ] ⌃⌥P (or the tray menu) while the pill is up: it goes, and does not come back until typeThai is unpaused.
- [ ] Windows (Notepad): the same, including a click from the browser into Notepad and back.

## Download chip on the bubble (not run live; `--demo-bubble --update` only so far)
- [x] `--demo-bubble --update --screenshot PNG` (and `--light`): the chip rides at the end of the strip's suggestions,
  standing on the rail's floor like the buttons, the download icon in the accent colour; the run also exercises the
  chip's hover tip the way a hover does and writes a `PNG-tip.png` beside the shot. No chip on the vertical rows
  (`--vertical`), none without `--update`.
- [ ] Hovering the chip shows the tip under it — *Update to version x.y.z* (or *Update typeThai* before a version is
  known) — and the tip ignores the mouse: it takes no focus, and a click on it is swallowed instead of reaching the
  tip. It follows the chip's screen spot, flips above when the screen ends too soon below, and goes away when the
  bubble moves, hides, or the chip does.
- [ ] With a real update in progress (publish a release newer than the running one, or point `TYPETHAI_UPDATE_URL` at
  a fake `releases/latest`): while the checker is downloading, staging or waiting to be opened by hand, the chip
  appears on the next bubble; clicking it does what the tray's update item does right then — restart into the staged
  update, start it going, or open the release page. With nothing to act on (checking, all current), no chip.
- [ ] The chip never takes focus, and clicks elsewhere reach the app as usual.

## Off in chosen apps (agent-verified on the Mac with System Events keystrokes into TextEdit; Windows not run)
- [x] Tray shows *Turn off in TextEdit* after a key in TextEdit; clicking it writes `"off_apps":[{"id":"com.apple.textedit","name":"TextEdit"}]` and the item becomes *Turn on in TextEdit*.
- [x] With TextEdit off, `l;ylfu ` + Tab types `l;ylfu ⇥` unchanged: no bubble, no language preview. *Turn on in TextEdit* and the same keys give `สวัสดี `.
- [x] `--demo-settings --page 5`: the Apps page lists the apps with their ids and a Remove button; search `code` finds it.
- [ ] Settings → Apps → Remove on a live entry turns that app back on without a restart.
- [ ] Switch to an off app with the bubble or the preview up: it goes at once.
- [ ] Windows: *Turn off in ‹exe›* names the window in front, never the taskbar after clicking the tray icon.

## Windows caret anchoring (0.5.19; agent-verified by logs and the app's own anchor, not by eye)
- [ ] Notepad: type a word; the bubble sits on the caret (its first label at the caret's x) and follows it as you type.
- [ ] Select a word and keep typing: the bubble stays on the caret (at the selection's end), not at the selection's edge.
- [ ] An app that shows no caret (VS Code's inline prompt): the bubble appears at the pointer — where you clicked — and
  not at the far left of the text field.
- [ ] The bubble never jumps to the mouse or to the field's edge while typing, and it does not jump when a read is slow.
- [ ] Two monitors: the bubble stays on the caret's monitor (the caret is read per monitor, in physical pixels).
