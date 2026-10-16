**typeThai** fixes Thai typed on the wrong keyboard layout, in both directions, from a suggestion bubble at your caret.

**New in 0.5.4**
- **English suggestions read the sentence.** Word completions now take their first row from the system's own
  prediction, which looks at the words before: `I wear a hel` offers *helmet*, `can you hel` offers *help*. macOS uses
  its spell checker, which also knows the words you have taught it; Windows uses its text prediction. typeThai's own
  words fill the rows below. English typo fixes on macOS also read the sentence (`I like teh` offers *the*, *tea*).

**Fixed in 0.5.3**
- An update downloaded but not yet installed no longer hides newer ones: if a later version comes out first, the tray
  offers that one instead.

**New in 0.5.2**
- **Tab+R.** Hold Tab and press R for the ⌃⌥R options on the word you just typed, or on the bubble's word. ⌃⌥R stays
  for selected text. Tab right after a word now goes through when you release it; typing on quickly still types it.

**Fixed in 0.5.2**
- Windows: update checks and AI rewrite failed on every HTTPS request. **Windows users on 0.5.0 or 0.5.1: install
  0.5.2 by hand once** (download the setup below); later versions update themselves.

**New in 0.5.1**
- **นา offers ok.** `ok` typed on the Thai layout (นา) now offers *ok* at Space or Tab, even though นา is a Thai word.
  Words on the Don't translate list are offered back the same way.
- **Enter closes the bubble.** Enter ends the line and dismisses what was offered, like Esc. A ⌃⌥R fix still applies
  with Enter.

**Fixed in 0.5.1**
- Windows: the tray icon is shown on the taskbar the first time typeThai starts. Windows 11 used to keep it in the
  hidden icons, where it could not be found when that menu is turned off.

**New in 0.5.0**
- **Updates itself.** typeThai checks GitHub for a new release at launch and every 6 hours, downloads it in the
  background and checks its signature. The tray menu then shows **Restart to update to …**: nothing changes until you
  pick it. Check right away from the tray or Settings → Updates, where the automatic check can also be turned off.
  Only GitHub is contacted.
- **Proper installers.** macOS: one DMG for Apple silicon and Intel, drag typeThai to Applications. Windows: a setup
  that installs for your user only (no admin prompt) and can start typeThai when you sign in.
- **Signed with typeThai's own certificate** (macOS), so updates keep the Accessibility permission. Coming from 0.4.0
  or earlier, macOS asks for it once more: remove the old typeThai from Privacy & Security → Accessibility and add
  `/Applications/typeThai.app` again.

**New in 0.4.0**
- **New bubble.** Suggestions now appear as a row of buttons. Each one shows why it is offered: a 🇹🇭 / 🇺🇸 flag
  when the word was typed in the wrong layout, a red spelling mark for a typo fix. Click an option to apply it. The
  old stacked rows are still in Settings → Word suggestions → Bubble layout.
- **Typo-tolerant suggestions.** A slipped key no longer hides the word you meant: `candel` offers *candle*, and the
  word before decides between close guesses (`we must candel` offers *cancel*).
- **⌃⌥R fixes what you point at.** With text selected it offers the text in the other layout and with typos fixed,
  then AI rewrite if set up. With nothing selected it offers fixes for the word before the caret. Enter applies the
  highlighted fix.
- **Pick with Ctrl+1–3** (optional, Settings → Word suggestions).
- **Smoother bubble.** Options change in place while you type instead of fading out and back in.

**Fixed in 0.4.0**
- The bubble now appears at the caret in Electron and Chromium apps whose text fields hide it from the usual query
  (for example Grok Bot), instead of at the left edge of the field.
- Windows: no more dim box or thin line around the bubble.

**New in 0.3.1**
- **Noto font.** The bubble and Settings now use Noto Sans Thai with Noto Sans for Latin, bundled in the app, so Thai
  looks the same on macOS and Windows.

**Fixed in 0.3.1**
- Windows: the bubble was cut off on the right and bottom, and the key hints sat against its edge.

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
- `typeThai-*-macos-universal.dmg`: macOS, Apple silicon and Intel. Open it and drag typeThai to Applications
- `typeThai-*-windows-x86_64-setup.exe`: Windows installer (**untested beta**), per-user, optional start at sign-in
- `typeThai-*-windows-x86_64.zip`: the same Windows build without an installer
- `*.sig`, `typeThai-*-macos-universal.zip`: used by the in-app updater
- `typethai.ttm`: the model on its own (already bundled in each package). CC BY-SA 4.0, see DATA_LICENSES.md

**macOS:** the app is signed with typeThai's own certificate but isn't notarized. The first time, right-click → Open,
or run `xattr -dr com.apple.quarantine /Applications/typeThai.app`. Then grant Accessibility and relaunch. Coming from
0.4.0 or earlier: remove the old typeThai from Accessibility and add it again, once. Add the Thai (Kedmanee) input
source to get automatic layout switching.
**Windows:** SmartScreen will warn (More info → Run anyway).

**How it works:** nothing changes until you press **Tab**. Shift+↑/↓ select a row, Esc dismisses, ⌃⌥1–3 apply a row,
⌃⌥Z undoes. Typing correction runs locally. AI rewrite is the one exception: the selected text goes to the service set
in Settings, and only after you press ⌃⌥R and pick an action. Password fields are skipped.

This is a beta. Very short slang and abbreviations are often missed, and English words inside Thai chat sometimes get a
needless bubble.
