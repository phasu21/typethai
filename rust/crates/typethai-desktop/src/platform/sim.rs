//! Headless simulation (any OS): feed stdin lines as keystrokes through the engine and print the resulting actions and
//! the text the target field would end up with. Hotkeys: `^Z` undo, `^X` force-convert, `^1`..`^3` accept, `^P` pause.
//! Input source: `<TH>` / `<EN>` set the active layout. Thai chars in the input stand for their Kedmanee keys (shown
//! as Thai while Thai is active, as the US letter after a switch to ABC); the sim follows `SwitchInput`.
//! Focus: `<Focus>` / `<Blur>`: focus is in a text field (the input-language preview shows and stays) or left it. Any
//! other token is a keystroke, so it takes the preview down like a real key does.
//! Editing: `<BS>` Backspace, `<A-BS>` Option/Ctrl+Backspace (word), `<C-BS>` Cmd+Backspace (line).
//! Bubble keys: `<Tab>` (or a literal tab) presses and releases Tab, `<TabDown>` / `<TabUp>` hold and release it (a digit
//! 1..3 in between applies that row), `<S-Up>`, `<S-Down>`, `<Esc>`, `<C-1>`..`<C-3>` (Ctrl+digit). `<Idle>`: a typing pause long enough for word
//! completions and for the input-language preview. Each line also prints `[bubble: ...]` state after keys that change it, with `>` marking the selected row.
//! `<APP:id>`: keys now go to the app with that bundle id or exe name (e.g. `<APP:com.mitchellh.ghostty>`); a terminal
//! deletes a grapheme per Backspace, here and in the engine's count.
//! `^R` (Ctrl+Opt+R) reads the selection at once: the text of the last `<SEL:text>` (taken by that capture), else nothing
//! selected. Local sentence completion calls Ollama; cloud rewrites and pastes are only printed.
//! Settings come from `prefs.json` beside `--state`: `{"accept_with":"tab_or_space"}` there makes Space apply a live row,
//! `{"pick_with_ctrl":true}` makes `<C-1>`..`<C-3>` pick a row.
use crate::{app::{hotkey_for, App}, backspace::DeleteUnit, engine::{Action, Boundary, Key, Outcome}, rewrite::{Captured, Job, Preset, Reply}};
use std::io::BufRead;
use unicode_segmentation::UnicodeSegmentation;
/// One Backspace in an app that deletes `unit`s.
fn backspace(screen: &mut Vec<char>, unit: DeleteUnit) {
    let text: String = screen.iter().collect();
    let n = text.graphemes(true).next_back().map_or(0, |g| if unit == DeleteUnit::Grapheme { g.chars().count() } else { 1 });
    screen.truncate(screen.len() - n);
}
pub fn apply(screen: &mut Vec<char>, acts: &[Action], swallowed: bool, typed: Option<char>, unit: DeleteUnit) {
    if !swallowed { match typed {
        Some('\u{8}') => backspace(screen, unit),
        Some('\u{17}') => { while screen.last().is_some_and(|c| c.is_whitespace()) { screen.pop(); } while screen.last().is_some_and(|c| !c.is_whitespace()) { screen.pop(); } } // word delete
        Some('\u{15}') => { while screen.last().is_some_and(|c| *c != '\n') { screen.pop(); } } // line delete
        Some(c) => screen.push(c), None => {}
    } }
    for a in acts { if let Action::Replace { delete, insert, then } = a {
        for _ in 0..*delete { backspace(screen, unit); } screen.extend(insert.chars()); if let Some(b) = then { screen.extend(b.text().chars()) }
    } }
}
pub fn run(mut app: App) {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap(); let mut screen: Vec<char> = vec![]; let mut bubble: Vec<String> = vec![]; let ch: Vec<char> = line.chars().collect(); let mut i = 0;
        let mut selection: Option<String> = None;
        while i < ch.len() {
            let rest: String = ch[i..].iter().collect();
            if let Some(id) = rest.strip_prefix("<APP:").and_then(|r| r.split_once('>')).map(|(id, _)| id.to_string()) {
                app.engine.delete_unit = DeleteUnit::of_app(&id);
                println!("  [app: {id}, Backspace deletes a {:?}]", app.engine.delete_unit);
                i += id.chars().count() + "<APP:>".len(); continue
            }
            if let Some(sel) = rest.strip_prefix("<SEL:").and_then(|r| r.split_once('>')).map(|(s, _)| s.to_string()) {
                println!("  [selected: {sel:?}]");
                i += sel.chars().count() + "<SEL:>".len(); selection = Some(sel); continue
            }
            let tok = ["<Tab>", "<TabDown>", "<TabUp>", "<S-Up>", "<S-Down>", "<Esc>", "<C-1>", "<C-2>", "<C-3>", "<TH>", "<EN>", "<Focus>", "<Blur>", "<BS>", "<A-BS>", "<C-BS>", "<Idle>"].into_iter().find(|t| rest.starts_with(t));
            // None = the typing pause (`<Idle>`)
            let (k, typed) = if let Some(t) = tok {
                i += t.chars().count() - 1;
                if t == "<TH>" || t == "<EN>" { app.engine.set_thai_active(t == "<TH>"); println!("  [input source: {}]", if t == "<TH>" { "Thai" } else { "ABC" }); i += 1; continue }
                if t == "<Focus>" || t == "<Blur>" { for a in app.engine.on_focus(t == "<Focus>") { println!("  {t} -> {a:?}") } i += 1; continue }
                match t { "<Tab>" | "<TabDown>" => (Some(Key::Tab), Some('\t')), "<TabUp>" => (Some(Key::TabUp), None), "<BS>" => (Some(Key::Backspace), Some('\u{8}')), "<A-BS>" => (Some(Key::WordDelete), Some('\u{17}')), "<C-BS>" => (Some(Key::LineDelete), Some('\u{15}')), "<S-Up>" => (Some(Key::SelectPrev), None), "<S-Down>" => (Some(Key::SelectNext), None), "<Idle>" => (None, None),
                    "<C-1>" | "<C-2>" | "<C-3>" => (Some(Key::PickRow(t.as_bytes()[3] as usize - b'1' as usize)), None), _ => (Some(Key::Esc), None) }
            } else if ch[i] == '^' && i + 1 < ch.len() { i += 1; (Some(Key::Hotkey(hotkey_for(ch[i]).expect("hotkey"))), None) }
                else { match ch[i] { ' ' => (Some(Key::Boundary(Boundary::Space)), Some(' ')), '\t' => (Some(Key::Tab), Some('\t')), c => match typethai_core::reverse::th_text_to_keys(&c.to_string()).filter(|_| typethai_core::keymap::is_thai(c)) {
                    // a Thai char in the input = that physical key; the screen shows the Thai char only while Thai is
                    // active (after a switch to ABC the same keys produce the English letters, as for a real user)
                    Some(k) => { let k = k.chars().next().unwrap(); (Some(Key::Char(k)), Some(if app.engine.thai_active() { c } else { k })) }
                    None => (Some(Key::Char(c)), Some(c)),
                } } };
            // a Tab not held with `<TabDown>` is released right away
            let release = k == Some(Key::Tab) && tok != Some("<TabDown>");
            for (k, typed) in std::iter::once((k, typed)).chain(release.then_some((Some(Key::TabUp), None))) {
                // a keystroke, as the platform sees it: it takes the input-language preview down (`<Idle>` is no key)
                let mut o = match &k {
                    Some(k) => { let mut a = if *k == Key::TabUp { vec![] } else { app.engine.on_keystroke(std::time::Instant::now()) };
                        let o = app.engine.on_key(k.clone()); a.extend(o.actions); Outcome { swallow: o.swallow, actions: a } }
                    None => Outcome { swallow: false, actions: app.engine.on_idle(std::time::Instant::now() + app.engine.opts.complete_idle.max(app.engine.opts.badge_idle.unwrap_or_default())) },
                };
                let captures: Vec<_> = o.actions.iter().filter_map(|a| if let Action::Rewrite(Job::Capture(id)) = a { Some(*id) } else { None }).collect();
                for id in captures {
                    let sel = selection.take().map_or(Captured::NoSelection, Captured::Text);
                    o.actions.extend(app.engine.on_reply(Reply::Captured { id, sel, has_key: true }));
                }
                let completion = o.actions.iter().find_map(|a| match a {
                    Action::Rewrite(Job::Run(run)) if run.preset == Preset::Finish => Some((run.req, crate::completion::run(&run.text, &run.cfg.local_model))),
                    _ => None,
                });
                if let Some((id, result)) = completion {
                    o.actions.extend(app.engine.on_reply(Reply::Rewritten(id, result)));
                }
                let k = k.map_or("Idle".to_string(), |k| format!("{k:?}"));
                for a in &o.actions { println!("  {k} -> {a:?}"); }
                if o.swallow && o.actions.is_empty() { println!("  {k} -> (swallowed)"); }
                for a in &o.actions { if let Action::SwitchInput(l) = a { app.engine.set_thai_active(*l == crate::engine::InputLang::Thai); } }
                for a in &o.actions { match a {
                    Action::ShowBubble(items) => bubble = items.iter().map(|b| b.label.clone()).collect(),
                    Action::HideBubble => bubble.clear(), _ => {}
                } }
                if o.actions.iter().any(|a| matches!(a, Action::ShowBubble(_) | Action::Select(_))) {
                    let sel = app.engine.selected();
                    println!("  [bubble: {}]", bubble.iter().enumerate().map(|(j, l)| format!("{}{l}", if j == sel { ">" } else { " " })).collect::<Vec<_>>().join(" | "));
                }
                apply(&mut screen, &o.actions, o.swallow, typed, app.engine.delete_unit);
            }
            i += 1;
        }
        let o = app.engine.on_key(Key::Boundary(Boundary::Enter)); apply(&mut screen, &o.actions, o.swallow, None, app.engine.delete_unit);
        for a in &o.actions { println!("  Enter -> {a:?}"); }
        println!("{}", screen.iter().collect::<String>()); app.maybe_save();
    }
    app.save_and_wait();
}
