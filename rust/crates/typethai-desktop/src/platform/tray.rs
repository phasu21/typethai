//! Tray icon with Pause/Resume, the update item, Settings, and Quit (tray-icon crate; needs the platform's main-thread event loop running).
use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, TrayIcon, TrayIconBuilder};
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
pub struct Tray { pub icon: TrayIcon, pause: MenuItem, app: MenuItem, update: MenuItem, settings: MenuItem, permission: MenuItem, quit: MenuItem }
fn icon(paused: bool) -> Icon {
    let (w, h) = (32u32, 32u32); let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h { for x in 0..w {
        let border = x < 2 || y < 2 || x >= w - 2 || y >= h - 2; let bar = (13..19).contains(&x) && (6..26).contains(&y) || (8..24).contains(&x) && (6..11).contains(&y); // a "T"
        let c: [u8; 4] = if border || bar { [255, 255, 255, 255] } else if paused { [128, 128, 128, 255] } else { [36, 84, 200, 255] };
        px.extend(c);
    } }
    Icon::from_rgba(px, w, h).expect("icon")
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
        let icon = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("typeThai").with_icon(icon(false)).build().map_err(|e| e.to_string())?;
        Ok(Tray { icon, pause, app, update, settings, permission, quit })
    }
    pub fn set_paused(&self, p: bool) {
        self.pause.set_text(if p { "Resume typeThai" } else { "Pause typeThai" }); let _ = self.icon.set_icon(Some(icon(p)));
        let _ = self.icon.set_tooltip(Some(if p { "typeThai (paused)" } else { "typeThai" }));
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
