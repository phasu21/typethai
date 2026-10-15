pub mod sim;
pub mod spell;
#[cfg(windows)] pub mod windows;
#[cfg(target_os = "macos")] pub mod macos;
#[cfg(any(windows, target_os = "macos"))] pub mod tray;

pub fn run(app: crate::app::App) {
    #[cfg(windows)] { windows::run(app) }
    #[cfg(target_os = "macos")] { macos::run(app) }
    #[cfg(not(any(windows, target_os = "macos")))] { eprintln!("no native key capture on this OS; running --simulate (stdin)"); sim::run(app) }
}

/// Enabled keyboard input sources (macOS: TIS; Windows: loaded keyboard layouts). Empty elsewhere.
pub fn input_sources() -> Vec<crate::inputsrc::Source> {
    #[cfg(windows)] { windows::input_sources() }
    #[cfg(target_os = "macos")] { macos::input_sources() }
    #[cfg(not(any(windows, target_os = "macos")))] { vec![] }
}
pub fn check_for_updates() {
    let out = match std::process::Command::new("curl").args(["-s", "-L", "https://api.github.com/repos/phasu21/typethai/releases/latest"]).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return,
    };
    // tiny JSON field extractor (avoids pulling serde into the desktop crate)
    let field = |key: &str| -> Option<String> {
        let needle = format!("\"{}\"", key);
        let i = out.find(&needle)?;
        let rest = &out[i + needle.len()..];
        let q = rest.find('"')? + 1;
        let end = rest[q..].find('"')?;
        Some(rest[q..q + end].to_string())
    };
    let tag = match field("tag_name") { Some(t) => t, None => return };
    let latest = tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");
    if latest == current { return }
    let url = field("html_url").unwrap_or_else(|| "https://github.com/phasu21/typethai/releases".to_string());
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
    #[cfg(not(any(target_os = "macos", windows)))]
    eprintln!("typeThai update available: {latest} (current {current}) -> {url}");
}
/// `--list-input-sources`: read-only listing plus what typeThai would switch to.
pub fn print_input_sources() {
    use crate::{engine::InputLang, inputsrc::pick};
    let v = input_sources();
    for s in &v { println!("{}\t{}\tascii={} selectable={}", s.id, s.name, s.ascii, s.selectable); }
    if v.is_empty() { println!("(no input sources reported on this OS)") }
    for l in [InputLang::Thai, InputLang::Latin] {
        match pick(&v, l) { Some(s) => println!("switch target {l:?}: {}", s.id), None => println!("switch target {l:?}: NONE ENABLED") }
    }
}
