//! typethai-desktop: system-wide wrong-layout (EN QWERTY -> Thai Kedmanee) auto-converter with a suggestion bubble.
#![allow(dead_code)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod app; mod backspace; mod bridge; mod candidate; mod completion; mod engine; mod keys; mod local_setup; mod placement; mod prefs; mod rank; mod rewrite; mod predict;
mod badge; mod inputsrc; mod evallive; mod evaltypo; mod platform; mod update;
#[cfg(any(windows, target_os = "macos"))] mod ui;
fn main() {
    // --demo-bubble [x,y ...] [--secs N]: show the gpui bubble with sample Thai items, no key capture, no model needed
    let args: Vec<String> = std::env::args().collect();
    // --check-update: ask GitHub for the latest release (through the redirect, no API) and print what it found
    if args.iter().any(|a| a == "--check-update") { update::print_check(); return }
    // --demo-toast: post the system notification an update raises, and say whether this copy can
    if args.iter().any(|a| a == "--demo-toast") {
        #[cfg(any(windows, target_os = "macos"))] { ui::demo_toast(); return }
        #[cfg(not(any(windows, target_os = "macos")))] { eprintln!("--demo-toast needs macOS or Windows"); std::process::exit(2) }
    }
    if let Some(i) = args.iter().position(|a| a == "--demo-bubble") {
        // extra debug options: --screenshot PATH (capture the bubble's own window to a PNG, then quit),
        // --items "a|b|c" (custom rows instead of the samples; repeat it to update the bubble in place; a row ending in
        // !th, !en or !typo is offered for that reason), --vertical (ask for rows, not a strip), --light,
        // --backdrop light|dark (a window of text behind the bubble), --hold P (Tab held over it, P of the way to the rows),
        // --shift (Shift held: the thumbs on the options offered in Thai), --update (the rail carries its download chip)
        let mut hold: Option<f32> = None; let mut pts = vec![]; let mut secs = 3.0; let mut shot: Option<String> = None; let mut items: Vec<Vec<bridge::Opt>> = vec![]; let mut select = 0usize; let mut focused = false; let mut layout = bridge::BubbleLayout::Horizontal; let mut light = false; let mut backdrop = None; let mut update = false;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() {
            if a == "--secs" { secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(3.0); continue }
            if a == "--screenshot" { shot = it.next().cloned(); continue }
            if a == "--hold" { hold = it.next().and_then(|s| s.parse().ok()); continue }
            if a == "--select" { select = it.next().and_then(|s| s.parse().ok()).unwrap_or(0); continue }
            if a == "--items" { items.extend(it.next().map(|s| s.split('|').map(demo_opt).collect())); continue }
            if a == "--backdrop" { backdrop = match it.next().map(String::as_str) { Some("light") => Some(true), Some("dark") => Some(false), b => { eprintln!("--backdrop: expected light or dark, got {b:?}"); std::process::exit(2) } }; continue }
            if a == "--vertical" { layout = bridge::BubbleLayout::Vertical; continue }
            if a == "--focused" { focused = true; continue }
            if a == "--light" { light = true; continue }
            if a == "--shift" { bridge::SHIFT_HELD.store(true, std::sync::atomic::Ordering::SeqCst); continue }
            if a == "--update" { update = true; continue }
            let v: Vec<f64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 2 { pts.push((v[0], v[1])) } else { eprintln!("--demo-bubble: expected x,y but got {a:?}"); std::process::exit(2) }
        }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo(pts, secs, shot, items, select, focused, hold, layout, light, backdrop, update); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (pts, secs, shot, items, select, focused, hold, layout, light, backdrop, update); eprintln!("--demo-bubble needs macOS or Windows"); std::process::exit(2) }
    }
    // --demo-badge [x,y ...] [--secs N] [--en] [--light] [--backdrop light|dark] [--screenshot PNG]: the input-language
    // badge at a fake caret at each point (Thai first, then taking turns), no key capture
    if let Some(i) = args.iter().position(|a| a == "--demo-badge") {
        let mut pts = vec![]; let mut secs = 2.0; let mut shot: Option<String> = None; let mut lang = engine::InputLang::Thai; let mut light = false; let mut backdrop = None; let mut pin = false;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() { match a.as_str() {
            "--secs" => secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(2.0),
            "--screenshot" => shot = it.next().cloned(),
            "--en" => lang = engine::InputLang::Latin,
            "--pin" => pin = true,
            "--light" => light = true,
            "--backdrop" => backdrop = match it.next().map(String::as_str) { Some("light") => Some(true), Some("dark") => Some(false), b => { eprintln!("--backdrop: expected light or dark, got {b:?}"); std::process::exit(2) } },
            _ => { let v: Vec<f64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() == 2 { pts.push((v[0], v[1])) } else { eprintln!("--demo-badge: expected x,y but got {a:?}"); std::process::exit(2) } }
        } }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo_badge(pts, secs, shot, lang, pin, light, backdrop); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (pts, secs, shot, lang, pin, light, backdrop); eprintln!("--demo-badge needs macOS or Windows"); std::process::exit(2) }
    }
    // --demo-hold [x,y ...] [--progress P] [--secs N] [--light] [--backdrop light|dark] [--screenshot PNG]: the hold-Tab
    // indicator with no bubble up, filling as a real hold does (or stopped at P), no key capture
    if let Some(i) = args.iter().position(|a| a == "--demo-hold") {
        let mut pts = vec![]; let mut secs = 1.0; let mut shot: Option<String> = None; let mut progress: Option<f32> = None; let mut light = false; let mut backdrop = None;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() { match a.as_str() {
            "--secs" => secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(1.0),
            "--screenshot" => shot = it.next().cloned(),
            "--progress" => progress = it.next().and_then(|s| s.parse().ok()),
            "--light" => light = true,
            "--backdrop" => backdrop = match it.next().map(String::as_str) { Some("light") => Some(true), Some("dark") => Some(false), b => { eprintln!("--backdrop: expected light or dark, got {b:?}"); std::process::exit(2) } },
            _ => { let v: Vec<f64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() == 2 { pts.push((v[0], v[1])) } else { eprintln!("--demo-hold: expected x,y but got {a:?}"); std::process::exit(2) } }
        } }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo_hold(pts, secs, shot, progress, light, backdrop); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (pts, secs, shot, progress, light, backdrop); eprintln!("--demo-hold needs macOS or Windows"); std::process::exit(2) }
    }
    // --demo-settings [--secs N] [--screenshot PNG] [--search TEXT] [--page N] [--light] [--local-state NAME|--local-cycle]:
    // only the Settings window, with sample words and no key capture, prefs or keychain. The local-state options show the
    // Local model setup row in a chosen state (checking, install, start, pull, downloading, ready, failed) instead of
    // asking Ollama, so every branch can be seen without moving Ollama around.
    if let Some(i) = args.iter().position(|a| a == "--demo-settings") {
        let mut secs = 30.0; let mut shot: Option<String> = None; let mut search: Option<String> = None; let mut page = 0usize; let mut light = false;
        let mut local: Option<ui::LocalDemo> = None;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() { match a.as_str() {
            "--secs" => secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(30.0),
            "--screenshot" => shot = it.next().cloned(),
            "--search" => search = it.next().cloned(),
            "--page" => page = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--light" => light = true,
            "--local-cycle" => local = ui::demo_local("cycle"),
            "--local-state" => match it.next().and_then(|s| ui::demo_local(s)) {
                Some(state) => local = Some(state),
                None => { eprintln!("--local-state: expected checking, install, start, pull, downloading, ready, failed or cycle"); std::process::exit(2) }
            },
            _ => { eprintln!("--demo-settings: unknown option {a:?}"); std::process::exit(2) }
        } }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo_settings(secs, shot, search, page, light, local); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (secs, shot, search, page, light, local); eprintln!("--demo-settings needs macOS or Windows"); std::process::exit(2) }
    }
    // --probe-caret [--secs N]: log frontmost app / anchor source / caret rect every 500 ms and mark it on screen
    if args.iter().any(|a| a == "--probe-caret") {
        let secs = args.iter().position(|a| a == "--secs").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(10.0);
        #[cfg(target_os = "macos")] { ui::probe(secs, platform::macos::caret_anchor, platform::macos::ax_trusted()); return }
        #[cfg(not(target_os = "macos"))] { let _ = secs; eprintln!("--probe-caret is implemented for macOS only"); std::process::exit(2) }
    }
    let cli = app::Cli::parse();
    if cli.list_sources { platform::print_input_sources(); return }
    if let Some(file) = &cli.rank_report {
        let path = if file.is_empty() { rank::path_beside(&cli.state) } else { std::path::PathBuf::from(file) };
        let (events, bad) = rank::load(&path);
        println!("{}: {} offers", path.display(), events.len());
        if bad > 0 { println!("{bad} line(s) could not be read and were skipped") }
        print!("{}", rank::report(&events));
        if events.is_empty() { println!("(nothing logged yet; the log is off while Learn from my typing is off)") }
        return
    }
    bridge::REQ_OPEN_SETTINGS.store(cli.open_settings, std::sync::atomic::Ordering::SeqCst);
    bridge::SETTINGS_PAGE.store(cli.settings_page, std::sync::atomic::Ordering::SeqCst);
    if let Some(dir) = &cli.eval_typo { evaltypo::run(&cli, dir); return }
    if let Some(dir) = &cli.eval_live { if cli.eval_reverse { evallive::run_reverse(&cli, dir, &cli.eval_split) } else { evallive::run(&cli, dir, &cli.eval_split) } return }
    let app = match app::App::load(&cli) { Ok(a) => a, Err(e) => { eprintln!("typethai: {e}"); std::process::exit(1) } };
    if cli.simulate { platform::sim::run(app) } else { update::spawn(app.engine.check_updates); platform::run(app) }
}
/// A `--demo-bubble --items` row: its label, and the reason a trailing `!th`, `!en` or `!typo` names. One offered in Thai
/// is rated, as a conversion is (`BubbleItem::rated_word`).
fn demo_opt(s: &str) -> bridge::Opt {
    use engine::{InputLang, Reason};
    let reason = |b| match b { "th" => Some(Reason::Layout(InputLang::Thai)), "en" => Some(Reason::Layout(InputLang::Latin)), "typo" => Some(Reason::Typo), _ => None };
    match s.rsplit_once('!').and_then(|(label, b)| Some((label, reason(b)?))) {
        Some((label, r)) => bridge::Opt { label: label.into(), reason: Some(r), rated: r == Reason::Layout(InputLang::Thai) },
        None => bridge::Opt { label: s.into(), reason: None, rated: false },
    }
}
