**typeThai** fixes Thai typed on the wrong keyboard layout, in both directions, from a suggestion bubble at your caret.

**New in 0.5.18**
- **A notification when an update is ready.** When typeThai finds a release and stages it, it says so with a system
  notification — *typeThai x.y.z is ready · Click to install it and start typeThai again* — so you no longer have to
  open the tray to find out. Clicking the notification is exactly what clicking the tray item does. A copy that has to
  be replaced by hand (*… is available*) opens the release page instead. One notification per release, none for a
  check that found nothing. macOS asks for notification permission the first time there is something to say.
- **Updates are checked when the computer wakes.** A laptop that slept through a release finds it on the next wake
  instead of waiting up to six hours — at most one check an hour, and only while the automatic check is on.

**Fixed in 0.5.18**
- Update checks no longer use GitHub's API, which allows 60 requests an hour per address and answers a rate-limited
  check with an error. The check now reads the tag GitHub redirects `releases/latest` to, so it cannot be throttled,
  and it costs one small request instead of a release document.

**New in 0.5.17**
- **Fewer weak word suggestions.** When you pause and the best completion is only a small share of the plausible words
  (under 15%), the bubble now stays hidden instead of showing three weak guesses. A clear favourite still shows the
  usual three choices. The local suggestion log now records how confident each row was and when a popup was suppressed,
  so the cutoff can be tuned from real use.

**New in 0.5.15**
- **A download chip on the suggestion bubble.** When there is something to act on (a staged release, one being
  downloaded, or one waiting to be opened by hand), a small download icon rides at the end of the bubble's
  suggestions: a click is what the tray's update item does right then — restart into the update, start the
  download, or open the release page. Hovering it says which release it is on its way to (*Update to version
  x.y.z*). It never takes focus, and neither does its tip.

**Fixed in 0.5.15**
- **Windows: the bubble now sits at the caret in Chromium and Electron apps** (Chrome, Edge, Slack, VS Code, Grok
  Bot, …) instead of at the mouse or the edge of the text field, and it keeps to the caret as you type. Those apps
  answer neither the Windows caret API nor the UI Automation caret query typeThai used before; their caret is read
  the way it already was on macOS, and it is re-read after each key while the bubble is up.

**New in 0.5.14**
- **Turn typeThai off in one app.** The menu-bar icon has *Turn off in ‹app›* for the app in front: there typeThai
  shows nothing, converts nothing and every key, hotkeys included, reaches the app. *Turn on in ‹app›* undoes it, and
  Settings → *Apps* lists the apps and removes them.

**New in 0.5.13**
- **Your own next words, also beside นา.** After a word and a pause, the words *you* usually type next come first
  (any you typed after that word at least twice, best first), then the built-in pairs. They now also show when the
  word just typed is offered in Thai: `ok ` → นา | lets | done | cool. The fourth option has no digit; reach it with
  the arrow keys or a click.
- **👍 / 👎 on a conversion.** Hold Shift over the bubble: the key hint of an option offered in Thai turns into two
  thumbs. 👎 moves that conversion behind every other option, now and each time the word comes back (it is still
  offered); 👍 puts it back in front.
- **Finish sentence locally.** ⌃⌥R on selected text offers *Finish sentence locally*: a local Ollama model (Gemma 4 8B
  by default, or the faster Typhoon 2.5 4B) continues it, with no key and nothing uploaded. Review it and press Tab.
  Settings → AI rewrite → *Local model setup* starts Ollama and downloads the model for you.
- **A tidier strip.** It sits above the caret line (below only at the top of the screen). The highlighted option is
  larger and the others dimmed, all on one bottom edge with even gaps and no box around them. Why an option is offered
  (🇹🇭 / 🇺🇸 or the spelling mark) is a faint mark inside its button.
- **The arrow keys wait for ↑ again.** ← and → move the cursor until ↑ moves into the bubble, as in 0.5.8; then ←/→
  choose and ↑ or Enter applies.

**New in 0.5.12**
- **The Accessibility permission repairs itself.** If typeThai starts without it, it no longer quits: it keeps running
  (the tray shows *Grant Accessibility…*), clears its own Accessibility record once per version so an older copy's
  record can no longer answer for this build, asks again, and starts working the moment you grant it — no relaunch.
  That is the case where the switch looks on in System Settings and nothing works, and every launch asks again.

**New in 0.5.11**
- **Mistyped English on the Thai layout is offered as what you meant.** With Thai active, English typed with a slip is
  offered back as the corrected word, not the raw keys: the row now carries the spelling the spell checker (macOS) or
  the completer's keyboard-aware distance picks (`know` typed as `าืนไ` roughly). A word one keyboard slip away is
  offered even when the raw keys are not a known word, so long as the model reads the keys as English at all.
- **The next word now draws on your own typing.** The *Suggest the next word* rows still lead with the built-in word
  pairs; the personal model (`Finish words from my typing`, renamed from *Predict the next word*) adds the word it
  expects to follow when it is not already one of them — a name, jargon or a habit of yours the built-in pairs do not
  know. Both sources are one feature with one switch (*Suggest the next word*); the model's own switch decides whether
  the model is used at all.

**Fixed in 0.5.11**
- Windows: the mouse no longer stutters while typeThai is running, in games above all. The mouse hook now has a thread
  of its own, and nothing it does waits on typing, suggestions or a look at the focused field.

**New in 0.5.10**
- **The input language shows before you type.** Focus a text field and the 🇹🇭 ไทย / 🇺🇸 EN flag sits above the caret and
  stays until your first key, so the language you are about to type is in sight; it comes back on its own after a pause
  in typing. Settings → Word suggestions → *Show it again after* chooses the pause (1 to 10 seconds, 2 by default).
  As before, it keeps out of password fields, non-text fields and the bubble's way.
- **Finish words from my typing.** Beside the dictionary completions, a small model of *your own* typing finishes the
  word you are typing, in Thai or English, and can offer words the built-in lists do not have (a name, jargon, or a word
  you misspell the same way twice). It learns from the lines you finish and stays on your machine. Settings → Word
  suggestions → *Finish words from my typing* turns it off; *Forget learned words* clears it. It also adds the word it
  expects next to the *Suggest the next word* rows (0.5.9), beside the built-in pairs.

**Fixed in 0.5.10**
- The input-language flag is placed where the caret really is. Right after Enter in a terminal it used to land a line
  off (the caret is read before the app lays the new line out), and it stayed on the old spot when you moved to another
  text field. It now waits for the caret to settle and follows it.

**New in 0.5.9**
- **Next word.** Pause after a word and the bubble offers the word that usually follows it: after a space in English
  (`thank ` → you), and in Thai as soon as a word ends, no space needed (`แซ่บ` → มาก, `ขอบคุณ` → ค่ะ). Only a clear
  favourite is offered. Settings → Word suggestions → *Suggest the next word*.
- **The arrow keys pick right away.** With the bubble up, ←/→ choose without pressing ↑ first; Enter or ↑ applies.
- **One "Pick with" setting** (Arrow keys, Ctrl+1–3, or both) replaces the two switches. Your saved choice is kept.
- **Model v4**: Thai word pairs now include Thai tweets (VISTEC-TP-TH-2021), so chat-style Thai is completed better.

**Fixed in 0.5.9**
- A Thai suggestion with many upper and lower marks (`ประวัติศาสตร์`) no longer breaks onto a second line outside
  its button.

**New in 0.5.8**
- **Pick with arrow keys is on by default.** With the bubble up, ↑ moves into it, ←/→ choose, ↑ again or Enter
  applies; until ↑ the arrow keys move the cursor as before.
- **Faster by default**: suggestions after a 50 ms pause and the ⌃⌥R options after holding Tab for 200 ms, the
  quickest choices in Settings. Settings you have already saved keep their values.
- **English typed on the Thai layout is offered more often.** A known English word of 4+ letters (`stab`, typed as
  หะฟิ) is offered after a pause or at Space even when it is not certain enough to show while you type.

**Fixed in 0.5.8**
- macOS: suggestions no longer come up in apps built on Electron or Chrome when no text field has focus (a settings
  or welcome page), or on the Finder desktop.

**New in 0.5.7**
- **Input language at the caret.** When the input source switches, by you or by typeThai after a conversion, a small
  flag with ไทย or EN shows just above the caret for a moment. Settings → Word suggestions → *Show the input language*.
- **Holding Tab shows its progress.** Held past a tap, a *Rewrite* pill (or, over the bubble, the Tab key) fills until
  the ⌃⌥R options open; let go early and it is just a Tab.
- **Choose how long to hold Tab**: Settings → AI rewrite → *Hold Tab for*, from 200 ms to 1 s (500 ms by default).
- **Quicker suggestions**: *Pause before suggesting* now also offers 50 and 100 ms.

**Fixed in 0.5.7**
- Holding Tab for the ⌃⌥R options typed a Tab first, so applying a fix could replace the wrong letters (`nw` became
  `ew` instead of `new`).

**Fixed in 0.5.6**
- Suggestions show again in Electron apps (terminals, chat and editor apps built on Electron) when they hide their
  text fields from macOS: 0.5.5 took that for "no text field" and offered nothing.
- When an app does not tell macOS where the cursor is, the bubble appears where you last clicked in that window
  instead of at the mouse pointer.

**New in 0.5.5**
- **Pick with the arrow keys.** Turn on Settings → Word suggestions → *Pick with arrow keys*: while the bubble is up,
  press Up to move into it, Left or Right to choose, and Up again or Enter to apply. Down or Esc leaves. Until you press
  Up, the arrow keys move the cursor as usual.
- **Hold Tab for the ⌃⌥R options**, on the selected text or the word before the caret. A quick tap still types a Tab,
  sent when you release it; holding Tab no longer repeats it. ⌃⌥R and Tab+R still work.
- **Keys on the corner.** In the strip each button's key sits on its top-left corner, in your accent colour on the
  highlighted option. The strip no longer shifts when the highlight moves.

**Fixed in 0.5.5**
- The bubble no longer pops up in games and other windows without a text field: typeThai now checks that you are
  typing into a text field before it offers anything.

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
