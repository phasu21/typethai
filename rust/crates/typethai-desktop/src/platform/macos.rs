//! macOS backend: a CGEventTap (needs the Accessibility permission), CGEventKeyboardSetUnicodeString injection, a
//! non-activating NSPanel bubble at the caret (AXFocusedUIElement + AXSelectedTextRange + AXBoundsForRange, mouse
//! fallback), secure-input skip (IsSecureEventInputEnabled / AXSecureTextField), input-source switch to Thai/ABC after applying (TISSelectInputSource), tray.
#![allow(non_upper_case_globals)]
use crate::{app::{hotkey_for, App}, backspace::DeleteUnit, bridge::{self, UiMsg}, engine::{Action, Arrow, Boundary, Hotkey, InputLang, Key}, inputsrc::{fits, pick, Source}, keys::mac_keycode_to_us, placement::{Anchor, Rect}, rewrite};
use core_foundation::base::CFRange;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
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
    gate: field::Gate, ax: Sender<Sender<Field>>, field_rx: Option<Receiver<Field>> }
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; static TX: RefCell<Option<Sender<UiMsg>>> = const { RefCell::new(None) }; }
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> { ST.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f))) }

// ---------- AX helpers ----------
unsafe fn ax_attr(e: AXUIElementRef, name: &str) -> Option<CFType> {
    let mut v: CFTypeRef = std::ptr::null(); let n = CFString::new(name);
    if AXUIElementCopyAttributeValue(e, n.as_concrete_TypeRef(), &mut v) == 0 && !v.is_null() { Some(CFType::wrap_under_create_rule(v)) } else { None }
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
/// Main thread only (see `bridge::INPUT_THAI`): make a switch the tap asked for, then publish the current source.
pub fn sync_input_source() {
    let req = bridge::REQ_SWITCH_INPUT.lock().unwrap().take();
    unsafe { if let Some(lang) = req { switch_input(lang) } bridge::INPUT_THAI.store(input_source_is_thai(), Ordering::SeqCst); }
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
/// Bundle id of the app a key goes to (the frontmost app when the event names no pid); empty when unknown.
fn bundle_of(pid: i64) -> String {
    autoreleasepool(|_| {
        let app = if pid > 0 { objc2_app_kit::NSRunningApplication::runningApplicationWithProcessIdentifier(pid as i32) }
            else { objc2_app_kit::NSWorkspace::sharedWorkspace().frontmostApplication() };
        app.and_then(|a| a.bundleIdentifier()).map(|s| s.to_string()).unwrap_or_default()
    })
}
fn is_chromium_browser(bundle: &str) -> bool {
    ["com.google.Chrome", "org.chromium", "com.brave.Browser", "com.microsoft.edgemac", "company.thebrowser", "com.vivaldi", "com.operasoftware"].iter().any(|b| bundle.starts_with(b))
}
static ENABLED: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());
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
                (s(fe, "AXSubrole").as_deref() == Some("AXSecureTextField"), field::ax(Some(&role), ax_attr(fe, "AXSelectedTextRange").is_some(), true))
            }
            None => (false, field::ax(None, false, ax_attr(app, "AXFocusedWindow").is_some())),
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
fn field_collect(st: &mut State) {
    let Some(rx) = st.field_rx.take() else { return };
    st.gate.answer(rx.recv_timeout(Duration::from_millis(60)).ok(), Instant::now());
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
            match bridge::geom().and_then(|g| g.hit(p.x, p.y)) {
                // on the bubble: swallow so the click never reaches (or focuses) it; left click on a row accepts
                Some(row) => { if let (CGEventType::LeftMouseDown, Some(i)) = (ty, row) { let o = st.app.engine.on_key(Key::Hotkey(Hotkey::Accept(i))); run_actions(st, o.actions); } true }
                None => { st.gate.touch(Instant::now()); let a = st.app.engine.reset(); run_actions(st, a); false }
            }
        }
        CGEventType::KeyDown | CGEventType::KeyUp => {
            let pid = e.get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID as CGEventField);
            if typing_in_our_window(st, pid) { return false }
            let code = e.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let key = if matches!(ty, CGEventType::KeyUp) {
                // the only release that matters: Tab, while it is held over the bubble
                if code != 0x30 || !st.app.engine.is_bubble_key(&Key::TabUp) { return false }
                Key::TabUp
            } else { let Some(key) = translate(code, e.get_flags().bits()) else { return false }; key };
            let pid = e.get_integer_value_field(EventField::EVENT_TARGET_UNIX_PROCESS_ID as CGEventField);
            if pid != st.last_pid { st.last_pid = pid; st.app.engine.delete_unit = DeleteUnit::of_app(&bundle_of(pid)); st.gate.reset(); st.field_rx = None; let a = st.app.engine.reset(); run_actions(st, a); }
            // these may move focus inside the app: a closed gate re-checks
            if matches!(key, Key::Tab | Key::Esc | Key::Nav | Key::PickRow(_) | Key::Boundary(Boundary::Enter)) { st.gate.touch(Instant::now()); }
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
        let a = st.app.engine.reset();
        run_actions(st, a);
    }
    true
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
        let idle = st.app.engine.on_idle(now); run_actions(st, idle);
        if st.gate.refresh_due(now, false) { field_refresh(st); }
        // a few AX reads after each key: the first is often still the pre-keystroke caret
        if st.move_due.is_some_and(|t| now >= t) && st.last_move.is_none_or(|t| now.duration_since(t).as_millis() >= 50) {
            st.move_due = None; st.last_move = Some(now);
            if st.app.engine.bubble_visible() {
                send(UiMsg::Move(unsafe { anchor() }));
                if st.move_left > 1 { st.move_left -= 1; st.move_due = Some(now + std::time::Duration::from_millis(70)); }
                else { st.move_left = 0; }
            } else { st.move_left = 0; }
        }
        if bridge::take(&bridge::REQ_QUIT) { st.app.save_and_wait(); send(UiMsg::Stopped); CFRunLoop::get_current().stop(); }
    });
}

/// Capture thread: owns the Engine, the CGEventTap and its own CFRunLoop; talks to the gpui UI only via `tx`/statics.
fn capture_thread(app: App, tx: Sender<UiMsg>) {
    TX.with(|t| *t.borrow_mut() = Some(tx.clone()));
    let tap = CGEventTap::new(CGEventTapLocation::Session, CGEventTapPlacement::HeadInsertEventTap, CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp, CGEventType::LeftMouseDown, CGEventType::RightMouseDown, CGEventType::OtherMouseDown], |_p, ty, e| on_event(ty, e));
    let tap = match tap { Ok(t) => t, Err(_) => {
        eprintln!("CGEventTapCreate failed: grant Accessibility (and Input Monitoring) to this binary/terminal, then relaunch.");
        let _ = tx.send(UiMsg::Stopped); return
    } };
    let port = tap.mach_port().as_concrete_TypeRef() as *const c_void;
    let src = tap.mach_port().create_runloop_source(0).expect("runloop source");
    let rl = CFRunLoop::get_current();
    rl.add_source(&src, unsafe { kCFRunLoopCommonModes }); tap.enable();
    ST.with(|s| *s.borrow_mut() = Some(State { app, worker: rewrite::Worker::spawn(|| MacHost), last_pid: -1, tap_port: port, move_due: None, move_left: 0, last_move: None, last_src: "", gate: field::Gate::default(), ax: ax_worker(), field_rx: None }));
    let timer = CFRunLoopTimer::new(core_foundation::date::CFDate::now().abs_time() + 0.2, 0.025, 0, 0, timer_cb, std::ptr::null_mut());
    rl.add_timer(&timer, unsafe { kCFRunLoopCommonModes });
    CFRunLoop::run_current();
    drop(tap);
}

pub fn run(app: App) {
    unsafe {
        let opts = CFDictionary::from_CFType_pairs(&[(CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt), CFBoolean::true_value().as_CFType())]);
        if !AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) {
            eprintln!("typeThai needs Accessibility permission: System Settings > Privacy & Security > Accessibility. Grant it and relaunch.");
        }
    }
    sync_input_source();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("typethai-capture".into()).spawn(move || capture_thread(app, tx)).expect("capture thread");
    crate::ui::run(rx); // gpui owns the main thread / NSApplication run loop
}
