pub mod sim;
pub mod spell;
pub mod field;
pub mod follow;
#[cfg(windows)] pub mod windows;
#[cfg(target_os = "macos")] pub mod macos;
#[cfg(any(windows, target_os = "macos"))] pub mod tray;

/// Ask the OS for the permission typeThai needs to see keys (macOS: Accessibility, with its pane and prompt; Windows
/// needs none). Also reachable from the tray, when a first attempt was refused.
pub fn request_permission() {
    #[cfg(target_os = "macos")] { macos::request_permission() }
}

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
