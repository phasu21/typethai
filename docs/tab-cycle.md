# Tab cycle (prototype)

Status: prototype behind a setting, off by default. macOS tested in the simulator and unit tests; Windows shares the
engine and is wired, but is untested.

## Why

People found the current ways to pick an option (⌃⌥1–3, Shift+arrows, Tab then Space) too fiddly. Tab cycle puts the
options straight into the text, one per Tab tap, so you see each one where it will end up and just keep typing when
the right one is there.

## Setting

Settings > Word suggestions > **Tab key: Tab cycle**. In `prefs.json`: `"tab_key": "cycle"`.

It is a fourth choice of the existing *Tab key* setting rather than a separate switch, because that setting already
decides what a Tab tap over the bubble does; a second switch would need rules for every combination. Older versions
read the unknown value as the default (*Space applies, Tab moves*).

## How it feels

- The bubble shows options as usual. Nothing changes until you tap Tab.
- **Tab** puts option 1 into the text in place of what you typed. The bubble stays up, highlights that option and its
  keycap reads `1/3` instead of `Tab`.
- **Tab** again swaps in option 2, then 3, then your own typing again (the keycap says `Tab` once more), then option
  1 again. **Shift+Tab** goes the other way. Shift+Up/Down do the same as Shift+Tab/Tab.
- **Keep typing** (a letter, Space, Enter, punctuation): the option on screen is kept and the key is typed after it,
  as if you had picked it. If the option is in the other language, the input source switches now, and the key you
  pressed is typed in the new layout.
- **Esc** (or ⌃⌥Z while cycling) puts back exactly what you typed and closes the bubble.
- **⌃⌥1–3**, Tab+1–3 or a click on a row keeps that row, even mid-cycle.
- After a commit, **⌃⌥Z** undoes it as today and switches the input source back if the commit switched it.
- The text in the other app cannot be styled (underlined) from outside it; the bubble's highlight and `k/n` keycap
  are the only marks that the text is still provisional.

## State machine

```
            Tab (bubble up, some row can be put in the text)
  idle ───────────────────────────────────────────────▶ cycling(0)
   ▲                                                      │  Tab / Shift+Tab
   │ Esc, ⌃⌥Z: text put back, bubble closed               ▼
   ├──────────────────────────────────────────────── cycling(k) ◀──┐
   │                                                      │ │       │ Tab past the last row
   │ focus lost / click / pause toggle: text stays as     │ └──▶ cycling(typed) ─┘ (your typing back)
   │ shown, nothing else happens                          │
   │                                                      │ any other key, ⌃⌥1–3, click on a row
   │                                                      ▼
   └──────────────────────────────────────────────── committed: the row is applied as today's Tab would
                                                       (learning, Undo, input switch), the key goes on as usual
```

- `idle`: no cycle. Keys behave as in *Highlight, then Space* except that the first Tab tap starts the cycle.
- `cycling(k)` (`Engine::cycle`, `Cycle::at = Some(k)`): row `k`'s text is on screen in place of the original. The
  engine's own view of the line (buffer, committed words) is **not** changed, so going back is exact.
  `cycling(typed)` (`at = None`) is the same with the original text on screen; a key there just ends the cycle.
- `committed`: the engine applies row `k` through the normal accept path (`on_hotkey(Accept(k))`, or
  `accept_rolled` for a typed key), so learning, the rank log, auto-convert's undo memory and Undo all behave as
  today. That apply computes the same edit that is already on screen; it is cut down to what remains (usually
  nothing, or the key just typed). If the two ever disagree, the original text is put back first and the apply runs
  on it, so the screen can never drift from the engine.

### How each Tab edits the text

When the cycle starts, every row is turned into an edit: the text it replaces (ending at the caret) and the text it
puts there (`Engine::row_edit`, the same text the row's accept path computes). The longest replaced text is the
*window*. Moving from one row to another is one edit from "window with row A applied" to "window with row B
applied", made with the usual Backspaces + insert and trimmed to what differs (`backspace::edit`), so each Tab is a
single, short, atomic replacement through the same `Action::Replace` the existing apply uses. Rows whose edit does not
end at the caret, or that change nothing, are skipped by Tab (the digits still pick them). If no row can be put in
the text (for example the ⌃⌥R rewrite rows), Tab falls back to highlighting, as *Highlight, then Space*.

### Input source

The input source does **not** change while cycling, only when a conversion is committed. Switching on every Tab
would flip the keyboard back and forth while the user is only looking, and an Esc would then have to switch back;
the key that commits is re-typed in the new layout anyway (the same as today's rolled Tab), so nothing is lost by
waiting.

## Edge cases

- **Password field**: a key that types still passes the platform's field checks while cycling; landing in a secure
  field resets the engine (the option stays as shown, nothing is typed by typeThai).
- **Caret moved / mouse click / focus lost / app switch**: the platform resets the engine. The text stays as shown
  (it can no longer be put back safely: the caret may be elsewhere), the cycle ends, and the input source is not
  switched. Arrow keys while cycling commit the shown option first, then move the caret.
- **Bubble refreshing mid-cycle**: while cycling, nothing replaces the rows: the pause timer (completions,
  predictions, next words, the hold-Tab rows) does nothing, and a blocked-word refresh waits. The rows the cycle
  started with stay until it ends.
- **Rapid Tab presses**: every tap is a full edit from the current state to the next, so taps can come as fast as
  the keyboard repeats; holding Tab does not spin through options (auto-repeat is swallowed, as today) and holding it
  alone does not open the ⌃⌥R rows while cycling. Tab+R still does: it keeps the shown option, then opens them.
- **Typing right after Tab** (Tab rolled into the next key): the Tab counts as released first, so the option goes in
  and the key commits it in the same event.
- **Multi-word phrases**: a phrase row (a reverse phrase, a Guess, ⌃⌥R's earlier-words flip) replaces several words
  at once; its edit reaches back over them and the window covers it. Undo after the commit restores the whole phrase,
  as today.
- **Thai with no spaces**: a Thai completion keeps the head of the run and replaces only the tail, the same text its
  accept would insert.
- **Auto-convert**: a word auto-converted at Space shows the undo and the typo fixes; Tab cycles them, so Tab once
  puts the typed English back (provisionally), and typing on keeps it (and remembers the undo, as ⌃⌥Z would).
- **Space rule (#51)**: Space never applies anything you did not put in the text yourself. With Tab cycle, Space only
  keeps what a Tab already put on screen; after tabbing back to the typed word, Space is a plain space even with
  *Accept with: Tab or Space*.
- **Setting off**: nothing changes; every existing Tab key choice behaves exactly as before.

## Not done in the prototype

- No inline marking of the provisional text (other apps' text cannot be styled from outside).
- Shift+Tab does not start a cycle (it stays the app's reverse focus key until a Tab tap started one).
- Windows is wired but untested.
