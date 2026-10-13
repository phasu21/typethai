//! Headless simulation (any OS): feed stdin lines as keystrokes through the engine and print the resulting actions and
//! the text the target field would end up with. Hotkeys: `^Z` undo, `^X` force-convert, `^1`..`^3` accept, `^P` pause.
//! Input source: `<TH>` / `<EN>` set the active layout. Thai chars in the input stand for their Kedmanee keys (shown
//! as Thai while Thai is active, as the US letter after a switch to ABC); the sim follows `SwitchInput`.
//! Editing: `<BS>` Backspace, `<A-BS>` Option/Ctrl+Backspace (word), `<C-BS>` Cmd+Backspace (line).
//! Bubble keys: `<Tab>` (or a literal tab), `<S-Up>`, `<S-Down>`, `<Esc>`. Each line also prints `[bubble: ...]` state
//! after keys that change it, with `>` marking the selected row.
use crate::{app::{hotkey_for, App}, engine::{Action, Boundary, Key}};
use std::io::BufRead;
pub fn apply(screen: &mut Vec<char>, acts: &[Action], swallowed: bool, typed: Option<char>) {
    if !swallowed { match typed {
        Some('\u{8}') => { screen.pop(); }                                                     // Backspace
        Some('\u{17}') => { while screen.last().is_some_and(|c| c.is_whitespace()) { screen.pop(); } while screen.last().is_some_and(|c| !c.is_whitespace()) { screen.pop(); } } // word delete
        Some('\u{15}') => { while screen.last().is_some_and(|c| *c != '\n') { screen.pop(); } } // line delete
        Some(c) => screen.push(c), None => {}
    } }
    for a in acts { if let Action::Replace { delete, insert, then } = a {
        for _ in 0..*delete { screen.pop(); } screen.extend(insert.chars()); if let Some(b) = then { screen.extend(b.text().chars()) }
    } }
}
pub fn run(mut app: App) {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap(); let mut screen: Vec<char> = vec![]; let mut bubble: Vec<String> = vec![]; let ch: Vec<char> = line.chars().collect(); let mut i = 0;
        while i < ch.len() {
            let rest: String = ch[i..].iter().collect();
            let tok = ["<Tab>", "<S-Up>", "<S-Down>", "<Esc>", "<TH>", "<EN>", "<BS>", "<A-BS>", "<C-BS>"].into_iter().find(|t| rest.starts_with(t));
            let (k, typed) = if let Some(t) = tok {
                i += t.chars().count() - 1;
                if t == "<TH>" || t == "<EN>" { app.engine.set_thai_active(t == "<TH>"); println!("  [input source: {}]", if t == "<TH>" { "Thai" } else { "ABC" }); i += 1; continue }
                match t { "<Tab>" => (Key::Tab, Some('\t')), "<BS>" => (Key::Backspace, Some('\u{8}')), "<A-BS>" => (Key::WordDelete, Some('\u{17}')), "<C-BS>" => (Key::LineDelete, Some('\u{15}')), "<S-Up>" => (Key::SelectPrev, None), "<S-Down>" => (Key::SelectNext, None), _ => (Key::Esc, None) }
            } else if ch[i] == '^' && i + 1 < ch.len() { i += 1; (Key::Hotkey(hotkey_for(ch[i]).expect("hotkey")), None) }
                else { match ch[i] { ' ' => (Key::Boundary(Boundary::Space), Some(' ')), '\t' => (Key::Tab, Some('\t')), c => match typethai_core::reverse::th_text_to_keys(&c.to_string()).filter(|_| typethai_core::keymap::is_thai(c)) {
                    // a Thai char in the input = that physical key; the screen shows the Thai char only while Thai is
                    // active (after a switch to ABC the same keys produce the English letters, as for a real user)
                    Some(k) => { let k = k.chars().next().unwrap(); (Key::Char(k), Some(if app.engine.thai_active() { c } else { k })) }
                    None => (Key::Char(c), Some(c)),
                } } };
            let o = app.engine.on_key(k.clone());
            for a in &o.actions { println!("  {k:?} -> {a:?}"); }
            if o.swallow && o.actions.is_empty() { println!("  {k:?} -> (swallowed)"); }
            for a in &o.actions { if let Action::SwitchInput(l) = a { app.engine.set_thai_active(*l == crate::engine::InputLang::Thai); } }
            for a in &o.actions { match a {
                Action::ShowBubble(items) => bubble = items.iter().map(|b| b.label.clone()).collect(),
                Action::HideBubble => bubble.clear(), _ => {}
            } }
            if o.actions.iter().any(|a| matches!(a, Action::ShowBubble(_) | Action::Select(_))) {
                let sel = app.engine.selected();
                println!("  [bubble: {}]", bubble.iter().enumerate().map(|(j, l)| format!("{}{l}", if j == sel { ">" } else { " " })).collect::<Vec<_>>().join(" | "));
            }
            apply(&mut screen, &o.actions, o.swallow, typed); i += 1;
        }
        let o = app.engine.on_key(Key::Boundary(Boundary::Enter)); apply(&mut screen, &o.actions, o.swallow, None);
        for a in &o.actions { println!("  Enter -> {a:?}"); }
        println!("{}", screen.iter().collect::<String>()); app.maybe_save();
    }
    app.save();
}
