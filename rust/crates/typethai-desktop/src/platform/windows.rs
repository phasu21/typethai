//! Windows backend: WH_KEYBOARD_LL / WH_MOUSE_LL hooks, SendInput (unicode) injection, a non-activating topmost bubble,
//! caret lookup (GetGUIThreadInfo -> UI Automation TextPattern2 -> focused element rect -> mouse), secure-field skip
//! (ES_PASSWORD / UIA IsPassword), switch to Thai/US layout after applying, tray icon.
use crate::{app::{hotkey_for, App}, backspace::DeleteUnit, bridge::{self, UiMsg}, engine::{Action, Arrow, Boundary, Hotkey, InputLang, Key}, inputsrc::Source, keys::win_vk_to_us, placement::{Anchor, Rect}, rewrite};
use std::cell::{Cell, RefCell};
use std::sync::{atomic::{AtomicU32, Ordering}, mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use super::field::{self, Field};
use super::follow;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Marks our own injected events (dwExtraInfo) so the hook ignores them.
const MAGIC: usize = 0x5454_4841;
const ES_PASSWORD_STYLE: i32 = 0x0020;
const THAI_KLID: usize = 0x041E;

/// `layouts`: the foreground window's input language as last seen, for the badge.
struct State { app: App, worker: rewrite::Worker, tx: mpsc::Sender<UiMsg>, last_fg: HWND, last_focus: HWND, secure: bool, uia: Option<mpsc::Sender<(UiaQuery, mpsc::Sender<UiaResp>)>>, uia_caret: Option<mpsc::Sender<(UiaQuery, mpsc::Sender<UiaResp>)>>,
    /// Where the last click outside our own windows landed: the caret went there, so it is the pointer the anchor
    /// falls back to when the app exposes no caret.
    last_click: Option<(f64, f64)>,
    gate: field::Gate, field_rx: Option<mpsc::Receiver<UiaResp>>, text_app: bool, layouts: crate::badge::Watch,
    /// Caret re-reads after a key (`arm_caret_follow`), for the bubble or the preview that is up.
    follow: follow::Follow,
    /// When the next look at focus is due, and whether a keystroke armed it (a click, a window switch or a key may have
    /// moved focus, and the handler that saw the trigger cannot wait for UI Automation).
    badge_due: Option<(Instant, bool)> }
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; static BUSY: Cell<bool> = const { Cell::new(false) }; }
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> { ST.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f))) }

// ---------- UI Automation worker (MTA thread, so UIA calls never run on the hook thread) ----------
#[derive(Clone, Copy)]
enum UiaQuery { Focus, Caret }
enum UiaResp { Focus(bool, Field), Caret { caret: Option<RECT>, field: Option<RECT> } }
/// A UI Automation worker thread.
///
/// The focus read is the slow one: the cached element carries five properties, each a round trip to the app. The
/// caret read is the one with a deadline, so it gets a worker of its own — a focus read in flight (the periodic
/// refresh) must not make the caret read miss its cap and leave the bubble without a caret to sit on.
fn uia_worker() -> mpsc::Sender<(UiaQuery, mpsc::Sender<UiaResp>)> {
    let (tx, rx) = mpsc::channel::<(UiaQuery, mpsc::Sender<UiaResp>)>();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
        let auto: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
        for (q, reply) in rx {
            let el = || auto.as_ref().and_then(|a| a.GetFocusedElement().ok());
            let r = match q {
                UiaQuery::Focus => { let (pw, f) = uia_focus(auto.as_ref()); UiaResp::Focus(pw, f) }
                UiaQuery::Caret => {
                    // The focused element and its caret in one round trip: a second query would queue behind this one
                    // on the worker thread, and the caret read is the one with a deadline.
                    let e = el();
                    let field = e.as_ref().and_then(|e| e.CurrentBoundingRectangle().ok());
                    let caret = e.as_ref().and_then(|e| uia_caret(e));
                    UiaResp::Caret { caret, field }
                }
            };
            let _ = reply.send(r);
        }
    });
    tx
}
const FOCUS_PROPS: [UIA_PROPERTY_ID; 5] = [UIA_IsPasswordPropertyId, UIA_ControlTypePropertyId, UIA_IsTextPatternAvailablePropertyId, UIA_IsValuePatternAvailablePropertyId, UIA_ValueIsReadOnlyPropertyId];
/// Focused element: (password, text-field verdict), in one cross-process round trip (cache request).
unsafe fn uia_focus(a: Option<&IUIAutomation>) -> (bool, Field) {
    // The cache request is made per read: made once at startup it can fail (the proxy is not up yet), and then
    // every read would fail with it and the focus would stay unknown forever.
    let Some(a) = a else { return (false, Field::Unknown) };
    let req = match a.CreateCacheRequest() { Ok(r) => r, Err(e) => { debug_uia(&format!("cache request: {e:?}")); return (false, Field::Unknown) } };
    for p in FOCUS_PROPS { let _ = req.AddProperty(p); }
    let e = match a.GetFocusedElementBuildCache(&req) { Ok(e) => e, Err(e) => { debug_uia(&format!("focused element: {e:?}")); return (false, Field::Unknown) } };
    // a property the element lacks comes back as the "not supported" sentinel, which is no bool
    let b = |p| e.GetCachedPropertyValue(p).ok().and_then(|v| bool::try_from(&v).ok()).unwrap_or(false);
    let ct = e.CachedControlType().map(|c| c.0).unwrap_or(0);
    (b(UIA_IsPasswordPropertyId), field::uia(ct, b(UIA_IsTextPatternAvailablePropertyId), b(UIA_IsValuePatternAvailablePropertyId), b(UIA_ValueIsReadOnlyPropertyId)))
}
/// First rectangle of a text range, on screen (physical pixels). A degenerate (caret) range comes back as the
/// insertion point itself where the provider has one for it (Chromium builds a 1-px rect for UIA, Word answers the
/// caret line).
unsafe fn range_rect(r: &IUIAutomationTextRange) -> Option<RECT> {
    let sa = r.GetBoundingRectangles().ok()?; if sa.is_null() { return None }
    let ub = SafeArrayGetUBound(sa, 1).ok()?; let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
    let out = if ub >= 3 && SafeArrayAccessData(sa, &mut p).is_ok() {
        let d = std::slice::from_raw_parts(p as *const f64, 4); let _ = SafeArrayUnaccessData(sa);
        Some(RECT { left: d[0] as i32, top: d[1] as i32, right: (d[0] + d[2]) as i32, bottom: (d[1] + d[3]) as i32 })
    } else { None };
    let _ = SafeArrayDestroy(sa); out
}
/// The caret of a text range: the range's own rect when the provider gives one for it, else the character the caret
/// sits on — its left edge, or its right edge when the caret is at the end of the text (the enclosing character is
/// then the one before it).
unsafe fn caret_of(range: &IUIAutomationTextRange) -> Option<RECT> {
    if let Some(r) = range_rect(range) { return Some(r) }
    let c = range.Clone().ok()?;
    c.ExpandToEnclosingUnit(TextUnit_Character).ok()?;
    let r = range_rect(&c)?;
    let at_end = range.CompareEndpoints(TextPatternRangeEndpoint_Start, &c, TextPatternRangeEndpoint_Start).ok()? > 0;
    let x = if at_end { r.right } else { r.left };
    Some(RECT { left: x, top: r.top, right: x + 1, bottom: r.bottom })
}
/// Caret rect of the focused element. TextPattern2 answers the caret itself (Word, WinUI); Chromium and Electron
/// implement TextPattern only, where the caret is the collapsed selection — the degenerate range whose rect Chromium
/// builds as the insertion point, the same way `marker_caret` reads their caret on macOS.
unsafe fn uia_caret(e: &IUIAutomationElement) -> Option<RECT> {
    if let Ok(tp) = e.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id) {
        let mut active = windows::core::BOOL(0);
        // `active` is false when the element has no caret of its own: the range is then somebody else's.
        if let Ok(caret) = tp.GetCaretRange(&mut active) { if active.as_bool() { if let Some(r) = caret_of(&caret) { return Some(r) } } }
    }
    let tp: IUIAutomationTextPattern = e.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
    let sel = tp.GetSelection().ok()?;
    let range = (0..sel.Length().ok()?).find_map(|i| sel.GetElement(i).ok())?;
    // The selection is not the caret: with text selected its rect is the whole selection, and anchoring the bubble to
    // that puts it at the selection's edge, not where the next character goes. The caret is where the selection
    // ends, the same end macOS reads for an AX selection.
    let caret = range.Clone().ok()?;
    let _ = caret.MoveEndpointByRange(TextPatternRangeEndpoint_Start, &range, TextPatternRangeEndpoint_End);
    caret_of(&caret)
}
/// `TYPETHAI_DEBUG`: a UI Automation failure worth knowing about.
fn debug_uia(what: &str) { if std::env::var_os("TYPETHAI_DEBUG").is_some() { eprintln!("typethai: uia {what}"); } }
fn uia_ask(st: &State, q: UiaQuery) -> Option<UiaResp> {
    // each query kind has its own worker: the slow focus read must not hold up the caret read
    let worker = match q { UiaQuery::Focus => st.uia.as_ref()?, UiaQuery::Caret => st.uia_caret.as_ref()? };
    let (tx, rx) = mpsc::channel();
    let t0 = Instant::now();
    if worker.send((q, tx)).is_err() { return None }
    let r = rx.recv_timeout(Duration::from_millis(60)).ok();
    // a read that missed its cap is a caret that is not there: say so, it is the one that misplaces the bubble
    if std::env::var_os("TYPETHAI_DEBUG").is_some() {
        let ms = t0.elapsed().as_millis();
        let kind = match q { UiaQuery::Focus => "focus", UiaQuery::Caret => "caret" };
        if r.is_none() { eprintln!("typethai: uia {kind} read dropped after {ms} ms"); }
        else if ms > 20 { eprintln!("typethai: uia {kind} read took {ms} ms"); }
    }
    r
}

// ---------- focus / secure / caret ----------
unsafe fn gui_info(fg: HWND) -> Option<GUITHREADINFO> {
    let tid = GetWindowThreadProcessId(fg, None);
    let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    GetGUIThreadInfo(tid, &mut gi).ok()?; Some(gi)
}
unsafe fn password_edit(focus: HWND) -> bool {
    if focus.is_invalid() { return false }
    let mut cls = [0u16; 64]; let n = GetClassNameW(focus, &mut cls) as usize;
    String::from_utf16_lossy(&cls[..n]).to_ascii_lowercase().contains("edit") && GetWindowLongW(focus, GWL_STYLE) & ES_PASSWORD_STYLE != 0
}
/// (password field, text-field verdict) now, 60 ms cap; no reply is "not a password, unknown".
unsafe fn focus_check(st: &State, focus: HWND) -> (bool, Field) {
    let (pw, f) = match uia_ask(st, UiaQuery::Focus) { Some(UiaResp::Focus(pw, f)) => (pw, f), _ => (false, Field::Unknown) };
    let pw = pw || password_edit(focus);
    // `TYPETHAI_DEBUG`: why the gate opened or closed in this app (the bubble shows only where it opens)
    if std::env::var_os("TYPETHAI_DEBUG").is_some() { eprintln!("typethai: focus hwnd={focus:?} password={pw} field={f:?}") }
    (pw, f)
}
/// Closed gate: re-check off-thread; the next key picks the reply up (`field_collect`).
fn field_refresh(st: &mut State) {
    let (tx, rx) = mpsc::channel();
    if st.uia.as_ref().is_some_and(|u| u.send((UiaQuery::Focus, tx)).is_ok()) { st.field_rx = Some(rx) } else { st.gate.answer(None, Instant::now()) }
}
/// Takes the off-thread reply if it is in; never waits (see `Gate::must_ask`).
fn field_collect(st: &mut State) {
    let Some(rx) = st.field_rx.take() else { return };
    match rx.try_recv() {
        Ok(UiaResp::Focus(_, f)) => st.gate.answer(Some(f), Instant::now()),
        Err(mpsc::TryRecvError::Empty) => st.field_rx = Some(rx),
        _ => st.gate.answer(None, Instant::now()),
    }
}
unsafe fn class_of(h: HWND) -> String { let mut c = [0u16; 64]; let n = GetClassNameW(h, &mut c) as usize; String::from_utf16_lossy(&c[..n]) }
/// Foreground window covers its whole monitor without being maximized (borderless / exclusive fullscreen: games).
unsafe fn fullscreen(fg: HWND) -> bool {
    let (mut r, mut mi) = (RECT::default(), MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() });
    if IsZoomed(fg).as_bool() || GetWindowRect(fg, &mut r).is_err() || !GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() { return false }
    let m = mi.rcMonitor; r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
}
fn rect(r: RECT) -> Rect { Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64) }
/// `TYPETHAI_DEBUG`: the anchor last logged: which of caret / field / mouse it came from, and the rects it used.
static LAST_ANCHOR: Mutex<Option<(&'static str, Option<Rect>, Option<Rect>, Option<(f64, f64)>)>> = Mutex::new(None);
/// Caret rect (GetGUIThreadInfo, else UIA TextPattern2 / TextPattern), focused element frame (UIA, else the focus
/// HWND) and the mouse, in physical screen pixels (gpui makes the process per-monitor DPI aware).
unsafe fn anchor(st: &State) -> Anchor {
    let fg = GetForegroundWindow(); let gi = gui_info(fg);
    let mut a = Anchor::default();
    if let Some(gi) = gi {
        // The Win32 caret, for the apps that make one (a text field, a console). Only the position is the caret's:
        // the rect's right and bottom carry the text direction and padding, not the caret's edges, and a window with
        // no caret reports a 1x1 placeholder at the client's origin - anchoring to that is the corner of the screen.
        if !gi.hwndCaret.is_invalid() {
            if let Some(c) = crate::placement::win32_caret(gi.rcCaret.left, gi.rcCaret.top, gi.rcCaret.bottom) {
                let mut p = POINT { x: gi.rcCaret.left, y: gi.rcCaret.top };
                let _ = ClientToScreen(gi.hwndCaret, &mut p);
                a.caret = Some(Rect::new(p.x as f64, p.y as f64, c.w, c.h));
            }
        }
    }
    // UI Automation, for the apps with no Win32 caret (Chromium, Electron). The caret is only trusted when it lands
    // in the focused element: a provider that answers with an element-relative or off-screen rect would put the
    // bubble nowhere near the text.
    if a.caret.is_none() {
        if let Some(UiaResp::Caret { caret, field }) = uia_ask(st, UiaQuery::Caret) {
            let field = field.map(rect);
            if let Some(c) = caret.map(rect) { if crate::placement::plausible_caret(c, field, &crate::ui::native::monitors()) { a.caret = Some(c); } }
            a.field = field;
        }
    }
    // the focused window's frame, and the click that put the focus there: the caret went where the user clicked, so
    // that is the pointer to fall back on when the app exposes no caret (the pointer itself moves on since)
    let focus_win = gi.and_then(|g| { let mut r = RECT::default(); if !g.hwndFocus.is_invalid() && GetWindowRect(g.hwndFocus, &mut r).is_ok() { Some(rect(r)) } else { None } });
    if a.field.is_none() { a.field = focus_win; }
    let mut p = POINT::default(); if GetCursorPos(&mut p).is_ok() { a.mouse = Some((p.x as f64, p.y as f64)); }
    a.mouse = crate::placement::pointer_at(st.last_click, focus_win, a.mouse);
    // a bubble that reads as misplaced is otherwise a guess: say which source placed it, and where the caret was
    if std::env::var_os("TYPETHAI_DEBUG").is_some() {
        let src = crate::placement::source_of(&a, &crate::placement::Opts::default()).map(|s| s.name()).unwrap_or("none");
        let mut last = LAST_ANCHOR.lock().unwrap();
        if *last != Some((src, a.caret, a.field, a.mouse)) { *last = Some((src, a.caret, a.field, a.mouse)); eprintln!("typethai: anchor source={src} caret={:?} field={:?} mouse={:?}", a.caret, a.field, a.mouse) }
    }
    a
}
unsafe fn layout_is_thai(fg: HWND) -> bool { (GetKeyboardLayout(GetWindowThreadProcessId(fg, None)).0 as usize & 0xFFFF) == THAI_KLID }

// ---------- injection ----------
fn kbd(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } }
}
fn tap(v: &mut Vec<INPUT>, vk: VIRTUAL_KEY) { v.push(kbd(vk, 0, KEYBD_EVENT_FLAGS(0))); v.push(kbd(vk, 0, KEYEVENTF_KEYUP)); }
/// Release held modifiers (hotkeys are Ctrl+Alt+...), otherwise our backspaces become Ctrl+Alt+Backspace.
unsafe fn release_held(v: &mut Vec<INPUT>) {
    for m in [VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LSHIFT, VK_RSHIFT] { if GetAsyncKeyState(m.0 as i32) < 0 { v.push(kbd(m, 0, KEYEVENTF_KEYUP)); } }
}
/// Ctrl+`vk` with no other modifier.
unsafe fn ctrl_chord(vk: VIRTUAL_KEY) {
    let mut v = Vec::new(); release_held(&mut v);
    v.push(kbd(VK_CONTROL, 0, KEYBD_EVENT_FLAGS(0))); tap(&mut v, vk); v.push(kbd(VK_CONTROL, 0, KEYEVENTF_KEYUP));
    SendInput(&v, std::mem::size_of::<INPUT>() as i32);
}
unsafe fn inject(delete: usize, insert: &str, then: Option<Boundary>) {
    let mut v = Vec::new(); release_held(&mut v);
    for _ in 0..delete { tap(&mut v, VK_BACK); }
    for u in insert.encode_utf16() {
        if u == b'\n' as u16 { tap(&mut v, VK_RETURN); continue } if u == b'\t' as u16 { tap(&mut v, VK_TAB); continue }
        v.push(kbd(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE)); v.push(kbd(VIRTUAL_KEY(0), u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP));
    }
    match then { Some(Boundary::Space) => tap(&mut v, VK_SPACE), Some(Boundary::Enter) => tap(&mut v, VK_RETURN), Some(Boundary::Tab) => tap(&mut v, VK_TAB), None => {} }
    SendInput(&v, std::mem::size_of::<INPUT>() as i32);
}
/// Thai Kedmanee (0x041E) or US (0x0409): ask the foreground window to change its input language (only if it differs).
unsafe fn switch_input(fg: HWND, lang: InputLang) {
    if layout_is_thai(fg) == (lang == InputLang::Thai) { return }
    let id = if lang == InputLang::Thai { w!("0000041E") } else { w!("00000409") };
    if let Ok(hkl) = LoadKeyboardLayoutW(id, ACTIVATE_KEYBOARD_LAYOUT_FLAGS(0)) {
        let _ = PostMessageW(Some(fg), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(hkl.0 as isize));
    }
}
/// Loaded keyboard layouts (GetKeyboardLayoutList), reported as KLID-style hex ids.
pub fn input_sources() -> Vec<Source> {
    unsafe {
        let n = GetKeyboardLayoutList(None); let mut v = vec![HKL::default(); n.max(0) as usize]; let n = GetKeyboardLayoutList(Some(&mut v));
        v.truncate(n.max(0) as usize);
        v.into_iter().map(|h| { let lang = h.0 as usize & 0xFFFF;
            let id = format!("{:08X}", h.0 as usize & 0xFFFF_FFFF);
            Source { id: if lang == THAI_KLID { format!("{id} (Thai)") } else { id }, name: format!("langid {lang:#06x}"), ascii: lang != THAI_KLID, selectable: true } }).collect()
    }
}
unsafe fn run_actions(st: &mut State, acts: Vec<Action>) {
    for a in acts { match a {
        Action::Replace { delete, insert, then } => inject(delete, &insert, then),
        Action::SwitchInput(l) => switch_input(GetForegroundWindow(), l),
        Action::Select(i) => { let _ = st.tx.send(UiMsg::Select(i)); }
        Action::Focus(f) => { let _ = st.tx.send(UiMsg::Focus(f)); }
        Action::ShowBubble(items) => { let an = anchor(st); let _ = st.tx.send(UiMsg::show(items, st.app.engine.opts.bubble, an)); arm_caret_follow(st); }
        Action::HideBubble => { bridge::set_geom(None); let _ = st.tx.send(UiMsg::Hide); }
        Action::Rewrite(job) => st.worker.submit(job),
        Action::Badge(on) => if on { let _ = offer_badge(st, GetForegroundWindow(), true); } else { let _ = st.tx.send(UiMsg::UnpinBadge); },
        Action::Hold(hold) => { let an = hold.filter(|_| !st.app.engine.bubble_visible()).map(|_| anchor(st)); let _ = st.tx.send(UiMsg::Hold { hold, anchor: an }); }
    } }
}

// ---------- rewrite: selection + clipboard, on the desk thread ----------
/// Built on the desk thread with its own COM MTA and IUIAutomation, so the caret `uia_worker` stays free.
struct WinHost { auto: Option<IUIAutomation> }
impl WinHost {
    fn new() -> WinHost { unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok(); WinHost { auto: CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() } } }
    fn focused(&self) -> Option<IUIAutomationElement> { unsafe { self.auto.as_ref()?.GetFocusedElement().ok() } }
}
/// GDI-handle formats are not HGLOBAL memory, so they cannot be saved as bytes (CF_BITMAP comes back from CF_DIB).
const GDI_FORMATS: [CLIPBOARD_FORMAT; 8] = [CF_BITMAP, CF_METAFILEPICT, CF_PALETTE, CF_ENHMETAFILE, CF_OWNERDISPLAY, CF_DSPBITMAP, CF_DSPMETAFILEPICT, CF_DSPENHMETAFILE];
/// Run `f` with the clipboard open, retrying while another app holds it.
unsafe fn with_clipboard<R>(f: impl FnOnce() -> R) -> Option<R> {
    for _ in 0..20 { if OpenClipboard(None).is_ok() { let r = f(); let _ = CloseClipboard(); return Some(r) } std::thread::sleep(Duration::from_millis(10)); }
    None
}
unsafe fn read_global(fmt: u32) -> Option<Vec<u8>> {
    let h = HGLOBAL(GetClipboardData(fmt).ok()?.0); let p = GlobalLock(h) as *const u8; if p.is_null() { return None }
    let v = std::slice::from_raw_parts(p, GlobalSize(h)).to_vec(); let _ = GlobalUnlock(h); Some(v)
}
/// With the clipboard open and emptied by us. The clipboard owns the memory once SetClipboardData succeeds.
unsafe fn set_global(fmt: u32, bytes: &[u8]) {
    let Ok(h) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else { return };
    let p = GlobalLock(h) as *mut u8; if p.is_null() { let _ = GlobalFree(Some(h)); return }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()); let _ = GlobalUnlock(h);
    if SetClipboardData(fmt, Some(HANDLE(h.0))).is_err() { let _ = GlobalFree(Some(h)); }
}
impl rewrite::Host for WinHost {
    type Snap = Vec<(u32, Vec<u8>)>;
    fn is_secure(&mut self) -> bool {
        unsafe { password_edit(gui_info(GetForegroundWindow()).map(|g| g.hwndFocus).unwrap_or_default()) || self.focused().and_then(|e| e.CurrentIsPassword().ok()).is_some_and(|b| b.as_bool()) }
    }
    fn selection_via_accessibility(&mut self) -> Option<String> {
        unsafe {
            let tp: IUIAutomationTextPattern = self.focused()?.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
            if tp.SupportedTextSelection().ok()? == SupportedTextSelection_None { return None }
            let sel = tp.GetSelection().ok()?;
            (0..sel.Length().ok()?).map(|i| sel.GetElement(i).and_then(|r| r.GetText(-1)).map(|b| b.to_string())).collect::<windows::core::Result<String>>().ok()
        }
    }
    fn clip_seq(&mut self) -> u64 { unsafe { GetClipboardSequenceNumber() as u64 } }
    fn clip_snapshot(&mut self) -> Self::Snap {
        unsafe { with_clipboard(|| {
            let mut v = vec![]; let mut f = EnumClipboardFormats(0);
            while f != 0 { if !GDI_FORMATS.iter().any(|g| g.0 as u32 == f) { if let Some(b) = read_global(f) { v.push((f, b)) } } f = EnumClipboardFormats(f); }
            v
        }).unwrap_or_default() }
    }
    fn clip_restore(&mut self, s: Self::Snap) { unsafe { with_clipboard(|| { let _ = EmptyClipboard(); for (f, b) in &s { set_global(*f, b) } }); } }
    fn clip_read_text(&mut self) -> Option<String> {
        let b = unsafe { with_clipboard(|| read_global(CF_UNICODETEXT.0 as u32)) }.flatten()?;
        let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&u))
    }
    fn clip_write_transient(&mut self, text: &str) {
        unsafe { with_clipboard(|| {
            let _ = EmptyClipboard();
            set_global(CF_UNICODETEXT.0 as u32, &text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect::<Vec<u8>>());
            // clipboard monitors, Win+V history and the cloud clipboard skip content carrying these formats
            set_global(RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")), &[0]);
            for name in [w!("CanIncludeInClipboardHistory"), w!("CanUploadToCloudClipboard")] { set_global(RegisterClipboardFormatW(name), &0u32.to_le_bytes()) }
        }); }
    }
    fn post_copy(&mut self) { unsafe { ctrl_chord(VK_C) } }
    fn post_paste(&mut self) { unsafe { ctrl_chord(VK_V) } }
    fn pause(&mut self, ms: u64) { std::thread::sleep(Duration::from_millis(ms)) }
}

// ---------- hooks ----------
fn down(vk: VIRTUAL_KEY) -> bool { unsafe { GetAsyncKeyState(vk.0 as i32) < 0 } }
unsafe fn translate(vk: u32) -> Option<Key> {
    let (ctrl, alt, win) = (down(VK_CONTROL), down(VK_MENU), down(VK_LWIN) || down(VK_RWIN));
    let shift = down(VK_SHIFT); let caps = GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0;
    let v = VIRTUAL_KEY(vk as u16);
    if ctrl && alt { if let Some(h) = win_vk_to_us(vk, false, false).and_then(hotkey_for) { return Some(Key::Hotkey(h)) } }
    if [VK_SHIFT, VK_LSHIFT, VK_RSHIFT, VK_CONTROL, VK_LCONTROL, VK_RCONTROL, VK_MENU, VK_LMENU, VK_RMENU, VK_CAPITAL, VK_LWIN, VK_RWIN].contains(&v) { return None }
    if v == VK_BACK && ctrl && !alt { return Some(Key::WordDelete) }
    if ctrl && !alt && !win && !shift { if let Some(d @ '1'..='3') = win_vk_to_us(vk, false, false) { return Some(Key::PickRow(d as usize - '1' as usize)) } }
    if ctrl || alt || win { return Some(Key::Nav) }
    Some(match v {
        VK_BACK => Key::Backspace, VK_SPACE => Key::Boundary(Boundary::Space), VK_RETURN => Key::Boundary(Boundary::Enter),
        VK_TAB => if shift { Key::Nav } else { Key::Tab }, // Shift+Tab (reverse focus) stays plain navigation
        VK_ESCAPE => Key::Esc,
        VK_UP if shift => Key::SelectPrev, VK_DOWN if shift => Key::SelectNext,
        VK_UP => Key::Arrow(Arrow::Up), VK_DOWN => Key::Arrow(Arrow::Down),
        VK_LEFT if !shift => Key::Arrow(Arrow::Left), VK_RIGHT if !shift => Key::Arrow(Arrow::Right),
        _ => match win_vk_to_us(vk, shift, caps) { Some(c) => Key::Char(c), None => Key::Nav },
    })
}
unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && !BUSY.get() {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let m = wp.0 as u32;
        let (is_down, is_up) = (m == WM_KEYDOWN || m == WM_SYSKEYDOWN, m == WM_KEYUP || m == WM_SYSKEYUP);
        if (is_down || is_up) && k.dwExtraInfo != MAGIC {
            BUSY.set(true);
            let swallow = with(|st| handle_key(st, k.vkCode, is_up)).unwrap_or(false);
            BUSY.set(false);
            if swallow { return LRESULT(1) }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}
/// File name of the executable that owns `hwnd` ("WindowsTerminal.exe"); empty when it cannot be read.
unsafe fn exe_of(hwnd: HWND) -> String {
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
    let (mut buf, mut len) = ([0u16; 1024], 1024u32);
    let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
    let _ = CloseHandle(h);
    if !ok { return String::new() }
    String::from_utf16_lossy(&buf[..len as usize]).rsplit('\\').next().unwrap_or_default().to_string()
}
fn foreground_is_self(fg: HWND) -> bool {
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}
/// Keys while our Settings window is in front are left alone, so the field can be typed in.
unsafe fn typing_in_our_window(st: &mut State, fg: HWND) -> bool {
    if !foreground_is_self(fg) { return false }
    if fg != st.last_fg {
        st.last_fg = fg;
        st.last_focus = gui_info(fg).map(|g| g.hwndFocus).unwrap_or_default();
        // the Settings window has focus: no caret of ours to sit over, and the preview belongs to the app behind it
        let _ = st.tx.send(UiMsg::HideBadge);
        let a = st.app.engine.reset();
        run_actions(st, a);
    }
    true
}
unsafe fn service_settings(st: &mut State) {
    let cmds = bridge::take_settings();
    if cmds.is_empty() && !bridge::take(&bridge::REQ_SETTINGS) { return }
    let (snap, hide) = crate::prefs::apply_commands(&mut st.app, cmds);
    run_actions(st, hide);
    let _ = st.tx.send(UiMsg::Settings(snap));
}
unsafe fn handle_key(st: &mut State, vk: u32, up: bool) -> bool {
    // the tap owns the shift state (Shift left AltGr alone: the right Alt arrives as Ctrl+Alt there)
    if vk == VK_SHIFT.0 as u32 || vk == VK_LSHIFT.0 as u32 || vk == VK_RSHIFT.0 as u32 {
        bridge::SHIFT_HELD.store(!up, Ordering::SeqCst);
    }
    take_click(st);
    if std::env::var_os("TYPETHAI_DEBUG").is_some() { eprintln!("vk {vk:#x} up={up}"); }
    // every keystroke, whatever becomes of it: typing takes the input-language preview down
    if !up { let a = st.app.engine.on_keystroke(Instant::now()); run_actions(st, a); }
    let fg = GetForegroundWindow();
    if typing_in_our_window(st, fg) { return false }
    let key = if up {
        // the only release that matters: Tab, while it is held over the bubble
        if vk != VK_TAB.0 as u32 || !st.app.engine.is_bubble_key(&Key::TabUp) { return false }
        Key::TabUp
    } else { let Some(key) = translate(vk) else { return false }; key };
    let gi = gui_info(fg); let focus = gi.map(|g| g.hwndFocus).unwrap_or_default();
    if fg != st.last_fg { let exe = exe_of(fg); st.app.engine.delete_unit = DeleteUnit::of_app(&exe); st.text_app = field::known_text_exe(&exe) || field::known_text_class(&class_of(fg));
        // the tray's app item follows the window in front, except the tray/taskbar itself (clicking its icon must not
        // make the taskbar the app the menu talks about) and our own windows
        let class = class_of(fg);
        if !exe.is_empty() && !foreground_is_self(fg) && !matches!(class.as_str(), "Shell_TrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland") {
            let name = match exe.len().checked_sub(4) { Some(i) if exe.is_char_boundary(i) && exe[i..].eq_ignore_ascii_case(".exe") => &exe[..i], _ => &exe };
            let a = st.app.set_front_app(&exe, name); run_actions(st, a);
        } }
    if fg != st.last_fg || focus != st.last_focus { st.last_fg = fg; st.last_focus = focus; st.secure = false; st.gate.reset(); st.field_rx = None; arm_focus_look(st, false);
        // the new window's caret is somewhere else: a preview that is up follows it there
        if st.app.engine.badge_shown() { let an = anchor(st); let _ = st.tx.send(UiMsg::MoveBadge(an)); } let a = st.app.engine.reset(); run_actions(st, a); }
    // typeThai is off in this app: every key reaches it untouched, before any focus/AX work
    if st.app.engine.app_off() { return false }
    // these may move focus inside the window: a closed gate re-checks, and so does the preview's policy (a look armed by
    // a key never pins the preview: the caret under it is not laid out yet, and Enter/Ctrl+C are typing)
    if matches!(key, Key::Tab | Key::Esc | Key::Nav | Key::PickRow(_) | Key::Boundary(Boundary::Enter)) { st.gate.touch(Instant::now()); arm_focus_look(st, true); }
    // Thai layout / password field / not a text field: bypass the engine, except hotkeys and the bubble controls while the bubble is up
    st.app.engine.set_thai_active(layout_is_thai(fg)); // Thai layout -> reverse detector (English typed on Kedmanee)
    if !matches!(key, Key::Hotkey(_)) && !st.app.engine.is_bubble_key(&key) {
        if layout_is_thai(fg) && st.app.engine.rev.is_none() { let a = st.app.engine.reset(); run_actions(st, a); return false }
        // text fields only (games: WASD): a caret (or a known text app) opens, else the cached UIA verdict. One query
        // (off-thread, 60 ms cap) at each word start answers it and the password check; while closed, only after a
        // click / focus key.
        let word_start = (matches!(key, Key::Char(_)) && st.app.engine.buffer().is_empty()) || matches!(key, Key::Boundary(_) | Key::Tab);
        let caret = st.text_app || gi.is_some_and(|g| !g.hwndCaret.is_invalid());
        field_collect(st); let now = Instant::now();
        if st.gate.must_ask(word_start, st.gate.open(caret, || fullscreen(fg))) { let (pw, f) = focus_check(st, focus); st.secure = pw; st.gate.answer(Some(f), now); }
        else if st.gate.refresh_due(now, true) { field_refresh(st); }
        if st.secure || !st.gate.open(caret, || fullscreen(fg)) { let a = st.app.engine.reset(); run_actions(st, a); return false }
    }
    let was_paused = st.app.engine.paused;
    let dbg = std::env::var_os("TYPETHAI_DEBUG").is_some();
    if dbg { eprintln!("key {key:?} buf={:?}", st.app.engine.buffer()); }
    let o = st.app.engine.on_key(key);
    if dbg && !o.actions.is_empty() { eprintln!("  -> swallow={} {:?}", o.swallow, o.actions); }
    if was_paused != st.app.engine.paused { let _ = st.tx.send(UiMsg::Paused(st.app.engine.paused)); }
    run_actions(st, o.actions); st.app.maybe_save();
    // the caret moves with every keystroke: follow it while the bubble is up (this hook runs before the app handles
    // the key, so the caret is read again a moment later)
    if st.app.engine.bubble_visible() { arm_caret_follow(st) }
    o.swallow
}
// ---------- mouse thread ----------
/// The last click the mouse thread saw, for the hook thread: elsewhere (the caret may have moved), or a left click on
/// an option or one of its thumbs. Taken before the next key and on the timer, so a key typed after a click sees it first.
enum Click { Elsewhere(f64, f64), On(bridge::Hit) }
static CLICK: Mutex<Option<Click>> = Mutex::new(None);
/// The hook thread, woken (WM_APP) to apply a clicked row right away.
static HOOK_TID: AtomicU32 = AtomicU32::new(0);
static MOUSE_TX: OnceLock<mpsc::Sender<UiMsg>> = OnceLock::new();
/// Runs on its own thread: Windows holds every mouse move until a low-level hook returns, so this one never waits on
/// the engine or a UI Automation query (a game, clicking and pressing keys, stuttered when it shared their thread).
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let m = wp.0 as u32;
    if code == HC_ACTION as i32 && (m == WM_LBUTTONDOWN || m == WM_RBUTTONDOWN || m == WM_MBUTTONDOWN) {
        let pt = (*(lp.0 as *const MSLLHOOKSTRUCT)).pt;
        // on the badge: swallow (it is not click-through here) and take it down, so the next click reaches the app
        if bridge::BADGE_AT.lock().unwrap().is_some_and(|r| r.contains(pt.x as f64, pt.y as f64)) {
            *bridge::BADGE_AT.lock().unwrap() = None;
            if let Some(tx) = MOUSE_TX.get() { let _ = tx.send(UiMsg::HideBadge); }
            return LRESULT(1)
        }
        // on the chip's hover tip: swallow (it is only a label on Windows; macOS's copy ignores the mouse itself)
        if bridge::TIP_AT.lock().unwrap().is_some_and(|r| r.contains(pt.x as f64, pt.y as f64)) {
            return LRESULT(1)
        }
        match bridge::geom().and_then(|g| g.hit(pt.x as f64, pt.y as f64, bridge::SHIFT_HELD.load(Ordering::SeqCst))) {
            // on the bubble: swallow so it is never clicked/activated; a left click on an option applies it, on a thumb
            // rates it, on the download chip it is what the tray's update item does right now
            Some(hit) => {
                if m == WM_LBUTTONDOWN {
                    if matches!(hit, bridge::Hit::Update) { bridge::REQ_UPDATE_CLICK.store(true, Ordering::SeqCst); }
                    else if hit.key().is_some() {
                        *CLICK.lock().unwrap() = Some(Click::On(hit));
                        let _ = PostThreadMessageW(HOOK_TID.load(Ordering::SeqCst), WM_APP, WPARAM(0), LPARAM(0));
                    }
                }
                return LRESULT(1)
            }
            None => *CLICK.lock().unwrap() = Some(Click::Elsewhere(pt.x as f64, pt.y as f64)),
        }
    }
    CallNextHookEx(None, code, wp, lp)
}
/// The click the mouse thread saw, on the hook thread.
unsafe fn take_click(st: &mut State) {
    // taken out first: the mouse thread must never wait on the engine
    let click = CLICK.lock().unwrap().take();
    match click {
        None => {}
        Some(Click::Elsewhere(x, y)) => {
            // the click that put the focus in the field is where the caret went: the anchor's pointer, when the app
            // exposes no caret of its own
            st.last_click = Some((x, y));
            st.gate.touch(Instant::now()); arm_focus_look(st, false);
            // the click moved the caret: a preview that is up follows it here, on the spot (a keystroke-driven follow
            // would read it before the app has laid it out, `arm_caret_follow`)
            if st.app.engine.badge_shown() { let an = anchor(st); let _ = st.tx.send(UiMsg::MoveBadge(an)); }
            let a = st.app.engine.reset(); run_actions(st, a);
        }
        Some(Click::On(hit)) => if let Some(k) = hit.key() { let o = st.app.engine.on_key(k); run_actions(st, o.actions); st.app.maybe_save(); },
    }
}
fn mouse_thread() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).expect("module").into();
        let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0) else { eprintln!("SetWindowsHookEx (mouse) failed"); return };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        let _ = UnhookWindowsHookEx(h);
    }
}

// ---------- hook thread ----------
/// Focus may have moved into or out of a text field (a click, a window switch, a focus key): `focus_look` asks once the
/// window has settled. The handler that saw the trigger cannot wait for it. `key`: a keystroke armed it, so this is not a
/// focus the user made (see `Engine::set_field`), and nothing is shown on the spot.
fn arm_focus_look(st: &mut State, key: bool) { st.badge_due = Some((Instant::now() + field::SETTLE, key)); }
/// Schedule the caret re-reads (`platform::follow`): the bubble or the preview that is up follows the caret the app
/// lays out after the key, which this hook sees before the app does.
fn arm_caret_follow(st: &mut State) { st.follow.arm(Instant::now()) }
/// One look at Focus, then the preview's policy: a focus the user made (a click, a window switch) shows the preview and
/// keeps it up while nothing has been typed in that field; elsewhere, in a password field, or in a game's window it
/// goes. Cheap signals only (a caret, a known text app, the gate's cached verdict): a full UIA look runs in
/// `offer_badge`, once the policy asks for one.
unsafe fn focus_look(st: &mut State, key: bool) {
    let fg = GetForegroundWindow(); let caret = st.text_app || gui_info(fg).is_some_and(|g| !g.hwndCaret.is_invalid());
    let text = !st.secure && st.gate.open(caret, || fullscreen(fg));
    let a = if key { st.app.engine.set_field(text) } else { st.app.engine.on_focus(text) }; run_actions(st, a);
}
/// The badge at the caret, unless it is off, typeThai is paused, the window is ours, or focus is a password field or no
/// text field (a fresh look, by the gate's rule: a caret or a known text app opens). `pin`: focus just landed in a text
/// field, so it stays until typing or focus leaving; else it goes after a moment.
unsafe fn offer_badge(st: &mut State, fg: HWND, pin: bool) -> bool {
    if st.app.engine.app_off() { return false }
    let lang = if layout_is_thai(fg) { InputLang::Thai } else { InputLang::Latin };
    let e = &st.app.engine; let on = e.opts.input_badge && !e.paused && !foreground_is_self(fg);
    // a password field or no text field (a fresh look, by the gate's rule: a caret or a known text app opens)
    let blocked = || {
        let gi = gui_info(fg); let (pw, f) = focus_check(st, gi.map(|g| g.hwndFocus).unwrap_or_default());
        let caret = (if fg == st.last_fg { st.text_app } else { field::known_text_exe(&exe_of(fg)) || field::known_text_class(&class_of(fg)) }) || gi.is_some_and(|g| !g.hwndCaret.is_invalid());
        pw || !field::Gate::told(f).open(caret, || fullscreen(fg))
    };
    match crate::badge::anchor_for(on, e.bubble_visible(), blocked, || anchor(st)) {
        // the preview follows the caret while it is up: read it again once the app has laid it out (arm the re-reads)
        Some(anchor) => { let _ = st.tx.send(UiMsg::Badge { lang, anchor, pin }); arm_caret_follow(st); true }
        None => false,
    }
}
/// Each window's thread has its own layout (Win+Space, Alt+Shift, the taskbar, or our switch after a conversion): a
/// change in the window in front brings up the badge, unless it is off, typeThai is paused, the bubble is up, the
/// window is ours, or focus is a password field or no text field. A different window with another layout is not a switch.
unsafe fn watch_layout(st: &mut State) {
    let fg = GetForegroundWindow(); if fg.is_invalid() { return }
    if st.layouts.see(fg.0 as usize as u64, layout_is_thai(fg)).is_some() { let _ = offer_badge(st, fg, false); }
}
unsafe fn control(st: &mut State) -> bool {
    take_click(st);
    service_settings(st);
    watch_layout(st);
    while let Some(r) = st.worker.poll() { let a = st.app.engine.on_reply(r); run_actions(st, a); }
    if bridge::take(&bridge::REQ_TOGGLE_PAUSE) { let o = st.app.engine.on_key(Key::Hotkey(Hotkey::TogglePause)); run_actions(st, o.actions); let _ = st.tx.send(UiMsg::Paused(st.app.engine.paused)); }
    let idle = st.app.engine.on_idle(std::time::Instant::now()); run_actions(st, idle);
    if st.gate.refresh_due(Instant::now(), false) { field_refresh(st); }
    let now = Instant::now();
    if let Some((_, key)) = st.badge_due.filter(|(t, _)| now >= *t) { st.badge_due = None; unsafe { focus_look(st, key) } }
    // the caret under a bubble or preview that is up follows the app's own (`arm_caret_follow`): re-read it now that
    // the app has laid the glyph out
    if st.follow.take(now, st.app.engine.bubble_visible() || st.app.engine.badge_shown()) {
        let an = unsafe { anchor(st) };
        let _ = st.tx.send(if st.app.engine.bubble_visible() { UiMsg::Move(an) } else { UiMsg::MoveBadge(an) });
    }
    if bridge::take(&bridge::REQ_QUIT) { st.app.save_and_wait(); let _ = st.tx.send(UiMsg::Stopped); return true }
    false
}
/// Low-level hooks are delivered to the installing thread's message loop, so the keyboard hook lives on its own thread
/// with the Engine (the mouse hook on another, `mouse_thread`); gpui owns the main thread. A 25 ms thread timer services
/// tray requests (pause / quit), clicks and the typing pause that brings up word completions.
fn hook_thread(app: App, tx: mpsc::Sender<UiMsg>) {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).expect("module").into();
        ST.with(|s| *s.borrow_mut() = Some(State { app, worker: rewrite::Worker::spawn(WinHost::new), tx: tx.clone(), last_fg: HWND::default(), last_focus: HWND::default(), secure: false, uia: Some(uia_worker()), uia_caret: Some(uia_worker()), last_click: None, gate: field::Gate::default(), field_rx: None, text_app: false, layouts: crate::badge::Watch::new(), follow: follow::Follow::default(), badge_due: Some((Instant::now(), false)) }));
        HOOK_TID.store(GetCurrentThreadId(), Ordering::SeqCst);
        let Ok(kb) = SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinst), 0) else {
            eprintln!("SetWindowsHookEx failed"); let _ = tx.send(UiMsg::Stopped); return
        };
        let _ = MOUSE_TX.set(tx.clone());
        std::thread::Builder::new().name("typethai-mouse".into()).spawn(mouse_thread).expect("mouse thread");
        let timer = SetTimer(None, 0, 25, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER && msg.hwnd.is_invalid() { if with(|st| control(st)).unwrap_or(false) { break } continue }
            if msg.message == WM_APP && msg.hwnd.is_invalid() { let _ = with(|st| take_click(st)); continue }
            let _ = TranslateMessage(&msg); DispatchMessageW(&msg);
        }
        let _ = KillTimer(None, timer);
        let _ = UnhookWindowsHookEx(kb);
    }
}

pub fn run(app: App) {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("typethai-hooks".into()).spawn(move || hook_thread(app, tx)).expect("hook thread");
    crate::ui::run(rx);
}
