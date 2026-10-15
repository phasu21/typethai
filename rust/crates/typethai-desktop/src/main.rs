//! typethai-desktop: system-wide wrong-layout (EN QWERTY -> Thai Kedmanee) auto-converter with a suggestion bubble.
#![allow(dead_code)]
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod app; mod bridge; mod engine; mod keys; mod placement; mod prefs; mod rewrite;
mod inputsrc; mod evallive; mod platform;
#[cfg(any(windows, target_os = "macos"))] mod ui;
fn main() {
    // --demo-bubble [x,y ...] [--secs N]: show the gpui bubble with sample Thai items, no key capture, no model needed
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--demo-bubble") {
        // extra debug options: --screenshot PATH (capture the bubble's own window to a PNG, then quit),
        // --items "a|b|c" (custom rows instead of the samples)
        let mut pts = vec![]; let mut secs = 3.0; let mut shot: Option<String> = None; let mut items: Option<Vec<String>> = None; let mut select = 0usize;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() {
            if a == "--secs" { secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(3.0); continue }
            if a == "--screenshot" { shot = it.next().cloned(); continue }
            if a == "--select" { select = it.next().and_then(|s| s.parse().ok()).unwrap_or(0); continue }
            if a == "--items" { items = it.next().map(|s| s.split('|').map(String::from).collect()); continue }
            let v: Vec<f64> = a.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if v.len() == 2 { pts.push((v[0], v[1])) } else { eprintln!("--demo-bubble: expected x,y but got {a:?}"); std::process::exit(2) }
        }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo(pts, secs, shot, items, select); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (pts, secs, shot, items, select); eprintln!("--demo-bubble needs macOS or Windows"); std::process::exit(2) }
    }
    // --demo-settings [--secs N] [--screenshot PNG] [--search TEXT] [--page N] [--light]: only the Settings window, with sample
    // words and no key capture, prefs or keychain
    if let Some(i) = args.iter().position(|a| a == "--demo-settings") {
        let mut secs = 30.0; let mut shot: Option<String> = None; let mut search: Option<String> = None; let mut page = 0usize; let mut light = false;
        let mut it = args[i + 1..].iter();
        while let Some(a) = it.next() { match a.as_str() {
            "--secs" => secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(30.0),
            "--screenshot" => shot = it.next().cloned(),
            "--search" => search = it.next().cloned(),
            "--page" => page = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--light" => light = true,
            _ => { eprintln!("--demo-settings: unknown option {a:?}"); std::process::exit(2) }
        } }
        #[cfg(any(windows, target_os = "macos"))] { ui::demo_settings(secs, shot, search, page, light); return }
        #[cfg(not(any(windows, target_os = "macos")))] { let _ = (secs, shot, search, page, light); eprintln!("--demo-settings needs macOS or Windows"); std::process::exit(2) }
    }
    // --probe-caret [--secs N]: log frontmost app / anchor source / caret rect every 500 ms and mark it on screen
    if args.iter().any(|a| a == "--probe-caret") {
        let secs = args.iter().position(|a| a == "--secs").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(10.0);
        #[cfg(target_os = "macos")] { ui::probe(secs, platform::macos::caret_anchor, platform::macos::ax_trusted()); return }
        #[cfg(not(target_os = "macos"))] { let _ = secs; eprintln!("--probe-caret is implemented for macOS only"); std::process::exit(2) }
    }
    let cli = app::Cli::parse();
    if cli.list_sources { platform::print_input_sources(); return }
    bridge::REQ_OPEN_SETTINGS.store(cli.open_settings, std::sync::atomic::Ordering::SeqCst);
    if let Some(dir) = &cli.eval_live { if cli.eval_reverse { evallive::run_reverse(&cli, dir, &cli.eval_split) } else { evallive::run(&cli, dir, &cli.eval_split) } return }
    let app = match app::App::load(&cli) { Ok(a) => a, Err(e) => { eprintln!("typethai: {e}"); std::process::exit(1) } };
    if cli.simulate { platform::sim::run(app) } else { platform::run(app) }
}
