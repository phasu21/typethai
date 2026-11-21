//! Tray icon with Pause/Resume, the update item, Settings, and Quit (tray-icon crate; needs the platform's main-thread event loop running).
//! The icon itself is the live input-language indicator (`trayicon`).
use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, TrayIcon, TrayIconBuilder};
use crate::bridge;
use crate::trayicon::{self, Frame, Live, Look, Style, Target};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
pub enum TrayCmd { TogglePause, Quit, CheckUpdate, Settings, Permission, ToggleApp }
/// The app item's label when no app is in front (or it is typeThai itself): there is nothing to turn off.
const APP_NONE: &str = "Turn off in this app";
/// A tray label stays readable: names over 28 characters are cut with an ellipsis.
fn short_name(name: &str) -> String {
    if name.chars().count() <= 28 { name.to_string() } else { name.chars().take(27).chain(std::iter::once('…')).collect() }
}
/// The permission item's two states. macOS needs Accessibility; other platforms have nothing to grant.
#[cfg(target_os = "macos")] const PERMISSION_NEEDED: &str = "Grant Accessibility…";
#[cfg(target_os = "macos")] const PERMISSION_OK: &str = "Accessibility: on";
#[cfg(not(target_os = "macos"))] const PERMISSION_NEEDED: &str = "Permissions";
#[cfg(not(target_os = "macos"))] const PERMISSION_OK: &str = "Permissions";
/// The tray's images (`tray/*.png`, drawn from `packaging/icon/`): the live input-language cards, see `trayicon`.
macro_rules! pngs { ($($n:literal),* $(,)?) => { &[$(($n, include_bytes!(concat!("../../tray/", $n, ".png")) as &[u8])),*] } }
#[cfg(target_os = "macos")]
static PNGS: &[(&str, &[u8])] = pngs![
    "mac-colour-english-18", "mac-colour-english-36", "mac-colour-english-paused-18", "mac-colour-english-paused-36",
    "mac-colour-swap1-18", "mac-colour-swap1-36", "mac-colour-swap2-18", "mac-colour-swap2-36",
    "mac-colour-thai-18", "mac-colour-thai-36", "mac-colour-thai-paused-18", "mac-colour-thai-paused-36",
    "mac-plain-english-18", "mac-plain-english-36", "mac-plain-english-paused-18", "mac-plain-english-paused-36",
    "mac-plain-swap1-18", "mac-plain-swap1-36", "mac-plain-swap2-18", "mac-plain-swap2-36",
    "mac-plain-thai-18", "mac-plain-thai-36", "mac-plain-thai-paused-18", "mac-plain-thai-paused-36"
];
#[cfg(windows)]
static PNGS: &[(&str, &[u8])] = pngs![
    "win-english-16", "win-english-20", "win-english-24", "win-english-32",
    "win-english-paused-16", "win-english-paused-20", "win-english-paused-24", "win-english-paused-32",
    "win-swap1-16", "win-swap1-20", "win-swap1-24", "win-swap1-32",
    "win-swap2-16", "win-swap2-20", "win-swap2-24", "win-swap2-32",
    "win-thai-16", "win-thai-20", "win-thai-24", "win-thai-32",
    "win-thai-paused-16", "win-thai-paused-20", "win-thai-paused-24", "win-thai-paused-32"
];
#[cfg(target_os = "macos")] const TARGET: Target = Target::Mac;
#[cfg(windows)] const TARGET: Target = Target::Windows;

/// Decode one of the tray PNGs to the RGBA the tray-icon crate takes.
fn decode(png_bytes: &[u8]) -> Result<Icon, String> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = dec.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let px = &buf[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return Err("unexpanded palette png".into()),
    };
    Icon::from_rgba(rgba, info.width, info.height).map_err(|e| e.to_string())
}
/// The size the tray wants now: macOS, the highest backing scale among the screens (the menu bar is on every one);
/// Windows, the small-icon size at the system DPI.
fn wanted_size() -> f64 {
    #[cfg(target_os = "macos")] { crate::ui::native::monitors().iter().map(|m| m.pixel).fold(1.0, f64::max) }
    #[cfg(windows)] unsafe {
        use windows::Win32::UI::{HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi}, WindowsAndMessaging::SM_CXSMICON};
        GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) as f64
    }
}
/// The system asks for less motion (macOS: Accessibility > Display > Reduce motion; Windows: animation effects off).
fn reduce_motion() -> bool {
    #[cfg(target_os = "macos")] unsafe {
        let ws: *mut objc2::runtime::AnyObject = objc2::msg_send![objc2::class!(NSWorkspace), sharedWorkspace];
        !ws.is_null() && { let b: bool = objc2::msg_send![ws, accessibilityDisplayShouldReduceMotion]; b }
    }
    #[cfg(windows)] unsafe {
        use windows::Win32::{Foundation::BOOL, UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS}};
        let mut on = BOOL(1);
        SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut BOOL as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)).is_ok() && !on.as_bool()
    }
}

pub struct Tray {
    pub icon: TrayIcon, pause: MenuItem, app: MenuItem, update: MenuItem, settings: MenuItem, permission: MenuItem, quit: MenuItem,
    /// The live icon: whether typeThai is paused, the swap in progress, and the images decoded so far.
    paused: Cell<bool>, live: RefCell<Live>, cache: RefCell<HashMap<String, Icon>>,
    /// The pixel size in use, and when it was last looked up (screens come and go).
    px: Cell<u32>, px_at: Cell<std::time::Instant>,
}
/// How often the tray asks again which pixel size the screens want.
const SIZE_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
fn image(cache: &RefCell<HashMap<String, Icon>>, frame: Frame, px: u32) -> Option<Icon> {
    let name = trayicon::asset(TARGET, frame, px);
    if let Some(i) = cache.borrow().get(&name) { return Some(i.clone()) }
    let bytes = PNGS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)?;
    match decode(bytes) {
        Ok(i) => { cache.borrow_mut().insert(name, i.clone()); Some(i) }
        Err(e) => { eprintln!("typeThai: tray image {name}: {e}"); None }
    }
}
/// What the icon should say now: the input source the platform last published, pausing, and the style from Settings.
fn current_look(paused: bool) -> Look {
    Look { thai: bridge::INPUT_THAI.load(Ordering::SeqCst), paused, style: if bridge::TRAY_PLAIN.load(Ordering::SeqCst) { Style::Plain } else { Style::Colour } }
}
impl Tray {
    pub fn new() -> Result<Tray, String> {
        let pause = MenuItem::new("Pause typeThai", true, None);
        let app = MenuItem::new(APP_NONE, false, None);
        let update = MenuItem::new("Check for updates", true, None);
        let settings = MenuItem::new("Settings…", true, None);
        let permission = MenuItem::new(PERMISSION_OK, true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        menu.append(&pause).map_err(|e| e.to_string())?;
        menu.append(&app).map_err(|e| e.to_string())?;
        menu.append(&update).map_err(|e| e.to_string())?;
        menu.append(&settings).map_err(|e| e.to_string())?;
        menu.append(&permission).map_err(|e| e.to_string())?;
        menu.append(&quit).map_err(|e| e.to_string())?;
        let cache = RefCell::new(HashMap::new());
        let mut live = Live::new();
        let look = current_look(false);
        let px = trayicon::pixel_size(TARGET, wanted_size());
        let first = live.update(std::time::Instant::now(), look, || false).and_then(|f| image(&cache, f, px)).ok_or("no tray image")?;
        let icon = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("typeThai").with_icon(first)
            .with_icon_as_template(cfg!(target_os = "macos") && look.style == Style::Plain).build().map_err(|e| e.to_string())?;
        Ok(Tray { icon, pause, app, update, settings, permission, quit, paused: Cell::new(false), live: RefCell::new(live), cache, px: Cell::new(px), px_at: Cell::new(std::time::Instant::now()) })
    }
    pub fn set_paused(&self, p: bool) {
        self.pause.set_text(if p { "Resume typeThai" } else { "Pause typeThai" }); self.paused.set(p); self.tick();
        let _ = self.icon.set_tooltip(Some(if p { "typeThai (paused)" } else { "typeThai" }));
    }
    /// Called every UI-loop turn: follow the input source (`bridge::INPUT_THAI`), pausing and the icon style, and play
    /// the swap. Only a change of frame touches the tray.
    pub fn tick(&self) {
        let now = std::time::Instant::now();
        let look = current_look(self.paused.get());
        let mut resized = false;
        if now.duration_since(self.px_at.get()) >= SIZE_EVERY {
            self.px_at.set(now);
            let px = trayicon::pixel_size(TARGET, wanted_size());
            if px != self.px.get() { self.px.set(px); resized = true }
        }
        let frame = self.live.borrow_mut().update(now, look, reduce_motion);
        let Some(frame) = frame.or_else(|| resized.then(|| self.live.borrow().shown()).flatten()) else { return };
        let Some(img) = image(&self.cache, frame, self.px.get()) else { return };
        #[cfg(target_os = "macos")] { let _ = self.icon.set_icon_with_as_template(Some(img), look.style == Style::Plain); }
        #[cfg(windows)] { let _ = self.icon.set_icon(Some(img)); }
    }
    /// The permission item: macOS refused the event tap, so nothing is captured until it is granted.
    pub fn set_permission(&self, needed: bool) {
        self.permission.set_text(if needed { PERMISSION_NEEDED } else { PERMISSION_OK });
        let _ = self.icon.set_tooltip(Some(if needed { "typeThai — Accessibility permission needed" } else { "typeThai" }));
    }
    /// The update item follows the updater: "Check for updates", "Restart to update to …", and so on.
    pub fn set_update(&self, s: &crate::update::Status) { let (text, enabled) = s.menu(); self.update.set_text(text); self.update.set_enabled(enabled); }
    /// The app item: turn typeThai off in the app in front, or back on when that app is already off.
    pub fn set_front_app(&self, f: Option<&crate::bridge::FrontApp>) {
        let (text, enabled) = match f {
            None => (APP_NONE.to_string(), false),
            Some(f) if f.off => (format!("Turn on in {}", short_name(&f.name)), true),
            Some(f) => (format!("Turn off in {}", short_name(&f.name)), true),
        };
        self.app.set_text(text); self.app.set_enabled(enabled);
    }
    /// Non-blocking poll of menu clicks.
    pub fn poll(&self) -> Option<TrayCmd> {
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == *self.pause.id() { return Some(TrayCmd::TogglePause) }
            if e.id == *self.app.id() { return Some(TrayCmd::ToggleApp) }
            if e.id == *self.update.id() { return Some(TrayCmd::CheckUpdate) }
            if e.id == *self.settings.id() { return Some(TrayCmd::Settings) }
            if e.id == *self.permission.id() { return Some(TrayCmd::Permission) }
            if e.id == *self.quit.id() { return Some(TrayCmd::Quit) }
        }
        None
    }
}

/// Windows 11 puts a new tray icon in the hidden-icons area, and with that area's menu turned off the icon cannot be
/// found at all. Once Explorer has recorded typeThai's icon (`HKCU\Control Panel\NotifyIconSettings\<id>`, matched by
/// executable path), show it on the taskbar, unless the user has already chosen (`IsPromoted` is set).
#[cfg(windows)]
pub fn promote_on_taskbar() {
    std::thread::spawn(|| {
        let Some(exe) = std::env::current_exe().ok().map(|p| p.to_string_lossy().to_lowercase()) else { return };
        // Explorer writes the entry shortly after the icon is added
        for _ in 0..30 {
            if let Some(done) = unsafe { promote(&exe) } { if done { return } }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}
/// Some(true) when typeThai's entry was found (and promoted unless already decided), None when it is not there yet.
#[cfg(windows)]
unsafe fn promote(exe: &str) -> Option<bool> {
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::Registry::*;
    let mut root = HKEY::default();
    if RegOpenKeyExW(HKEY_CURRENT_USER, w!("Control Panel\\NotifyIconSettings"), Some(0), KEY_READ, &mut root).is_err() { return None }
    let mut found = None;
    let mut name = [0u16; 256];
    for i in 0.. {
        let mut len = name.len() as u32;
        if RegEnumKeyExW(root, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() { break }
        let mut key = HKEY::default();
        if RegOpenKeyExW(root, PCWSTR(name.as_ptr()), Some(0), KEY_READ | KEY_SET_VALUE, &mut key).is_err() { continue }
        let path = read_string(key, w!("ExecutablePath"));
        if path.is_some_and(|p| p.to_lowercase() == exe) {
            let decided = RegQueryValueExW(key, w!("IsPromoted"), None, None, None, None).is_ok();
            if !decided { let _ = RegSetValueExW(key, w!("IsPromoted"), Some(0), REG_DWORD, Some(&1u32.to_le_bytes())); }
            found = Some(true);
        }
        let _ = RegCloseKey(key);
        if found.is_some() { break }
    }
    let _ = RegCloseKey(root);
    found
}
#[cfg(windows)]
unsafe fn read_string(key: windows::Win32::System::Registry::HKEY, value: windows::core::PCWSTR) -> Option<String> {
    use windows::Win32::System::Registry::*;
    let mut buf = [0u16; 1024]; let mut size = (buf.len() * 2) as u32; let mut kind = REG_VALUE_TYPE::default();
    RegQueryValueExW(key, value, None, Some(&mut kind), Some(buf.as_mut_ptr() as *mut u8), Some(&mut size)).ok().ok()?;
    if kind != REG_SZ { return None }
    let n = (size as usize / 2).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]).trim_end_matches('\0').to_string())
}
