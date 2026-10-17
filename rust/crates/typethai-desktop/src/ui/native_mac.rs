//! macOS: monitor list from NSScreen (converted to top-left global points) and native show/hide of the gpui window.
use crate::placement::{Monitor, Rect};
use gpui_kit::Window;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSScreen, NSView, NSWindowStyleMask};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

fn mtm() -> MainThreadMarker { MainThreadMarker::new().expect("UI must run on the main thread") }
fn primary_h() -> f64 { NSScreen::screens(mtm()).firstObject().map(|s| s.frame().size.height).unwrap_or(900.0) }
fn flip(r: NSRect, ph: f64) -> Rect { Rect::new(r.origin.x, ph - (r.origin.y + r.size.height), r.size.width, r.size.height) }

/// The user's accent colour (System Settings > Appearance), as sRGB 0..1. Main thread.
pub fn accent() -> Option<(f32, f32, f32)> {
    let c = objc2_app_kit::NSColor::controlAccentColor().colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())?;
    Some((c.redComponent() as f32, c.greenComponent() as f32, c.blueComponent() as f32))
}
/// Monitors from any thread (the capture thread uses it to sanity-check AX rects); empty off the main thread.
pub fn monitors_any_thread() -> Vec<Monitor> { if MainThreadMarker::new().is_some() { monitors() } else { MONS.lock().unwrap().clone() } }
static MONS: std::sync::Mutex<Vec<Monitor>> = std::sync::Mutex::new(Vec::new());
pub fn monitors() -> Vec<Monitor> { let m = monitors_main(); *MONS.lock().unwrap() = m.clone(); m }
fn monitors_main() -> Vec<Monitor> {
    let screens = NSScreen::screens(mtm()); let ph = primary_h();
    screens.iter().map(|s| Monitor { frame: flip(s.frame(), ph), visible: flip(s.visibleFrame(), ph), size_scale: 1.0, pixel: s.backingScaleFactor() }).collect()
}
/// Native handles of the bubble, taken once while gpui lends us the Window. Show/hide/resize are done through these
/// *outside* any gpui borrow (spawned on the foreground executor, like gpui's own `Window::resize`): AppKit delivers the resulting setFrameSize: synchronously, and gpui drops resize
/// callbacks for a window that is currently borrowed, which left gpui's viewport at the creation size (clipping).
#[derive(Clone)]
pub struct Native { win: objc2::rc::Retained<objc2_app_kit::NSWindow>, view: objc2::rc::Retained<NSView> }
pub fn native(window: &Window) -> Option<Native> {
    let h = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(a) = h.as_raw() else { return None };
    let view: &NSView = unsafe { &*(a.ns_view.as_ptr() as *const NSView) };
    Some(Native { win: view.window()?, view: objc2::Message::retain(view) })
}
pub fn prepare(n: &Native) {
    let w = &n.win;
    // gpui gives every window Titled|FullSizeContentView, which draws the system frame/rounded border around the content.
    // Make it a plain borderless non-activating panel: the gpui panel (rounded, bordered) is then the only visible surface.
    w.setStyleMask(NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel);
    w.setOpaque(false);
    w.setBackgroundColor(Some(&objc2_app_kit::NSColor::clearColor()));
    // gpui's CAMetalLayer must not be opaque either, or the area around what the bubble draws is filled
    unsafe {
        let layer: *mut objc2::runtime::AnyObject = objc2::msg_send![&*n.view, layer];
        if !layer.is_null() { let _: () = objc2::msg_send![layer, setOpaque: false]; }
    }
    // no window server shadow: it would outline the whole window; the bubble draws its own around what it shows
    w.setHidesOnDeactivate(false); w.setIgnoresMouseEvents(false); w.setHasShadow(false);
    // gpui gives PopUp windows the utility-window animation, which fades every orderOut / orderFront
    w.setAnimationBehavior(objc2_app_kit::NSWindowAnimationBehavior::None);
}
pub fn show(n: &Native, r: Rect) {
    let ph = primary_h();
    n.win.setFrame_display(NSRect::new(NSPoint::new(r.x, ph - r.y - r.h), NSSize::new(r.w, r.h)), true);
    n.view.setFrameSize(NSSize::new(r.w, r.h)); // no-op if AppKit already resized it
    n.win.orderFrontRegardless(); // shows without activating our app or making the panel key
}
pub fn hide(n: &Native) { n.win.orderOut(None); }


/// Debug: native frame + content view size.
pub fn describe(n: &Native) -> String {
    let w = &n.win; format!("ns frame {:?} view {:?} scale {} winnum {} animation {:?}", w.frame(), n.view.frame(), w.backingScaleFactor(), w.windowNumber(), w.animationBehavior())
}
/// Debug: capture just this window to a PNG with `screencapture -l` (no shadow).
/// Also writes `<path stem>-onscreen.png`: the screen region around the window (+16 pt), showing corners and shadow
/// against whatever is behind it.
pub fn capture(n: &Native, path: &str) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let st = std::process::Command::new("/usr/sbin/screencapture").args(args).status().map_err(|e| e.to_string())?;
        if st.success() { Ok(()) } else { Err(format!("screencapture exit {st}")) }
    };
    run(&["-x", "-o", &format!("-l{}", n.win.windowNumber()), path])?;
    let f = n.win.frame(); let ph = primary_h(); let m = 16.0;
    let region = format!("-R{},{},{},{}", f.origin.x - m, ph - f.origin.y - f.size.height - m, f.size.width + 2.0 * m, f.size.height + 2.0 * m);
    let on = match path.strip_suffix(".png") { Some(s) => format!("{s}-onscreen.png"), None => format!("{path}-onscreen.png") };
    run(&["-x", &region, &on])
}
