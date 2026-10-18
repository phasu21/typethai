//! macOS backend: a CGEventTap (needs the Accessibility permission), CGEventKeyboardSetUnicodeString injection, a
//! non-activating NSPanel bubble at the caret (AXFocusedUIElement + AXSelectedTextRange + AXBoundsForRange, mouse
//! fallback), secure-input skip (IsSecureEventInputEnabled / AXSecureTextField), input-source switch to Thai/ABC after applying (TISSelectInputSource), tray.
#![allow(non_upper_case_globals)]
use crate::{app::{hotkey_for, App}, backspace::DeleteUnit, bridge::{self, UiMsg}, engine::{Action, Arrow, Boundary, Hotkey, InputLang, Key}, inputsrc::{fits, pick, Source}, keys::mac_keycode_to_us, placement::{Anchor, Rect}, rewrite};
use core_foundation::base::CFRange;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Instant;
use super::field::{self, Field};
use std::sync::atomic::Ordering;
use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::array::CFArrayRef;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGEventField, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType, CallbackResult, EventField};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::geometry::{CGPoint, CGRect};
use objc2::rc::autoreleasepool;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString};
use objc2_foundation::{NSArray, NSData, NSString};
use std::cell::RefCell;
use std::ffi::c_void;

const MAGIC: i64 = 0x5454_4841;
type AXUIElementRef = *const c_void;
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementSetAttributeValue(e: AXUIElementRef, attr: CFStringRef, value: CFTypeRef) -> i32;
    fn AXUIElementSetMessagingTimeout(e: AXUIElementRef, secs: f32) -> i32;
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCopyAttributeValue(e: AXUIElementRef, attr: CFStringRef, value: *mut CFTypeRef) -> i32;
    fn AXUIElementIsAttributeSettable(e: AXUIElementRef, attr: CFStringRef, settable: *mut u8) -> i32;
    fn AXUIElementCopyParameterizedAttributeValue(e: AXUIElementRef, attr: CFStringRef, param: CFTypeRef, value: *mut CFTypeRef) -> i32;
    fn AXValueGetValue(v: CFTypeRef, ty: u32, out: *mut c_void) -> bool;
    fn AXValueCreate(ty: u32, ptr: *const c_void) -> CFTypeRef;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn CGEventTapEnable(tap: *const c_void, enable: bool);
}
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
    fn TISCopyCurrentKeyboardInputSource() -> *const c_void;
    fn TISGetInputSourceProperty(src: *const c_void, key: CFStringRef) -> *const c_void;
    fn TISCreateInputSourceList(props: CFDictionaryRef, include_all: bool) -> CFArrayRef;
    fn TISSelectInputSource(src: *const c_void) -> i32;
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyLocalizedName: CFStringRef;
    static kTISPropertyInputSourceIsASCIICapable: CFStringRef;
    static kTISPropertyInputSourceIsSelectCapable: CFStringRef;
    static kTISPropertyInputSourceCategory: CFStringRef;
    static kTISCategoryKeyboardInputSource: CFStringRef;
}
const kAXValueCGPointType: u32 = 1;
const kAXValueCGSizeType: u32 = 2;
const kAXValueCGRectType: u32 = 3;
const kAXValueCFRangeType: u32 = 4;

struct State { app: App, worker: rewrite::Worker, last_pid: i64, tap_port: *const c_void, move_due: Option<std::time::Instant>, move_left: u8, last_move: Option<std::time::Instant>, last_src: &'static str,
    gate: field::Gate, ax: Sender<Sender<Field>>, field_rx: Option<Receiver<Field>>,
    /// When the next look at focus is due, and whether a keystroke armed it (a click, an app switch or a key may have
    /// moved focus, and the handler that saw the trigger cannot wait for AX).
    badge_due: Option<(std::time::Instant, bool)>,
    /// The event tap, kept for as long as this thread runs; None while macOS refuses to make one (no Accessibility).
    tap: Option<CGEventTap<'static>>,
    /// When the tap was last tried while it was refused (once a second at most).
    tap_try: Option<std::time::Instant> }
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; static TX: RefCell<Option<Sender<UiMsg>>> = const { RefCell::new(None) }; }
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> { ST.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f))) }

// ---------- AX helpers ----------
unsafe fn ax_attr(e: AXUIElementRef, name: &str) -> Option<CFType> {
    let mut v: CFTypeRef = std::ptr::null(); let n = CFString::new(name);
    if AXUIElementCopyAttributeValue(e, n.as_concrete_TypeRef(), &mut v) == 0 && !v.is_null() { Some(CFType::wrap_under_create_rule(v)) } else { None }
}
unsafe fn ax_settable(e: AXUIElementRef, name: &str) -> bool {
    let mut b = 0u8; let n = CFString::new(name);
    AXUIElementIsAttributeSettable(e, n.as_concrete_TypeRef(), &mut b) == 0 && b != 0
}
unsafe fn focused() -> Option<CFType> { let sys = AXUIElementCreateSystemWide(); let r = ax_attr(sys, "AXFocusedUIElement"); CFRelease(sys as CFTypeRef); r }
unsafe fn secure_now() -> bool {
    if IsSecureEventInputEnabled() { return true }
    let Some(f) = focused() else { return false };
    ax_attr(f.as_CFTypeRef() as AXUIElementRef, "AXSubrole").and_then(|s| s.downcast::<CFString>()).map(|s| s.to_string() == "AXSecureTextField").unwrap_or(false)
}
unsafe fn input_source_is_thai() -> bool {
    let src = TISCopyCurrentKeyboardInputSource(); if src.is_null() { return false }
    let id = TISGetInputSourceProperty(src, kTISPropertyInputSourceID);
    let thai = !id.is_null() && CFString::wrap_under_get_rule(id as CFStringRef).to_string().to_lowercase().contains("thai");
    CFRelease(src as CFTypeRef); thai
}
unsafe fn tis_str(src: *const c_void, key: CFStringRef) -> String {
    let v = TISGetInputSourceProperty(src, key); if v.is_null() { String::new() } else { CFString::wrap_under_get_rule(v as CFStringRef).to_string() }
}
unsafe fn tis_bool(src: *const c_void, key: CFStringRef) -> bool {
    let v = TISGetInputSourceProperty(src, key); !v.is_null() && CFBoolean::wrap_under_get_rule(v as _) == CFBoolean::true_value()
}
/// Enabled keyboard input sources (read-only), with their TIS objects for selection.
unsafe fn tis_sources() -> Vec<(Source, CFType)> {
    let props = CFDictionary::from_CFType_pairs(&[(CFString::wrap_under_get_rule(kTISPropertyInputSourceCategory), CFString::wrap_under_get_rule(kTISCategoryKeyboardInputSource).as_CFType())]);
    let list = TISCreateInputSourceList(props.as_concrete_TypeRef(), false); if list.is_null() { return vec![] }
    let arr: core_foundation::array::CFArray<CFType> = core_foundation::array::CFArray::wrap_under_create_rule(list);
    arr.iter().map(|o| { let p = o.as_CFTypeRef();
        (Source { id: tis_str(p, kTISPropertyInputSourceID), name: tis_str(p, kTISPropertyLocalizedName), ascii: tis_bool(p, kTISPropertyInputSourceIsASCIICapable), selectable: tis_bool(p, kTISPropertyInputSourceIsSelectCapable) }, o.clone())
    }).collect()
}
pub fn input_sources() -> Vec<Source> { unsafe { tis_sources().into_iter().map(|(s, _)| s).collect() } }
/// Main thread only (see `bridge::INPUT_THAI`): make a switch the tap asked for, then publish the current source. A
/// change (the user's or ours) asks the tap thread for the badge, as the caret is read there.
pub fn sync_input_source() {
    static WATCH: std::sync::Mutex<crate::badge::Watch> = std::sync::Mutex::new(crate::badge::Watch::new());
    let req = bridge::REQ_SWITCH_INPUT.lock().unwrap().take();
    let thai = unsafe { if let Some(lang) = req { switch_input(lang) } input_source_is_thai() };
    bridge::INPUT_THAI.store(thai, Ordering::SeqCst);
    if let Some(lang) = WATCH.lock().unwrap().see(0, thai) { *bridge::REQ_BADGE.lock().unwrap() = Some(lang) }
}
/// Switch to the installed source fitting `lang`, unless the current one already fits. Warns once per language if none.
unsafe fn switch_input(lang: InputLang) {
    let cur = TISCopyCurrentKeyboardInputSource();
    if !cur.is_null() {
        let fit = fits(&tis_str(cur, kTISPropertyInputSourceID), tis_bool(cur, kTISPropertyInputSourceIsASCIICapable), lang);
        CFRelease(cur as CFTypeRef); if fit { return }
    }
    let all = tis_sources();
    let srcs: Vec<Source> = all.iter().map(|(s, _)| s.clone()).collect();
    match pick(&srcs, lang) {
        Some(t) => { if let Some((_, o)) = all.iter().find(|(s, _)| s.id == t.id) { TISSelectInputSource(o.as_CFTypeRef()); } }
        None => { static WARNED: std::sync::Once = std::sync::Once::new(); WARNED.call_once(|| eprintln!("typeThai: no enabled {lang:?} input source to switch to (add one in System Settings > Keyboard > Input Sources)")); }
    }
}

// ---------- injection ----------
fn post_key(src: &CGEventSource, code: u16, down: bool, flags: CGEventFlags) {
    if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), code, down) { e.set_flags(flags); e.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC); e.post(CGEventTapLocation::Session); }
}
fn tap(src: &CGEventSource, code: u16) { post_key(src, code, true, CGEventFlags::CGEventFlagNull); post_key(src, code, false, CGEventFlags::CGEventFlagNull); }
/// Cmd+`code` with exactly the Command flag: a Private source, so the Ctrl+Option still held from the hotkey never leaks in.
fn command(code: u16) {
    let Ok(src) = CGEventSource::new(CGEventSourceStateID::Private) else { return };
    post_key(&src, code, true, CGEventFlags::CGEventFlagCommand); post_key(&src, code, false, CGEventFlags::CGEventFlagCommand);
}
fn inject(delete: usize, insert: &str, then: Option<Boundary>) {
    let Ok(src) = CGEventSource::new(CGEventSourceStateID::Private) else { return };
    for _ in 0..delete { tap(&src, 0x33); }
    let u: Vec<u16> = insert.encode_utf16().collect();
    for chunk in u.chunks(16) { // CGEventKeyboardSetUnicodeString handles <= 20 UTF-16 units per event
        for down in [true, false] { if let Ok(e) = CGEvent::new_keyboard_event(src.clone(), 0, down) {
            e.set_flags(CGEventFlags::CGEventFlagNull); e.set_string_from_utf16_unchecked(chunk); e.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, MAGIC); e.post(CGEventTapLocation::Session);
        } }
    }
    match then { Some(Boundary::Space) => tap(&src, 0x31), Some(Boundary::Enter) => tap(&src, 0x24), Some(Boundary::Tab) => tap(&src, 0x30), None => {} }
}

// ---------- bubble (drawn by the gpui UI thread; we only send it the items + where the text is) ----------
unsafe fn ax_rect_value(v: &CFType) -> Option<CGRect> {
    let mut r = CGRect::new(&CGPoint::new(0.0, 0.0), &core_graphics::geometry::CGSize::new(0.0, 0.0));
    if AXValueGetValue(v.as_CFTypeRef(), kAXValueCGRectType, &mut r as *mut _ as *mut c_void) { Some(r) } else { None }
}
unsafe fn bounds_for(fe: AXUIElementRef, loc: isize, len: isize) -> Option<CGRect> {
    let range = CFRange { location: loc, length: len };
    let param = AXValueCreate(kAXValueCFRangeType, &range as *const _ as *const c_void); if param.is_null() { return None }
    let param = CFType::wrap_under_create_rule(param);
    let mut v: CFTypeRef = std::ptr::null(); let n = CFString::new("AXBoundsForRange");
    if AXUIElementCopyParameterizedAttributeValue(fe, n.as_concrete_TypeRef(), param.as_CFTypeRef(), &mut v) != 0 || v.is_null() { return None }
    ax_rect_value(&CFType::wrap_under_create_rule(v)).filter(|r| r.size.height > 0.0 && (r.origin.x != 0.0 || r.origin.y != 0.0))
}
/// Chromium and WebKit text: the caret from the selected text-marker range. Their `AXBoundsForRange` can answer a
/// zero-size rect at x 0 (Electron apps such as Grok Bot), while this API gives the real position.
unsafe fn marker_caret(fe: AXUIElementRef) -> Option<Rect> {
    let tm = ax_attr(fe, "AXSelectedTextMarkerRange")?;
    let mut v: CFTypeRef = std::ptr::null(); let n = CFString::new("AXBoundsForTextMarkerRange");
    if AXUIElementCopyParameterizedAttributeValue(fe, n.as_concrete_TypeRef(), tm.as_CFTypeRef(), &mut v) != 0 || v.is_null() { return None }
    let r = ax_rect_value(&CFType::wrap_under_create_rule(v)).filter(|r| r.size.height > 0.0 && (r.origin.x != 0.0 || r.origin.y != 0.0))?;
    Some(Rect::new(r.origin.x + r.size.width, r.origin.y, 1.0, r.size.height))
}
// ---------- caret lookup ----------
/// Frontmost app: (pid, bundle id, name).
fn frontmost() -> Option<(i32, String, String)> {
    #[allow(unused_unsafe)]
    unsafe {
        let ws = objc2_app_kit::NSWorkspace::sharedWorkspace();
        let app = ws.frontmostApplication()?;
        Some((app.processIdentifier(), app.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(), app.localizedName().map(|s| s.to_string()).unwrap_or_default()))
    }
}
/// Bundle id and display name of the app a key goes to (the frontmost app when the event names no pid); empty when
/// unknown.
fn app_of(pid: i64) -> (String, String) {
    autoreleasepool(|_| {
        let app = if pid > 0 { objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32) }
            else { objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication() };
        let Some(a) = app else { return (String::new(), String::new()) };
        (a.bundleIdentifier().map(|s| s.to_string()).unwrap_or_default(), a.localizedName().map(|s| s.to_string()).unwrap_or_default())
    })
}
fn is_chromium_browser(bundle: &str) -> bool {
    ["com.google.Chrome", "org.chromium", "com.brave.Browser", "com.microsoft.edgemac", "company.thebrowser", "com.vivaldi", "com.operasoftware"].iter().any(|b| bundle.starts_with(b))
}
static ENABLED: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
/// Pids whose AX tree is built on request: Electron (took AXManualAccessibility) and Chromium browsers.
static LAZY: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
fn lazy_tree(pid: i32) -> bool { LAZY.lock().unwrap().contains(&pid) }
/// Chromium / Electron build their accessibility tree only on request: set AXManualAccessibility (Electron: Slack,
/// VS Code, Discord, …; ignored by native apps) and, for Chromium browsers only, AXEnhancedUserInterface
/// (it changes window animations elsewhere, so not set for every app). Once per pid.
unsafe fn enable_ax_tree(app: AXUIElementRef, pid: i32, bundle: &str) {
    let mut done = ENABLED.lock().unwrap();
    if done.contains(&pid) { return }
    done.push(pid); if done.len() > 64 { done.remove(0); }
    let t = CFBoolean::true_value();
    let r1 = AXUIElementSetAttributeValue(app, CFString::new("AXManualAccessibility").as_concrete_TypeRef(), t.as_CFTypeRef());
    let r2 = if is_chromium_browser(bundle) { AXUIElementSetAttributeValue(app, CFString::new("AXEnhancedUserInterface").as_concrete_TypeRef(), t.as_CFTypeRef()) } else { 1 };
    if r1 == 0 || is_chromium_browser(bundle) { let mut l = LAZY.lock().unwrap(); l.push(pid); if l.len() > 64 { l.remove(0); } }
    eprintln!("typethai: AX tree enable pid {pid} {bundle}: AXManualAccessibility={} AXEnhancedUserInterface={}", r1 == 0, if is_chromium_browser(bundle) { (r2 == 0).to_string() } else { "n/a".into() });
}
/// The app's AX element (release it), with a short messaging timeout and its AX tree switched on.
unsafe fn app_element(pid: i32, bundle: &str) -> AXUIElementRef { let e = AXUIElementCreateApplication(pid); AXUIElementSetMessagingTimeout(e, 0.25); enable_ax_tree(e, pid, bundle); e }
/// Focused element now: (secure, text-field verdict). Goes through the frontmost app's element so its AX tree is on;
/// right after switching it on the verdict is Unknown (Chromium / Electron build the tree lazily).
unsafe fn focus_check() -> (bool, Field) {
    if IsSecureEventInputEnabled() { return (true, Field::Unknown) }
    autoreleasepool(|_| {
        let Some((pid, bundle, _)) = frontmost() else { return (false, Field::Unknown) };
        let fresh = !ENABLED.lock().unwrap().contains(&pid);
        let app = app_element(pid, &bundle);
        let s = |e: AXUIElementRef, a: &str| ax_attr(e, a).and_then(|v| v.downcast::<CFString>()).map(|v| v.to_string());
        let r = match focused_element(Some(app)) {
            Some(f) => {
                let fe = f.as_CFTypeRef() as AXUIElementRef; let role = s(fe, "AXRole").unwrap_or_default();
                let range = ax_attr(fe, "AXSelectedTextRange").is_some();
                // two more round trips, only when the role does not decide
                let editable = range && !field::TEXT_ROLES.contains(&role.as_str()) && (ax_settable(fe, "AXSelectedTextRange") || ax_settable(fe, "AXValue"));
                (s(fe, "AXSubrole").as_deref() == Some("AXSecureTextField"), field::ax(Some(&role), range, editable, true, lazy_tree(pid)))
            }
            None => (false, field::ax(None, false, false, ax_attr(app, "AXFocusedWindow").is_some(), lazy_tree(pid))),
        };
        CFRelease(app as CFTypeRef);
        let f = if field::known_text_app(&bundle) { Field::Text } else if fresh && r.1 != Field::Text { Field::Unknown } else { r.1 };
        (r.0, f)
    })
}
/// Closed-gate re-checks off the tap thread (AX calls are synchronous IPC to the focused app).
fn ax_worker() -> Sender<Sender<Field>> {
    let (tx, rx) = std::sync::mpsc::channel::<Sender<Field>>();
    let _ = std::thread::Builder::new().name("typethai-ax".into()).spawn(move || for reply in rx { let _ = reply.send(unsafe { focus_check() }.1); });
    tx
}
/// The next key picks the reply up (`field_collect`).
fn field_refresh(st: &mut State) {
    let (tx, rx) = std::sync::mpsc::channel();
    if st.ax.send(tx).is_ok() { st.field_rx = Some(rx) } else { st.gate.answer(None, Instant::now()) }
}
/// Takes the off-thread reply if it is in; never waits (see `Gate::must_ask`).
fn field_collect(st: &mut State) {
    let Some(rx) = st.field_rx.take() else { return };
    match rx.try_recv() {
        Ok(f) => st.gate.answer(Some(f), Instant::now()),
        Err(std::sync::mpsc::TryRecvError::Empty) => st.field_rx = Some(rx),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => st.gate.answer(None, Instant::now()),
    }
}
/// Focused element: system-wide AXFocusedUIElement, else the frontmost app's AXFocusedUIElement.
unsafe fn focused_element(app: Option<AXUIElementRef>) -> Option<CFType> {
    focused().or_else(|| app.and_then(|a| ax_attr(a, "AXFocusedUIElement")))
}
unsafe fn ax_point(v: &CFType) -> Option<CGPoint> { let mut pt = CGPoint::new(0.0, 0.0); if AXValueGetValue(v.as_CFTypeRef(), kAXValueCGPointType, &mut pt as *mut _ as *mut c_void) { Some(pt) } else { None } }
unsafe fn ax_size(v: &CFType) -> Option<core_graphics::geometry::CGSize> { let mut s = core_graphics::geometry::CGSize::new(0.0, 0.0); if AXValueGetValue(v.as_CFTypeRef(), kAXValueCGSizeType, &mut s as *mut _ as *mut c_void) { Some(s) } else { None } }
unsafe fn selected_range(fe: AXUIElementRef) -> Option<CFRange> {
    let rv = ax_attr(fe, "AXSelectedTextRange")?;
    let mut range = CFRange { location: 0, length: 0 };
    AXValueGetValue(rv.as_CFTypeRef(), kAXValueCFRangeType, &mut range as *mut _ as *mut c_void).then_some(range)
}
/// Caret rect from the selected range. See `placement::caret_top` for the one-line AppKit correction.
unsafe fn caret_rect(fe: AXUIElementRef) -> Option<(Rect, &'static str)> {
    let range = selected_range(fe)?;
    if range.length > 0 {
        if let Some(r) = bounds_for(fe, range.location + range.length - 1, 1) {
            return Some((Rect::new(r.origin.x + r.size.width, r.origin.y, 1.0, r.size.height), "range-end"));
        }
    }
    let zero = bounds_for(fe, range.location, 0);
    let prev = if range.location > 0 { bounds_for(fe, range.location - 1, 1) } else { None };
    let next = bounds_for(fe, range.location, 1);
    let glyphs: Vec<f64> = [prev, next].into_iter().flatten().map(|r| r.origin.y).collect();
    if let Some(z) = zero {
        let y = crate::placement::caret_top(z.origin.y, &glyphs);
        let h = [prev, next].into_iter().flatten().find(|r| (r.origin.y - y).abs() < 0.5).map(|r| r.size.height).unwrap_or(z.size.height);
        let how = if (y - z.origin.y).abs() < 0.5 { "range-0" } else { "glyph-line" };
        return Some((Rect::new(z.origin.x, y, 1.0, h), how));
    }
    prev.map(|r| (Rect::new(r.origin.x + r.size.width, r.origin.y, 1.0, r.size.height), "prev-char"))
        .or_else(|| next.map(|r| (Rect::new(r.origin.x, r.origin.y, 1.0, r.size.height), "next-char")))
        .or_else(|| marker_caret(fe).map(|r| (r, "text-marker")))
}
/// Caret rect + focused element frame + mouse (global top-left points), plus a description of what was found.
pub fn caret_anchor() -> (Anchor, String) {
    unsafe {
        let mut a = Anchor { caret: None, field: None, mouse: mouse_location() };
        let fm = frontmost();
        let app = fm.as_ref().map(|(pid, b, _)| app_element(*pid, b));
        let appname = fm.as_ref().map(|(_, b, n)| format!("{n} ({b})")).unwrap_or_else(|| "?".into());
        let mut how = "no-focused-element";
        if let Some(f) = focused_element(app) {
            let fe = f.as_CFTypeRef() as AXUIElementRef;
            if let (Some(p), Some(sz)) = (ax_attr(fe, "AXPosition").and_then(|v| ax_point(&v)), ax_attr(fe, "AXSize").and_then(|v| ax_size(&v))) {
                a.field = Some(Rect::new(p.x, p.y, sz.width, sz.height));
            }
            how = "no-caret-bounds";
            if let Some((mut c, mut h)) = caret_rect(fe) {
                // No neighboring glyph (empty field): AppKit's empty range still sits one line above the frame.
                if let Some(f) = a.field {
                    if c.bottom() <= f.y + 1.0 && c.y + c.h < f.bottom() { c = Rect::new(c.x, c.y + c.h, c.w, c.h); h = "above-field"; }
                }
                let mons = crate::ui::native::monitors_any_thread();
                if crate::placement::plausible_caret(c, a.field, &mons) { a.caret = Some(c); how = h } else { how = "caret-implausible" }
            }
        }
        // nothing to go by (Electron with its tree off): the click that focused the input, if it was in this window
        if a.caret.is_none() && a.field.is_none() {
            let win = app.and_then(|e| ax_attr(e, "AXFocusedWindow")).and_then(|w| { let w = w.as_CFTypeRef() as AXUIElementRef;
                let (p, sz) = (ax_attr(w, "AXPosition").and_then(|v| ax_point(&v))?, ax_attr(w, "AXSize").and_then(|v| ax_size(&v))?); Some(Rect::new(p.x, p.y, sz.width, sz.height)) });
            a.mouse = crate::placement::pointer_at(*LAST_CLICK.lock().unwrap(), win, a.mouse);
        }
        if let Some(e) = app { CFRelease(e as CFTypeRef); }
        let src = crate::placement::source_of(&a, &crate::placement::Opts::default()).map(|s| s.name()).unwrap_or("none");
        (a, format!("app={appname} source={src} ({how}) caret={:?} field={:?} mouse={:?}", a.caret, a.field, a.mouse))
    }
}
pub fn ax_trusted() -> bool { unsafe { AXIsProcessTrusted() } }
unsafe fn anchor() -> Anchor {
    let (a, info) = caret_anchor();
    let src = crate::placement::source_of(&a, &crate::placement::Opts::default()).map(|s| s.name()).unwrap_or("none");
    with(|st| if st.last_src != src { st.last_src = src; eprintln!("typethai: bubble anchor {info}"); });
    a
}
/// Where the last click outside the bubble landed (global top-left points).
static LAST_CLICK: std::sync::Mutex<Option<(f64, f64)>> = std::sync::Mutex::new(None);
fn mouse_location() -> Option<(f64, f64)> {
    let s = CGEventSource::new(CGEventSourceStateID::CombinedSessionState).ok()?; let e = CGEvent::new(s).ok()?; let p = e.location(); Some((p.x, p.y))
}
fn send(m: UiMsg) { TX.with(|t| if let Some(t) = t.borrow().as_ref() { let _ = t.send(m); }) }
/// Schedule a few caret re-reads. The event tap runs before the focused app lays out the new glyph, so the first
/// AX rect is still the previous caret; later samples land on it. Does not restart a follow that's already armed.
fn arm_caret_follow(st: &mut State) {
    if st.move_due.is_none() {
        st.move_left = 3;
        st.move_due = Some(std::time::Instant::now() + std::time::Duration::from_millis(30));
    }
}
fn run_actions(st: &mut State, acts: Vec<Action>) {
    for a in acts { match a {
        Action::Replace { delete, insert, then } => inject(delete, &insert, then),
        Action::SwitchInput(l) => *bridge::REQ_SWITCH_INPUT.lock().unwrap() = Some(l),
        Action::Select(i) => send(UiMsg::Select(i)),
        Action::Focus(f) => send(UiMsg::Focus(f)),
        Action::ShowBubble(items) => {
            send(UiMsg::show(items, st.app.engine.opts.bubble, unsafe { anchor() }));
            // the tap sees the key before the app moves its caret
            arm_caret_follow(st);
        }
        Action::HideBubble => { bridge::set_geom(None); send(UiMsg::Hide) }
        Action::Rewrite(job) => st.worker.submit(job),
        Action::Badge(on) => if on { if offer_badge(st, true) { arm_caret_follow(st) } } else { send(UiMsg::UnpinBadge) },
        Action::Hold(hold) => { let anchor = hold.filter(|_| !st.app.engine.bubble_visible()).map(|_| unsafe { anchor() }); send(UiMsg::Hold { hold, anchor }) }
    } }
}

// ---------- rewrite: selection + clipboard, on the desk thread ----------
struct MacHost;
impl rewrite::Host for MacHost {
    /// Every pasteboard item as its (type, bytes) pairs.
    type Snap = Vec<Vec<(String, Vec<u8>)>>;
    fn is_secure(&mut self) -> bool { unsafe { secure_now() } }
    fn selection_via_accessibility(&mut self) -> Option<String> {
        autoreleasepool(|_| unsafe {
            let app = frontmost().map(|(pid, b, _)| app_element(pid, &b));
            let r = focused_element(app).and_then(|f| {
                let fe = f.as_CFTypeRef() as AXUIElementRef;
                let t = ax_attr(fe, "AXSelectedText")?.downcast::<CFString>()?.to_string();
                // an empty AXSelectedText over a non-empty range means the app does not expose the text
                (!t.is_empty() || selected_range(fe).is_none_or(|r| r.length == 0)).then_some(t)
            });
            if let Some(e) = app { CFRelease(e as CFTypeRef); }
            r
        })
    }
    fn clip_seq(&mut self) -> u64 { NSPasteboard::generalPasteboard().changeCount() as u64 }
    fn clip_snapshot(&mut self) -> Self::Snap {
        autoreleasepool(|_| NSPasteboard::generalPasteboard().pasteboardItems().map(|items| items.iter().map(|it| it.types().iter().filter_map(|t| Some((t.to_string(), it.dataForType(&t)?.to_vec()))).collect()).collect()).unwrap_or_default())
    }
    fn clip_restore(&mut self, s: Self::Snap) {
        autoreleasepool(|_| {
            let items: Vec<_> = s.iter().map(|pairs| { let it = NSPasteboardItem::new(); for (t, d) in pairs { it.setData_forType(&NSData::with_bytes(d), &NSString::from_str(t)); } ProtocolObject::from_retained(it) }).collect();
            let pb = NSPasteboard::generalPasteboard(); pb.clearContents(); if !items.is_empty() { pb.writeObjects(&NSArray::from_retained_slice(&items)); }
        })
    }
    fn clip_read_text(&mut self) -> Option<String> { autoreleasepool(|_| NSPasteboard::generalPasteboard().stringForType(unsafe { NSPasteboardTypeString }).map(|s| s.to_string())) }
    fn clip_write_transient(&mut self, text: &str) {
        // nspasteboard.org markers: clipboard managers skip transient and concealed items
        autoreleasepool(|_| {
            let it = NSPasteboardItem::new(); it.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
            for marker in ["org.nspasteboard.TransientType", "org.nspasteboard.ConcealedType"] { it.setData_forType(&NSData::new(), &NSString::from_str(marker)); }
            let pb = NSPasteboard::generalPasteboard(); pb.clearContents(); pb.writeObjects(&NSArray::from_retained_slice(&[ProtocolObject::from_retained(it)]));
        })
    }
    fn post_copy(&mut self) { command(0x08) }
    fn post_paste(&mut self) { command(0x09) }
    fn pause(&mut self, ms: u64) { std::thread::sleep(std::time::Duration::from_millis(ms)) }
}

// ---------- event tap ----------
fn translate(code: u16, flags: u64) -> Option<Key> {
    let (shift, ctrl, alt, cmd, caps) = (flags & 0x20000 != 0, flags & 0x40000 != 0, flags & 0x80000 != 0, flags & 0x100000 != 0, flags & 0x10000 != 0);
    if ctrl && alt { if let Some(h) = mac_keycode_to_us(code, false, false).and_then(hotkey_for) { return Some(Key::Hotkey(h)) } }
    if code == 0x33 && cmd { return Some(Key::LineDelete) }
    if code == 0x33 && alt && !ctrl { return Some(Key::WordDelete) }
    if ctrl && !alt && !cmd && !shift { if let Some(d @ '1'..='3') = mac_keycode_to_us(code, false, false) { return Some(Key::PickRow(d as usize - '1' as usize)) } }
    if ctrl || alt || cmd { return Some(Key::Nav) }
    Some(match code {
        0x33 => Key::Backspace, 0x31 => Key::Boundary(Boundary::Space), 0x24 | 0x4C => Key::Boundary(Boundary::Enter),
        0x30 => if shift { Key::Nav } else { Key::Tab }, // Shift+Tab (reverse focus) stays plain navigation
        0x35 => Key::Esc,
        0x7E if shift => Key::SelectPrev, 0x7D if shift => Key::SelectNext,
        0x7E => Key::Arrow(Arrow::Up), 0x7D => Key::Arrow(Arrow::Down), 0x7B if !shift => Key::Arrow(Arrow::Left), 0x7C if !shift => Key::Arrow(Arrow::Right),
        c => match mac_keycode_to_us(c, shift, caps) { Some(ch) => Key::Char(ch), None => Key::Nav },
    })
}
fn on_event(ty: CGEventType, e: &CGEvent) -> CallbackResult {
    if e.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA) == MAGIC { return CallbackResult::Keep }
    let r = with(|st| match ty {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => { unsafe { CGEventTapEnable(st.tap_port, true) }; false }
        CGEventType::LeftMouseDown | CGEventType::RightMouseDown | CGEventType::OtherMouseDown => {
            let p = e.location();
            match bridge::geom().and_then(|g| g.hit(p.x, p.y, e.get_flags().bits() & 0x20000 != 0)) {
                // on the bubble: swallow so the click never reaches (or focuses) it; a left click on an option applies it,
                // on a thumb rates it
                Some(hit) => { if let (CGEventType::LeftMouseDown, Some(k)) = (ty, hit.key()) { let o = st.app.engine.on_key(k); run_actions(st, o.actions); st.app.maybe_save(); } true }
                None => { *LAST_CLICK.lock().unwrap() = Some((p.x, p.y)); st.gate.touch(Instant::now()); arm_focus_look(st, false); arm_caret_follow(st); let a = st.app.engine.reset(); run_actions(st, a); false }
            }
        }
        // Shift is a modifier: pressing or releasing it alone is a flags change, never a KeyDown/KeyUp. The bubble shows
        // its thumbs while it is held
        CGEventType::FlagsChanged => { bridge::SHIFT_HELD.store(e.get_flags().bits() & 0x20000 != 0, Ordering::SeqCst); false }
        CGEventType::KeyDown | CGEventType::KeyUp => {
            // every keystroke, whatever becomes of it: typing takes the input-language preview down
            if matches!(ty, CGEventType::KeyDown) { let a = st.app.engine.on_keystroke(Instant::now()); run_actions(st, a); }
            let pid = e.get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID as CGEventField);
            if typing_in_our_window(st, pid) { return false }
            let code = e.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let key = if matches!(ty, CGEventType::KeyUp) {
                // the only release that matters: Tab, while it is held over the bubble
                if code != 0x30 || !st.app.engine.is_bubble_key(&Key::TabUp) { return false }
                Key::TabUp
            } else { let Some(key) = translate(code, e.get_flags().bits()) else { return false }; key };
            let pid = e.get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID as CGEventField);
            if pid != st.last_pid { st.last_pid = pid; let (id, name) = app_of(pid); st.app.engine.delete_unit = DeleteUnit::of_app(&id); st.gate.reset(); st.field_rx = None; arm_focus_look(st, false); arm_caret_follow(st);
                // the tray's app item follows keys to another app, and turning it off there takes anything on screen down
                let mut a = if id.is_empty() { vec![] } else { st.app.set_front_app(&id, if name.is_empty() { &id } else { &name }) };
                // the new app's caret is somewhere else, and the pill may be up over the old one
                a.extend(st.app.engine.reset()); run_actions(st, a); }
            // typeThai is off in this app: every key reaches it untouched, before any AX work
            if st.app.engine.app_off() { return false }
            // these may move focus inside the app: a closed gate re-checks, and so does the preview's policy (a look
            // armed by a key never pins the preview: the caret under it is not laid out yet, and Enter/⌘C are typing)
            if matches!(key, Key::Tab | Key::Esc | Key::Nav | Key::PickRow(_) | Key::Boundary(Boundary::Enter)) { st.gate.touch(Instant::now()); arm_focus_look(st, true); }
            // Thai input active / secure field: the engine is bypassed (keys already produce Thai, nothing to convert),
            // except hotkeys and the bubble controls (Tab / Shift+Up/Down / Esc) while the bubble is up, and Space when it
            // applies a live row (`is_bubble_key`).
            // The active input source decides the direction: Thai active -> reverse detector (English typed on Kedmanee),
            // or bypass entirely when the reverse model is off (--no-reverse). Keys always arrive as US-QWERTY chars.
            let thai = bridge::INPUT_THAI.load(Ordering::SeqCst);
            st.app.engine.set_thai_active(thai);
            if !matches!(key, Key::Hotkey(_)) && !st.app.engine.is_bubble_key(&key) {
                let first = matches!(key, Key::Char(_)) && st.app.engine.buffer().is_empty();
                if thai && st.app.engine.rev.is_none() { let a = st.app.engine.reset(); run_actions(st, a); return false }
                // text fields only (games: WASD): the cached AX verdict. One look at each word start answers it and the
                // secure check; while closed, only after a click / focus key (or off-thread, `field_refresh`).
                let word_start = first || matches!(key, Key::Boundary(_) | Key::Tab);
                field_collect(st); let now = Instant::now(); let mut secure = false;
                if st.gate.must_ask(word_start, st.gate.open(false, || false)) { let (s, f) = unsafe { focus_check() }; secure = s; st.gate.answer(Some(f), now); }
                else if st.gate.refresh_due(now, true) { field_refresh(st); }
                if secure || !st.gate.open(false, || false) { let a = st.app.engine.reset(); run_actions(st, a); return false }
            }
            let was = st.app.engine.paused; let o = st.app.engine.on_key(key);
            if was != st.app.engine.paused { send(UiMsg::Paused(st.app.engine.paused)) }
            run_actions(st, o.actions); st.app.maybe_save();
            // caret moves with every keystroke: follow it while the bubble is up (after the app handled the key)
            if st.app.engine.bubble_visible() { arm_caret_follow(st); }
            o.swallow
        }
        _ => false,
    }).unwrap_or(false);
    if r { CallbackResult::Drop } else { CallbackResult::Keep }
}

/// Keys aimed at this process (the Settings window) are left alone, so the field can be typed in.
fn key_targets_us(target_pid: i64) -> bool {
    let us = std::process::id() as i64;
    if target_pid == us { return true }
    if target_pid > 0 { return false }
    frontmost().is_some_and(|(pid, _, _)| pid as i64 == us)
}
fn typing_in_our_window(st: &mut State, target_pid: i64) -> bool {
    if !key_targets_us(target_pid) { return false }
    let us = std::process::id() as i64;
    if st.last_pid != us {
        st.last_pid = us;
        // the Settings window has focus: no caret of ours to sit over, and the preview belongs to the app behind it
        send(UiMsg::HideBadge);
        let a = st.app.engine.reset();
        run_actions(st, a);
    }
    true
}
/// The input source now in effect, as the menu bar shows it.
fn source_lang() -> InputLang { if bridge::INPUT_THAI.load(Ordering::SeqCst) { InputLang::Thai } else { InputLang::Latin } }
/// Focus may have moved into or out of a text field (a click, an app switch, a focus key): `focus_look` asks AX once the
/// app has settled. The handler that saw the trigger cannot wait for it. `key`: a keystroke armed it, so this is not a
/// focus the user made (see `Engine::set_field`), and nothing is shown on the spot.
fn arm_focus_look(st: &mut State, key: bool) { st.badge_due = Some((Instant::now() + field::SETTLE, key)); }
/// One fresh look at Focus, then the preview's policy: a focus the user made (a click, an app switch) shows the preview
/// and keeps it up while nothing has been typed in that field; elsewhere, or in a password field, it goes. The verdict
/// is the gate's own rule (macOS has no caret flag: the focused element decides, and an unknown one opens).
fn focus_look(st: &mut State, key: bool) {
    // a click or Cmd+Tab may have brought another app forward: the tray names it, and its off state may differ
    if let Some((pid, bundle, name)) = frontmost() {
        if !bundle.is_empty() && pid as i64 != std::process::id() as i64 && pid as i64 != st.last_pid { let a = st.app.set_front_app(&bundle, if name.is_empty() { &bundle } else { &name }); run_actions(st, a); }
    }
    let (secure, f) = unsafe { focus_check() };
    let text = !secure && field::Gate::told(f).open(false, || false);
    let acts = if key { st.app.engine.set_field(text) } else { st.app.engine.on_focus(text) };
    run_actions(st, acts);
}
/// The input source changed: the badge at the caret, unless it is off, typeThai is paused, the Settings window is in
/// front, or focus is a password field or no text field (a fresh look, by the gate's rule). `pin`: focus just landed in
/// a text field, so it stays until typing or focus leaving; else it goes after a moment.
fn offer_badge(st: &mut State, pin: bool) -> bool {
    if st.app.engine.app_off() { return false }
    let lang = source_lang();
    let ours = key_targets_us(0);
    let (on, bubble) = (st.app.engine.opts.input_badge && !st.app.engine.paused && !ours, st.app.engine.bubble_visible());
    let blocked = || { let (secure, f) = unsafe { focus_check() }; secure || !field::Gate::told(f).open(false, || false) };
    let at = if pin { crate::badge::anchor_for(on, bubble, blocked, badge_anchor) } else { crate::badge::anchor_for(on, bubble, blocked, || unsafe { anchor() }) };
    match at { Some(anchor) => { send(UiMsg::Badge { lang, anchor, pin }); true } None => false }
}
/// The last caret / field the preview was placed from (the mouse is part of an `Anchor` but moves constantly, so it is
/// left out of the comparison).
static LAST_BADGE: std::sync::Mutex<Option<(Option<Rect>, Option<Rect>)>> = std::sync::Mutex::new(None);
/// The caret the preview sits on. The line is logged whenever it moves, so a pill that reads as misplaced can be
/// checked against what Accessibility reported (`--probe-caret` shows the same on screen for a moment).
fn badge_anchor() -> Anchor {
    let (a, info) = caret_anchor();
    let mut last = LAST_BADGE.lock().unwrap();
    let seen = (a.caret, a.field);
    if *last != Some(seen) { *last = Some(seen); eprintln!("typethai: preview anchor {info}") }
    a
}
fn service_settings(st: &mut State) {
    let cmds = bridge::take_settings();
    if cmds.is_empty() && !bridge::take(&bridge::REQ_SETTINGS) { return }
    let (snap, hide) = crate::prefs::apply_commands(&mut st.app, cmds);
    run_actions(st, hide);
    send(UiMsg::Settings(snap));
}
extern "C" fn timer_cb(_t: CFRunLoopTimerRef, _i: *mut c_void) {
    with(|st| {
        service_settings(st);
        while let Some(r) = st.worker.poll() { let a = st.app.engine.on_reply(r); run_actions(st, a); }
        if bridge::take(&bridge::REQ_TOGGLE_PAUSE) { let o = st.app.engine.on_key(Key::Hotkey(Hotkey::TogglePause)); run_actions(st, o.actions); send(UiMsg::Paused(st.app.engine.paused)); }
        let now = std::time::Instant::now();
        // refused earlier: try once a second, so a grant takes effect without a relaunch
        if st.tap_port.is_null() && st.tap_try.is_none_or(|t| now.duration_since(t) >= std::time::Duration::from_millis(1000)) {
            st.tap_try = Some(now);
            if arm_tap(st) { eprintln!("typeThai: event tap created"); send(UiMsg::Permission(false)) }
        }
        let idle = st.app.engine.on_idle(now); run_actions(st, idle);
        if st.gate.refresh_due(now, false) { field_refresh(st); }
        if let Some((_, key)) = st.badge_due.filter(|(t, _)| now >= *t) { st.badge_due = None; focus_look(st, key); }
        if bridge::REQ_BADGE.lock().unwrap().take().is_some() { offer_badge(st, false); }
        // a few AX reads after a key (the first is often still the pre-keystroke caret), and one when the preview goes
        // up: the app lays the caret out under it, and an app that scrolls (a terminal) moves it while the pill shows
        if st.move_due.is_some_and(|t| now >= t) && st.last_move.is_none_or(|t| now.duration_since(t).as_millis() >= 50) {
            st.move_due = None; st.last_move = Some(now);
            let follow = if st.app.engine.bubble_visible() { send(UiMsg::Move(unsafe { anchor() })); true }
                else if st.app.engine.badge_shown() { send(UiMsg::MoveBadge(badge_anchor())); true }
                else { false };
            if follow && st.move_left > 1 { st.move_left -= 1; st.move_due = Some(now + std::time::Duration::from_millis(70)); }
            else { st.move_left = 0; }
        }
        if bridge::take(&bridge::REQ_QUIT) { st.app.save_and_wait(); send(UiMsg::Stopped); CFRunLoop::get_current().stop(); }
    });
}

/// Capture thread: owns the Engine, the CGEventTap and its own CFRunLoop; talks to the gpui UI only via `tx`/statics.
/// `TYPETHAI_FAKE_UNTRUSTED=1`: behave as if macOS refused the tap, so the permission path can be walked by hand.
fn no_permission() -> bool { std::env::var_os("TYPETHAI_FAKE_UNTRUSTED").is_some() }

/// Make the event tap on this thread and arm it. False while macOS refuses one (no Accessibility for this build, or a
/// record saved by an older copy answering for us): the app then keeps running without it, says so, and retries, so
/// granting the permission takes effect without a relaunch.
fn arm_tap(st: &mut State) -> bool {
    st.tap = None; st.tap_port = std::ptr::null();
    if no_permission() { return false }
    let tap = match CGEventTap::new(CGEventTapLocation::Session, CGEventTapPlacement::HeadInsertEventTap, CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::FlagsChanged, CGEventType::LeftMouseDown, CGEventType::RightMouseDown, CGEventType::OtherMouseDown], |_p, ty, e| on_event(ty, e)) {
        Ok(t) => t,
        Err(_) => return false,
    };
    st.tap_port = tap.mach_port().as_concrete_TypeRef() as *const c_void;
    let src = tap.mach_port().create_runloop_source(0).expect("runloop source");
    CFRunLoop::get_current().add_source(&src, unsafe { kCFRunLoopCommonModes });
    tap.enable();
    st.tap = Some(tap);
    true
}

/// The bundle id macOS judges this process by. None for an unbundled build (a dev binary started from a terminal): its
/// permissions are the app's that started it, so it can neither read nor repair its own record.
fn bundle_id() -> Option<String> { std::env::var("__CFBundleIdentifier").ok().filter(|s| !s.is_empty()) }

/// Forget this app's own Accessibility record, once per version, and ask for it again. A record saved by an older copy
/// (signed ad-hoc, or by the pre-0.5 certificate) keeps answering for the build in front of the user: the switch looks
/// on and nothing works, and every launch asks again. Only this app's identifier is touched, and only when the app is
/// the installed bundle (`tccutil` would otherwise clear the installed copy's record for a dev build's sake, which is
/// also why the marker lives beside the state file).
fn repair_permission_once(state: &std::path::Path, id: &str, version: &str) {
    let marker = state.with_file_name("permission-repair");
    if std::fs::read_to_string(&marker).is_ok_and(|seen| seen.trim() == version) { return }
    eprintln!("typeThai: no event tap; clearing {id}'s own Accessibility record so it can be granted again");
    let _ = std::process::Command::new("/usr/bin/tccutil").args(["reset", "Accessibility", id]).status();
    if let Some(dir) = marker.parent() { let _ = std::fs::create_dir_all(dir); }
    let _ = std::fs::write(&marker, version);
}

fn capture_thread(app: App, tx: Sender<UiMsg>) {
    TX.with(|t| *t.borrow_mut() = Some(tx.clone()));
    ST.with(|s| *s.borrow_mut() = Some(State { app, worker: rewrite::Worker::spawn(|| MacHost), last_pid: -1, tap_port: std::ptr::null(), tap: None, tap_try: None, move_due: None, move_left: 0, last_move: None, last_src: "", gate: field::Gate::default(), ax: ax_worker(), field_rx: None, badge_due: Some((std::time::Instant::now(), false)) }));
    let armed = with(|st| arm_tap(st)).unwrap_or(false);
    if !armed {
        eprintln!("typeThai: no event tap yet; running without it and trying again once Accessibility is granted");
        let _ = tx.send(UiMsg::Permission(true));
    }
    let timer = CFRunLoopTimer::new(core_foundation::date::CFDate::now().abs_time() + 0.2, 0.025, 0, 0, timer_cb, std::ptr::null_mut());
    CFRunLoop::get_current().add_timer(&timer, unsafe { kCFRunLoopCommonModes });
    CFRunLoop::run_current();
}

/// Ask macOS for Accessibility, showing its prompt once per process.
pub fn request_permission() {
    unsafe {
        let opts = CFDictionary::from_CFType_pairs(&[(CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt), CFBoolean::true_value().as_CFType())]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef());
    }
    let _ = std::process::Command::new("/usr/bin/open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility").status();
}

pub fn run(app: App) {
    if unsafe { !AXIsProcessTrusted() } && !no_permission() {
        // a record that does not match this build is the reported "the switch is on and nothing works": clear our own
        // once per version, then ask again. The capture thread keeps retrying, so no relaunch is needed afterwards.
        if let Some(id) = bundle_id() { repair_permission_once(&app.state_path, &id, env!("CARGO_PKG_VERSION")) }
        request_permission();
        eprintln!("typeThai needs Accessibility permission: System Settings > Privacy & Security > Accessibility.");
    }
    sync_input_source();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("typethai-capture".into()).spawn(move || capture_thread(app, tx)).expect("capture thread");
    crate::ui::run(rx); // gpui owns the main thread / NSApplication run loop
}
