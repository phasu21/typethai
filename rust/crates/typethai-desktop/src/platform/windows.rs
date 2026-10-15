//! Windows backend: WH_KEYBOARD_LL / WH_MOUSE_LL hooks, SendInput (unicode) injection, a non-activating topmost bubble,
//! caret lookup (GetGUIThreadInfo -> UI Automation TextPattern2 -> focused element rect -> mouse), secure-field skip
//! (ES_PASSWORD / UIA IsPassword), switch to Thai/US layout after applying, tray icon.
use crate::{app::{hotkey_for, App}, bridge::{self, UiMsg}, engine::{Action, Boundary, Hotkey, InputLang, Key}, inputsrc::Source, keys::win_vk_to_us, placement::{Anchor, Rect}};
use std::cell::{Cell, RefCell};
use std::sync::mpsc;
use std::time::Duration;
use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Ole::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Marks our own injected events (dwExtraInfo) so the hook ignores them.
const MAGIC: usize = 0x5454_4841;
const ES_PASSWORD_STYLE: i32 = 0x0020;
const THAI_KLID: usize = 0x041E;

struct State { app: App, tx: mpsc::Sender<UiMsg>, last_fg: HWND, last_focus: HWND, secure: bool, uia: Option<mpsc::Sender<(UiaQuery, mpsc::Sender<UiaResp>)>> }
thread_local! { static ST: RefCell<Option<State>> = const { RefCell::new(None) }; static BUSY: Cell<bool> = const { Cell::new(false) }; }
fn with<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> { ST.with(|s| s.try_borrow_mut().ok().and_then(|mut s| s.as_mut().map(f))) }

// ---------- UI Automation worker (MTA thread, so UIA calls never run on the hook thread) ----------
enum UiaQuery { IsPassword, Caret, Field }
enum UiaResp { Bool(bool), Rect(Option<RECT>) }
fn uia_worker() -> mpsc::Sender<(UiaQuery, mpsc::Sender<UiaResp>)> {
    let (tx, rx) = mpsc::channel::<(UiaQuery, mpsc::Sender<UiaResp>)>();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED).ok();
        let auto: Option<IUIAutomation> = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok();
        for (q, reply) in rx {
            let el = auto.as_ref().and_then(|a| a.GetFocusedElement().ok());
            let r = match q {
                UiaQuery::IsPassword => UiaResp::Bool(el.as_ref().and_then(|e| e.CurrentIsPassword().ok()).map(|b| b.as_bool()).unwrap_or(false)),
                UiaQuery::Caret => UiaResp::Rect(el.as_ref().and_then(|e| uia_caret(e))),
                UiaQuery::Field => UiaResp::Rect(el.as_ref().and_then(|e| e.CurrentBoundingRectangle().ok())),
            };
            let _ = reply.send(r);
        }
    });
    tx
}
unsafe fn uia_caret(e: &IUIAutomationElement) -> Option<RECT> {
    let tp: IUIAutomationTextPattern2 = e.GetCurrentPatternAs(UIA_TextPattern2Id).ok()?;
    let mut active = windows::core::BOOL(0);
    let range = tp.GetCaretRange(&mut active).ok()?;
    let rect = |r: &IUIAutomationTextRange| -> Option<RECT> {
        let sa = r.GetBoundingRectangles().ok()?; if sa.is_null() { return None }
        let ub = SafeArrayGetUBound(sa, 1).ok()?; let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        let out = if ub >= 3 && SafeArrayAccessData(sa, &mut p).is_ok() {
            let d = std::slice::from_raw_parts(p as *const f64, 4); let _ = SafeArrayUnaccessData(sa);
            Some(RECT { left: d[0] as i32, top: d[1] as i32, right: (d[0] + d[2]) as i32, bottom: (d[1] + d[3]) as i32 })
        } else { None };
        let _ = SafeArrayDestroy(sa); out
    };
    rect(&range).or_else(|| { let c = range.Clone().ok()?; c.ExpandToEnclosingUnit(TextUnit_Character).ok()?; rect(&c) })
}
fn uia_ask(st: &State, q: UiaQuery) -> Option<UiaResp> {
    let (tx, rx) = mpsc::channel(); st.uia.as_ref()?.send((q, tx)).ok()?; rx.recv_timeout(Duration::from_millis(60)).ok()
}

// ---------- focus / secure / caret ----------
unsafe fn gui_info(fg: HWND) -> Option<GUITHREADINFO> {
    let tid = GetWindowThreadProcessId(fg, None);
    let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
    GetGUIThreadInfo(tid, &mut gi).ok()?; Some(gi)
}
unsafe fn is_secure(st: &State, focus: HWND) -> bool {
    if !focus.is_invalid() {
        let mut cls = [0u16; 64]; let n = GetClassNameW(focus, &mut cls) as usize;
        let cls = String::from_utf16_lossy(&cls[..n]).to_ascii_lowercase();
        if cls.contains("edit") && GetWindowLongW(focus, GWL_STYLE) & ES_PASSWORD_STYLE != 0 { return true }
    }
    matches!(uia_ask(st, UiaQuery::IsPassword), Some(UiaResp::Bool(true)))
}
fn rect(r: RECT) -> Rect { Rect::new(r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64) }
/// Caret rect (GetGUIThreadInfo, else UIA TextPattern2), focused element frame (UIA, else the focus HWND) and the
/// mouse, in physical screen pixels (gpui makes the process per-monitor DPI aware).
unsafe fn anchor(st: &State) -> Anchor {
    let fg = GetForegroundWindow(); let gi = gui_info(fg);
    let mut a = Anchor::default();
    if let Some(gi) = gi {
        if !gi.hwndCaret.is_invalid() && (gi.rcCaret.right > gi.rcCaret.left || gi.rcCaret.bottom > gi.rcCaret.top) {
            let (mut p1, mut p2) = (POINT { x: gi.rcCaret.left, y: gi.rcCaret.top }, POINT { x: gi.rcCaret.right, y: gi.rcCaret.bottom });
            let _ = ClientToScreen(gi.hwndCaret, &mut p1); let _ = ClientToScreen(gi.hwndCaret, &mut p2);
            a.caret = Some(Rect::new(p1.x as f64, p1.y as f64, (p2.x - p1.x).max(1) as f64, (p2.y - p1.y) as f64));
        }
    }
    if a.caret.is_none() { if let Some(UiaResp::Rect(Some(r))) = uia_ask(st, UiaQuery::Caret) { a.caret = Some(rect(r)); } }
    if let Some(UiaResp::Rect(Some(r))) = uia_ask(st, UiaQuery::Field) { a.field = Some(rect(r)); }
    else if let Some(gi) = gi { let mut r = RECT::default(); if !gi.hwndFocus.is_invalid() && GetWindowRect(gi.hwndFocus, &mut r).is_ok() { a.field = Some(rect(r)); } }
    let mut p = POINT::default(); if GetCursorPos(&mut p).is_ok() { a.mouse = Some((p.x as f64, p.y as f64)); }
    a
}
unsafe fn layout_is_thai(fg: HWND) -> bool { (GetKeyboardLayout(GetWindowThreadProcessId(fg, None)).0 as usize & 0xFFFF) == THAI_KLID }

// ---------- injection ----------
fn kbd(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } }
}
fn tap(v: &mut Vec<INPUT>, vk: VIRTUAL_KEY) { v.push(kbd(vk, 0, KEYBD_EVENT_FLAGS(0))); v.push(kbd(vk, 0, KEYEVENTF_KEYUP)); }
unsafe fn inject(delete: usize, insert: &str, then: Option<Boundary>) {
    let mut v = Vec::new();
    // release held modifiers (hotkeys are Ctrl+Alt+...), otherwise our backspaces become Ctrl+Alt+Backspace
    for m in [VK_LCONTROL, VK_RCONTROL, VK_LMENU, VK_RMENU, VK_LSHIFT, VK_RSHIFT] { if GetAsyncKeyState(m.0 as i32) < 0 { v.push(kbd(m, 0, KEYEVENTF_KEYUP)); } }
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
        Action::ShowBubble(items) => { let an = anchor(st); let _ = st.tx.send(UiMsg::Show { items: items.into_iter().map(|i| i.label).collect(), anchor: an }); }
        Action::HideBubble => { bridge::set_geom(None); let _ = st.tx.send(UiMsg::Hide); }
    } }
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
    if ctrl || alt || win { return Some(Key::Nav) }
    Some(match v {
        VK_BACK => Key::Backspace, VK_SPACE => Key::Boundary(Boundary::Space), VK_RETURN => Key::Boundary(Boundary::Enter),
        VK_TAB => if shift { Key::Nav } else { Key::Tab }, // Shift+Tab (reverse focus) stays plain navigation
        VK_ESCAPE => Key::Esc,
        VK_UP if shift => Key::SelectPrev, VK_DOWN if shift => Key::SelectNext,
        _ => match win_vk_to_us(vk, shift, caps) { Some(c) => Key::Char(c), None => Key::Nav },
    })
}
unsafe extern "system" fn kb_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && !BUSY.get() {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        let is_down = wp.0 as u32 == WM_KEYDOWN || wp.0 as u32 == WM_SYSKEYDOWN;
        if is_down && k.dwExtraInfo != MAGIC {
            BUSY.set(true);
            let swallow = with(|st| handle_key(st, k.vkCode)).unwrap_or(false);
            BUSY.set(false);
            if swallow { return LRESULT(1) }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}
fn foreground_is_self(fg: HWND) -> bool {
    let mut pid = 0u32;
    let _ = unsafe { GetWindowThreadProcessId(fg, Some(&mut pid)) };
    pid != 0 && pid == unsafe { GetCurrentProcessId() }
}
/// Keys while our Settings window is in front are left alone, so the field can be typed in.
fn typing_in_our_window(st: &mut State, fg: HWND) -> bool {
    if !foreground_is_self(fg) { return false }
    if fg != st.last_fg {
        st.last_fg = fg;
        st.last_focus = gui_info(fg).map(|g| g.hwndFocus).unwrap_or_default();
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
    let _ = st.tx.send(UiMsg::Settings(snap));
}
unsafe fn handle_key(st: &mut State, vk: u32) -> bool {
    if std::env::var_os("TYPETHAI_DEBUG").is_some() { eprintln!("vk {vk:#x}"); }
    let fg = GetForegroundWindow();
    if typing_in_our_window(st, fg) { return false }
    let Some(key) = translate(vk) else { return false };
    let focus = gui_info(fg).map(|g| g.hwndFocus).unwrap_or_default();
    if fg != st.last_fg || focus != st.last_focus { st.last_fg = fg; st.last_focus = focus; st.secure = false; let a = st.app.engine.reset(); run_actions(st, a); }
    // Thai layout / password field: bypass the engine, except hotkeys and the bubble controls while the bubble is up
    st.app.engine.set_thai_active(layout_is_thai(fg)); // Thai layout -> reverse detector (English typed on Kedmanee)
    if !matches!(key, Key::Hotkey(_)) && !st.app.engine.is_bubble_key(&key) {
        if layout_is_thai(fg) && st.app.engine.rev.is_none() { let a = st.app.engine.reset(); run_actions(st, a); return false }
        // check for a password field at the start of each word and at the boundary (UIA is queried off-thread, 60 ms cap)
        if (matches!(key, Key::Char(_)) && st.app.engine.buffer().is_empty()) || matches!(key, Key::Boundary(_) | Key::Tab) { st.secure = is_secure(st, focus); }
        if st.secure { let a = st.app.engine.reset(); run_actions(st, a); return false }
    }
    let was_paused = st.app.engine.paused;
    let dbg = std::env::var_os("TYPETHAI_DEBUG").is_some();
    if dbg { eprintln!("key {key:?} buf={:?}", st.app.engine.buffer()); }
    let o = st.app.engine.on_key(key);
    if dbg && !o.actions.is_empty() { eprintln!("  -> swallow={} {:?}", o.swallow, o.actions); }
    if was_paused != st.app.engine.paused { let _ = st.tx.send(UiMsg::Paused(st.app.engine.paused)); }
    run_actions(st, o.actions); st.app.maybe_save();
    o.swallow
}
unsafe extern "system" fn mouse_proc(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let m = wp.0 as u32;
    if code == HC_ACTION as i32 && (m == WM_LBUTTONDOWN || m == WM_RBUTTONDOWN || m == WM_MBUTTONDOWN) {
        let pt = (*(lp.0 as *const MSLLHOOKSTRUCT)).pt;
        let swallow = with(|st| match bridge::geom().and_then(|g| g.hit(pt.x as f64, pt.y as f64)) {
            // on the bubble: swallow so it is never clicked/activated; a left click on a row accepts that item
            Some(row) => { if let (WM_LBUTTONDOWN, Some(i)) = (m, row) { let o = st.app.engine.on_key(Key::Hotkey(Hotkey::Accept(i))); run_actions(st, o.actions); } true }
            None => { let a = st.app.engine.reset(); run_actions(st, a); false }
        }).unwrap_or(false);
        if swallow { return LRESULT(1) }
    }
    CallNextHookEx(None, code, wp, lp)
}

// ---------- hook thread ----------
unsafe fn control(st: &mut State) -> bool {
    service_settings(st);
    if bridge::take(&bridge::REQ_TOGGLE_PAUSE) { let o = st.app.engine.on_key(Key::Hotkey(Hotkey::TogglePause)); run_actions(st, o.actions); let _ = st.tx.send(UiMsg::Paused(st.app.engine.paused)); }
    let idle = st.app.engine.on_idle(std::time::Instant::now()); run_actions(st, idle);
    if bridge::take(&bridge::REQ_QUIT) { st.app.save(); let _ = st.tx.send(UiMsg::Stopped); return true }
    false
}
/// Low-level hooks are delivered to the installing thread's message loop, so they live on their own thread with the
/// Engine; gpui owns the main thread. A 100 ms thread timer services tray requests (pause / quit) and the typing pause
/// that brings up word completions.
fn hook_thread(app: App, tx: mpsc::Sender<UiMsg>) {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).expect("module").into();
        ST.with(|s| *s.borrow_mut() = Some(State { app, tx: tx.clone(), last_fg: HWND::default(), last_focus: HWND::default(), secure: false, uia: Some(uia_worker()) }));
        let (Ok(kb), Ok(ms)) = (SetWindowsHookExW(WH_KEYBOARD_LL, Some(kb_proc), Some(hinst), 0), SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), Some(hinst), 0)) else {
            eprintln!("SetWindowsHookEx failed"); let _ = tx.send(UiMsg::Stopped); return
        };
        let timer = SetTimer(None, 0, 100, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER && msg.hwnd.is_invalid() { if with(|st| control(st)).unwrap_or(false) { break } continue }
            let _ = TranslateMessage(&msg); DispatchMessageW(&msg);
        }
        let _ = KillTimer(None, timer);
        let _ = UnhookWindowsHookEx(kb); let _ = UnhookWindowsHookEx(ms);
    }
}

pub fn run(app: App) {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new().name("typethai-hooks".into()).spawn(move || hook_thread(app, tx)).expect("hook thread");
    crate::ui::run(rx);
}
