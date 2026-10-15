//! Windows: monitor list (physical pixels, per-monitor DPI) and native show/hide of the gpui window without activation.
use crate::placement::{Monitor, Rect};
use gpui_kit::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_NCRENDERING_POLICY, DWMWA_TRANSITIONS_FORCEDISABLED, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND};
use windows::Win32::Graphics::Gdi::{ClientToScreen, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
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
    // gpui creates popups with style 0, an overlapped window with a resize frame: DWM draws its top edge as a thin line
    // and its drop shadow under the whole window, which shows through the transparent margin as a dim rectangle
    SetWindowLongPtrW(n.0, GWL_STYLE, WS_POPUP.0 as isize);
    let off = DWMNCRP_DISABLED;
    let _ = DwmSetWindowAttribute(n.0, DWMWA_NCRENDERING_POLICY, &off as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&off) as u32);
    let _ = SetWindowPos(n.0, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
    // no DWM fade when the bubble is shown or hidden
    let no_fade = windows::core::BOOL(1);
    let _ = DwmSetWindowAttribute(n.0, DWMWA_TRANSITIONS_FORCEDISABLED, &no_fade as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&no_fade) as u32);
    // Windows 11 rounds and outlines windows: the window is only a transparent canvas, so no corners and no border
    let square = DWMWCP_DONOTROUND; let none = DWMWA_COLOR_NONE;
    let _ = DwmSetWindowAttribute(n.0, DWMWA_WINDOW_CORNER_PREFERENCE, &square as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&square) as u32);
    let _ = DwmSetWindowAttribute(n.0, DWMWA_BORDER_COLOR, &none as *const _ as *const core::ffi::c_void, std::mem::size_of_val(&none) as u32);
} }
/// `rc` is the area gpui draws in. `prepare` removes the frame; should a frame remain, the window rect is grown by it and
/// moved so that the client area still lands exactly on `rc`, else the bubble is cut off on the right and bottom.
pub fn show(n: &Native, rc: Rect) { unsafe {
    let (mut wr, mut cr, mut o) = (RECT::default(), RECT::default(), POINT::default());
    let (l, t, dw, dh) = if GetWindowRect(n.0, &mut wr).is_ok() && GetClientRect(n.0, &mut cr).is_ok() && ClientToScreen(n.0, &mut o).as_bool() {
        (o.x - wr.left, o.y - wr.top, (wr.right - wr.left) - cr.right, (wr.bottom - wr.top) - cr.bottom)
    } else { (0, 0, 0, 0) };
    let _ = SetWindowPos(n.0, Some(HWND_TOPMOST), rc.x.round() as i32 - l, rc.y.round() as i32 - t, rc.w.round() as i32 + dw, rc.h.round() as i32 + dh, SWP_NOACTIVATE | SWP_SHOWWINDOW);
} }
pub fn hide(n: &Native) { unsafe { let _ = ShowWindow(n.0, SW_HIDE); } }
pub fn describe(n: &Native) -> String { let mut rc = RECT::default(); unsafe { let _ = GetWindowRect(n.0, &mut rc); } format!("hwnd rect {:?}", rc) }
/// Debug capture is implemented for macOS only.
pub fn capture(_: &Native, _: &str) -> Result<(), String> { Err("--screenshot is macOS-only".into()) }
