//! Tray icon with Pause/Resume and Quit (tray-icon crate; needs the platform's main-thread event loop running).
use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, TrayIcon, TrayIconBuilder};
pub enum TrayCmd { TogglePause, Quit }
pub struct Tray { pub icon: TrayIcon, pause: MenuItem, quit: MenuItem }
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
        let pause = MenuItem::new("Pause typeThai", true, None); let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new(); menu.append(&pause).map_err(|e| e.to_string())?; menu.append(&quit).map_err(|e| e.to_string())?;
        let icon = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("typeThai").with_icon(icon(false)).build().map_err(|e| e.to_string())?;
        Ok(Tray { icon, pause, quit })
    }
    pub fn set_paused(&self, p: bool) {
        self.pause.set_text(if p { "Resume typeThai" } else { "Pause typeThai" }); let _ = self.icon.set_icon(Some(icon(p)));
        let _ = self.icon.set_tooltip(Some(if p { "typeThai (paused)" } else { "typeThai" }));
    }
    /// Non-blocking poll of menu clicks.
    pub fn poll(&self) -> Option<TrayCmd> {
        while let Ok(e) = MenuEvent::receiver().try_recv() {
            if e.id == *self.pause.id() { return Some(TrayCmd::TogglePause) }
            if e.id == *self.quit.id() { return Some(TrayCmd::Quit) }
        }
        None
    }
}
