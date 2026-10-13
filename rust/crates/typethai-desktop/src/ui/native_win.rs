//! Windows: monitor list (physical pixels, per-monitor DPI) and native show/hide of the gpui window without activation.
use crate::placement::{Monitor, Rect};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

fn r(rc: RECT) -> Rect { Rect::new(rc.left as f64, rc.top as f64, (rc.right - rc.left) as f64, (rc.bottom - rc.top) as f64) }
unsafe extern "system" fn enum_cb(h: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> windows::core::BOOL {
    let out = &mut *(lp.0 as *mut Vec<Monitor>);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(h, &mut mi).as_bool() {
        let (mut dx, mut dy) = (96u32, 96u32); let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        out.push(Monitor { frame: r(mi.rcMonitor), visible: r(mi.rcWork), size_scale: dx as f64 / 96.0, pixel: 1.0 });
    }
    true.into()
}
pub fn monitors() -> Vec<Monitor> {
    let mut v: Vec<Monitor> = Vec::new();
    unsafe { let _ = EnumDisplayMonitors(None, None, Some(enum_cb), LPARAM(&mut v as *mut _ as isize)); }
    v
}
/// See native_mac::Native: SetWindowPos sends WM_SIZE synchronously, so it must run outside gpui's window borrow.
#[derive(Clone)]
pub struct Native(HWND);
unsafe impl Send for Native {}
pub fn native(window: &Window) -> Option<Native> {
    let h = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::Win32(w) = h.as_raw() else { return None };
    Some(Native(HWND(w.hwnd.get() as *mut core::ffi::c_void)))
}
pub fn prepare(n: &Native) { unsafe {
    let ex = GetWindowLongPtrW(n.0, GWL_EXSTYLE);
    SetWindowLongPtrW(n.0, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0) as isize);
} }
pub fn show(n: &Native, rc: Rect) { unsafe {
    let _ = SetWindowPos(n.0, Some(HWND_TOPMOST), rc.x.round() as i32, rc.y.round() as i32, rc.w.round() as i32, rc.h.round() as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
} }
pub fn hide(n: &Native) { unsafe { let _ = ShowWindow(n.0, SW_HIDE); } }
pub fn describe(n: &Native) -> String { let mut rc = RECT::default(); unsafe { let _ = GetWindowRect(n.0, &mut rc); } format!("hwnd rect {:?}", rc) }
/// Debug capture is implemented for macOS only.
pub fn capture(_: &Native, _: &str) -> Result<(), String> { Err("--screenshot is macOS-only".into()) }
