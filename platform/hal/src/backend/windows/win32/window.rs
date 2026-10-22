//! Native windows: creation, the WndProc that turns messages into HAL events, and the window
//! commands. Main thread only. Moved from os/windows/win32_window.rs and win32_app.rs; the
//! synchronous Cx callbacks became events, the drag query an asynchronous `DragQuery` /
//! `DragReply` round trip (a press in the client waits for the app's answer before it is a
//! click or a caption drag, as on X11).

use super::super::super::exec::OsCmd;
use super::{clipboard, dialog, drag, identity, ime, keys, lock, log_enabled, menu, rawinput, Key, Native, Op, Shared, WM_HAL_OPS};
use crate::types::*;
use crate::Id;
use makepad_windows_sys::core::PCWSTR;
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::Graphics::Dwm::*;
use makepad_windows_sys::Win32::Graphics::Gdi::*;
use makepad_windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use makepad_windows_sys::Win32::System::Ole::{IDropTarget, OleInitialize};
use makepad_windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use makepad_windows_sys::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use makepad_windows_sys::Win32::UI::HiDpi::*;
use makepad_windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

const CLASS: &str = "MakepadHalWindow";
const DISPATCH_CLASS: &str = "MakepadHalDispatch";
/// the resize band of a client-sized window, in points
const RESIZE_EDGE: f64 = 8.0;

#[derive(Clone, Copy)]
pub(super) struct Press {
    button: u32,
    x: f64,
    y: f64,
    mods: u32,
    screen: POINT,
}

pub(super) struct Win {
    pub(super) hwnd: Cell<HWND>,
    /// the physical identity: the session and id it was made for
    own: Key,
    /// who hears its events (a borrower of a lent host window)
    pub(super) recipient: Cell<Key>,
    kind: u32,
    parent: Option<Key>,
    /// set once CreateWindowExW returned (the registered record the backend reads)
    native: RefCell<Option<Arc<Native>>>,
    pub(super) dpi: Cell<f64>,
    tracking: Cell<bool>,
    pub(super) pending: Cell<Option<Press>>,
    high_surrogate: Cell<u16>,
    cursor: Cell<u32>,
    /// closed because its session went: no `Closed` event
    silent: Cell<bool>,
    hidden_until_present: Cell<bool>,
    pub(super) buttons: Cell<u32>,
    last_pos: Cell<(f64, f64)>,
    no_focus: bool,
    pub(super) ime: ime::ImeState,
    /// the app's last answer to a `DragOver` (`drag_response`): what an OLE drop gets now
    pub(super) drag_answer: Cell<u32>,
    pub(super) drop_target: RefCell<Option<IDropTarget>>,
    /// a maximized window keeps no frame inset and covers its monitor
    chromeless_when_max: Cell<bool>,
    /// fullscreen: maximized over the whole monitor (taskbar covered) with no frame inset;
    /// a restore puts back the rect it had before
    fullscreen: Cell<bool>,
    /// Alt went down alone: its release opens the menu
    alt_tap: Cell<bool>,
    /// keys whose press reached the app (their release goes too), a bit per virtual key
    forwarded: Cell<[u64; 4]>,
}

impl Win {
    pub(super) fn push(&self, f: impl FnOnce(&mut crate::buffers::EventQueue, Id)) {
        Shared::global().push(self.recipient.get(), f);
    }
    fn popup(&self) -> bool {
        self.kind & 0xff == window_kind::POPUP
    }
    fn maximized(&self) -> bool {
        unsafe { IsZoomed(self.hwnd.get()).as_bool() }
    }
    /// A maximized window covers the whole monitor, frameless: chromeless when maximized,
    /// or fullscreen.
    fn covers_monitor(&self) -> bool {
        self.chromeless_when_max.get() || self.fullscreen.get()
    }
}

#[derive(Default)]
struct App {
    wins: HashMap<Key, Rc<Win>>,
    cursors: HashMap<u32, HCURSOR>,
    /// the app icon of each session (`AppIcon`)
    icons: HashMap<u32, HICON>,
}

thread_local! {
    static APP: RefCell<App> = RefCell::new(App::default());
    /// OLE is initialized on the main thread: windows are drop targets
    static OLE: Cell<bool> = const { Cell::new(false) };
    /// the app is the active one (popups are dismissed once when it stops being)
    static APP_ACTIVE: Cell<bool> = const { Cell::new(true) };
}

pub(super) fn win_of(hwnd: HWND) -> Option<Rc<Win>> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Win;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: GWLP_USERDATA holds an Rc<Win> pointer until WM_NCDESTROY takes it back
    unsafe {
        Rc::increment_strong_count(ptr);
        Some(Rc::from_raw(ptr))
    }
}

fn find(key: Key) -> Option<Rc<Win>> {
    APP.with_borrow(|a| a.wins.get(&key).cloned())
}

/// The app theme the user chose (Settings > Personalization > Colors: AppsUseLightTheme).
pub fn system_dark() -> Option<bool> {
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, Some(PCWSTR(key.as_ptr())), Some(PCWSTR(value.as_ptr())), RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size))
    };
    (r.0 == 0).then_some(data == 0)
}

/// The window frame follows the system theme (borders, shadow, system menu).
fn apply_dark(hwnd: HWND) {
    let dark = makepad_windows_sys::core::BOOL(system_dark().unwrap_or(false) as i32);
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, std::mem::size_of_val(&dark) as u32);
    }
}

fn modifiers() -> u32 {
    let down = |vk: VIRTUAL_KEY| unsafe { GetKeyState(vk.0 as i32) } as u16 & 0x8000 != 0;
    let mut m = 0;
    if down(VK_SHIFT) {
        m |= input::MOD_SHIFT;
    }
    if down(VK_CONTROL) {
        m |= input::MOD_CONTROL;
    }
    if down(VK_MENU) {
        m |= input::MOD_ALT;
    }
    if down(VK_LWIN) || down(VK_RWIN) {
        m |= input::MOD_LOGO;
    }
    m
}

fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xffff) as u16 as i16 as i32, ((lparam.0 >> 16) & 0xffff) as u16 as i16 as i32)
}

fn dpi_of(hwnd: HWND) -> f64 {
    let d = unsafe { GetDpiForWindow(hwnd) };
    if d == 0 { 1.0 } else { d as f64 / 96.0 }
}

/// Makes the process per-monitor-v2 DPI aware and answers the system DPI factor.
pub(super) fn dpi_awareness() -> f32 {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() && log_enabled() {
            eprintln!("[hal-win32] per-monitor v2 DPI awareness was already set or is unavailable");
        }
        let dpi = GetDpiForSystem();
        (if dpi == 0 { 96 } else { dpi }) as f32 / 96.0
    }
}

pub(super) fn start(s: &'static Shared) -> Result<(), String> {
    unsafe {
        // per-monitor v2 before the first HWND
        let dpi = super::system_dpi();
        let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(0),
            lpfnWndProc: Some(window_proc),
            hInstance: HINSTANCE(instance.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            return Err("RegisterClassExW(window)".into());
        }
        let dclass = wide(DISPATCH_CLASS);
        let dc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(dispatch_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: PCWSTR(dclass.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&dc) == 0 {
            return Err("RegisterClassExW(dispatch)".into());
        }
        let dispatch = CreateWindowExW(WINDOW_EX_STYLE(0), Some(PCWSTR(dclass.as_ptr())), None, WINDOW_STYLE(0), 0, 0, 0, 0,
            Some(HWND_MESSAGE), None, Some(HINSTANCE(instance.0)), None);
        if dispatch.0.is_null() {
            return Err("CreateWindowExW(dispatch)".into());
        }
        s.dispatch.store(dispatch.0 as usize, Ordering::Release);
        s.dpi.store(dpi.to_bits(), Ordering::Release);
        identity::process_started();
        // the main thread is OLE's STA: drop targets and outgoing drags
        match OleInitialize(std::ptr::null()) {
            Ok(()) => OLE.set(true),
            Err(e) => eprintln!("[hal-win32] OleInitialize failed ({e:?}): no drag and drop"),
        }
        refresh_screens(s);
        // ops queued before the dispatch window existed, and a stop that came before it
        // (stop_main_loop sets `stopped` before it reads `dispatch`: it posts the stop itself, or
        // this sees it) -- else the loop never ended
        let stopped = lock(&s.run).stopped;
        let _ = PostMessageW(Some(dispatch), WM_HAL_OPS, WPARAM(stopped as usize), LPARAM(0));
    }
    Ok(())
}

pub(super) fn message_loop(_s: &'static Shared) {
    unsafe {
        let mut msg = MSG::default();
        loop {
            let r = GetMessageW(&mut msg, None, 0, 0);
            if r.0 == 0 || r.0 == -1 {
                break;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn monitor_proc(monitor: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> makepad_windows_sys::core::BOOL {
    let out = &mut *(data.0 as *mut Vec<crate::ScreenInfo>);
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if GetMonitorInfoW(monitor, &mut info).as_bool() {
        // Windows' global coordinates: physical pixels of the virtual desktop, top-left origin
        let r = |r: RECT| [r.left as f64, r.top as f64, (r.right - r.left) as f64, (r.bottom - r.top) as f64];
        out.push(crate::ScreenInfo { bounds: r(info.rcMonitor), work_area: r(info.rcWork), is_primary: info.dwFlags & 1 != 0 });
    }
    makepad_windows_sys::core::BOOL(1)
}

fn refresh_screens(s: &Shared) {
    let mut out: Vec<crate::ScreenInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(monitor_proc), LPARAM(&mut out as *mut _ as isize));
    }
    *lock(&s.screens) = out;
}

unsafe extern "system" fn dispatch_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TIMER && wparam.0 == clipboard::RETRY_TIMER {
        clipboard::retry(hwnd);
        return LRESULT(0);
    }
    if msg == WM_HAL_OPS {
        let s = Shared::global();
        for op in s.take_ops() {
            apply(s, op);
        }
        if wparam.0 == 1 {
            // stop: windows go silently, then the loop ends
            let all: Vec<Rc<Win>> = APP.with_borrow(|a| a.wins.values().cloned().collect());
            for w in all {
                w.silent.set(true);
                let _ = DestroyWindow(w.hwnd.get());
            }
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn apply(s: &'static Shared, op: Op) {
    match op {
        Op::Cmd(sid, cmd) => command(s, sid, cmd),
        Op::CloseSession(sid) => {
            let wins: Vec<Rc<Win>> = APP.with_borrow(|a| a.wins.values().filter(|w| w.own.0 == sid).cloned().collect());
            for w in wins {
                w.silent.set(true);
                unsafe {
                    let _ = DestroyWindow(w.hwnd.get());
                }
            }
            menu::session_closed(sid);
            clipboard::session_closed(sid);
            if let Some(icon) = APP.with_borrow_mut(|a| a.icons.remove(&sid)) {
                unsafe {
                    let _ = DestroyIcon(icon);
                }
            }
        }
        Op::Lend { host, session, alias } => {
            if let Some(w) = find((crate::session::HOST, host)) {
                w.recipient.set(if session == crate::session::HOST { (crate::session::HOST, host) } else { (session, alias) });
                publish_geom(&w);
            }
        }
        Op::Reveal => {
            let wins: Vec<Rc<Win>> = APP.with_borrow(|a| a.wins.values().cloned().collect());
            for w in wins {
                if w.hidden_until_present.replace(false) {
                    show(&w);
                }
            }
        }
    }
}

fn show(w: &Win) {
    unsafe {
        let cmd = if w.no_focus || w.popup() { SW_SHOWNOACTIVATE } else { SW_SHOW };
        let _ = ShowWindow(w.hwnd.get(), cmd);
    }
}

fn style_for(kind: u32) -> (WINDOW_STYLE, WINDOW_EX_STYLE) {
    match kind & 0xff {
        window_kind::POPUP => (WS_POPUP, WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE),
        window_kind::CHROMELESS => (WS_POPUP | WS_THICKFRAME, WS_EX_APPWINDOW),
        _ => (WS_OVERLAPPEDWINDOW, WS_EX_APPWINDOW),
    }
}

fn create(s: &'static Shared, sid: u32, id: Id, kind: u32, parent: Id, geom: [f32; 4]) {
    let key = (sid, id);
    let parent_key = (!parent.is_none()).then_some((sid, parent));
    let parent_hwnd = parent_key.and_then(find).map(|w| w.hwnd.get());
    let (style, ex) = style_for(kind);
    let dpi = parent_hwnd.map(dpi_of).unwrap_or(s.dpi() as f64);
    let flags = s.flags();
    let title = wide("Makepad");
    let class = wide(CLASS);
    let placed = !geom[0].is_nan() && !geom[1].is_nan();
    let (w, h) = ((geom[2].max(1.0) as f64 * dpi).round() as i32, (geom[3].max(1.0) as f64 * dpi).round() as i32);
    let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
    if placed {
        x = (geom[0] as f64 * dpi).round() as i32;
        y = (geom[1] as f64 * dpi).round() as i32;
        if let (Some(p), true) = (parent_hwnd, kind & 0xff == window_kind::POPUP) {
            // popup positions are relative to the parent's client area
            let mut pt = POINT { x, y };
            unsafe {
                let _ = ClientToScreen(p, &mut pt);
            }
            x = pt.x;
            y = pt.y;
        }
    }
    let win = Rc::new(Win {
        hwnd: Cell::new(HWND(std::ptr::null_mut())),
        own: key,
        recipient: Cell::new(key),
        kind,
        parent: parent_key,
        native: RefCell::new(None),
        dpi: Cell::new(dpi),
        tracking: Cell::new(false),
        pending: Cell::new(None),
        high_surrogate: Cell::new(0),
        cursor: Cell::new(cursor::DEFAULT),
        silent: Cell::new(false),
        hidden_until_present: Cell::new(flags & open_flags::HIDDEN_UNTIL_PRESENT != 0 && !s.revealed.load(Ordering::Acquire)),
        buttons: Cell::new(0),
        last_pos: Cell::new((0.0, 0.0)),
        no_focus: flags & open_flags::NO_FOCUS != 0,
        ime: ime::ImeState::default(),
        drag_answer: Cell::new(input::drag_response::NONE),
        drop_target: RefCell::new(None),
        chromeless_when_max: Cell::new(false),
        fullscreen: Cell::new(false),
        alt_tap: Cell::new(false),
        forwarded: Cell::new([0; 4]),
    });
    // WM_NCCREATE takes this reference into GWLP_USERDATA (and the map takes its own)
    let raw = Rc::into_raw(win.clone());
    let hwnd = unsafe {
        let instance = GetModuleHandleW(None).ok().map(|m| HINSTANCE(m.0));
        CreateWindowExW(ex, Some(PCWSTR(class.as_ptr())), Some(PCWSTR(title.as_ptr())), style, x, y, w, h, parent_hwnd, None, instance, Some(raw as *const _))
    };
    if hwnd.0.is_null() {
        if win.hwnd.get().0.is_null() {
            // WM_NCCREATE never ran: the reference was not taken
            unsafe { drop(Rc::from_raw(raw)) };
        }
        eprintln!("[hal-win32] CreateWindowExW failed for window {key:?}");
        s.push(key, |q, id| q.closed(id));
        return;
    }
    *win.native.borrow_mut() = Some(s.register(key, hwnd, win.popup()));
    if !win.popup() {
        unsafe {
            let margins = MARGINS { cxLeftWidth: 0, cxRightWidth: 0, cyTopHeight: 0, cyBottomHeight: 0 };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            let policy = DWMNCRP_ENABLED;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_NCRENDERING_POLICY, &policy as *const _ as *const _, std::mem::size_of_val(&policy) as u32);
            let corner = DWMWCP_ROUND;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &corner as *const _ as *const _, std::mem::size_of_val(&corner) as u32);
            // a client-sized frame (WM_NCCALCSIZE below) now that the state pointer is set
            let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
            if placed {
                // the requested size is the client size
                let _ = SetWindowPos(hwnd, None, 0, 0, w, h, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }
    win.dpi.set(dpi_of(hwnd));
    ime::window_created(&win);
    if !win.popup() {
        identity::window_created(hwnd);
        apply_dark(hwnd);
        if OLE.get() {
            drag::register(&win);
        }
        if let Some(icon) = APP.with_borrow(|a| a.icons.get(&sid).copied()) {
            set_icon(hwnd, icon);
        }
    }
    publish_geom(&win);
    if !win.hidden_until_present.get() {
        show(&win);
    }
    if log_enabled() {
        eprintln!("[hal-win32] window {key:?} kind={kind:#x} hwnd={:?} dpi={}", hwnd.0, win.dpi.get());
    }
}

/// Publishes the window's geometry (points) and client pixels.
fn publish_geom(w: &Win) {
    let hwnd = w.hwnd.get();
    let dpi = dpi_of(hwnd);
    w.dpi.set(dpi);
    let mut client = RECT::default();
    let mut outer = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(hwnd, &mut outer);
    }
    let (cw, ch) = ((client.right - client.left).max(0) as u32, (client.bottom - client.top).max(0) as u32);
    if let Some(n) = w.native.borrow().as_ref() {
        n.pixels.store((cw as u64) << 32 | ch as u64, Ordering::Release);
    }
    let state = unsafe {
        if IsIconic(hwnd).as_bool() {
            window_state::MINIMIZE
        } else if IsZoomed(hwnd).as_bool() {
            if w.fullscreen.get() { window_state::FULLSCREEN } else { window_state::MAXIMIZE }
        } else {
            // restored by any path (the system menu, a key): no longer fullscreen
            w.fullscreen.set(false);
            window_state::RESTORE
        }
    };
    let (mut x, mut y) = (outer.left as f64 / dpi, outer.top as f64 / dpi);
    if let (true, Some(parent)) = (w.popup(), w.parent.and_then(find)) {
        let mut pt = POINT { x: outer.left, y: outer.top };
        unsafe {
            let _ = ScreenToClient(parent.hwnd.get(), &mut pt);
        }
        x = pt.x as f64 / dpi;
        y = pt.y as f64 / dpi;
    }
    let (wp, hp) = (cw as f64 / dpi, ch as f64 / dpi);
    w.push(|q, id| q.geom(id, x, y, wp, hp, dpi as f32, state));
}

fn command(s: &'static Shared, sid: u32, cmd: OsCmd) {
    let target = |id: Id| find((sid, id));
    match cmd {
        OsCmd::WindowCreate { id, kind, parent, geom } => create(s, sid, id, kind, parent, geom),
        OsCmd::WindowClose { id } => {
            if let Some(w) = target(id) {
                unsafe {
                    let _ = DestroyWindow(w.hwnd.get());
                }
            }
        }
        OsCmd::WindowGeom { id, geom } => {
            if let Some(w) = target(id) {
                let dpi = w.dpi.get();
                let (cw, ch) = ((geom[2] as f64 * dpi).round() as i32, (geom[3] as f64 * dpi).round() as i32);
                let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
                let (mut x, mut y) = (0, 0);
                if geom[0].is_nan() || geom[1].is_nan() {
                    flags = flags | SWP_NOMOVE;
                } else {
                    x = (geom[0] as f64 * dpi).round() as i32;
                    y = (geom[1] as f64 * dpi).round() as i32;
                    if let (true, Some(p)) = (w.popup(), w.parent.and_then(find)) {
                        let mut pt = POINT { x, y };
                        unsafe {
                            let _ = ClientToScreen(p.hwnd.get(), &mut pt);
                        }
                        x = pt.x;
                        y = pt.y;
                    }
                }
                unsafe {
                    let _ = SetWindowPos(w.hwnd.get(), None, x, y, cw, ch, flags);
                }
                publish_geom(&w);
            }
        }
        OsCmd::WindowState { id, state } => {
            if let Some(w) = target(id) {
                // fullscreen is a maximize over the whole monitor: switching between the two
                // goes through the normal rect, so a later restore finds it
                let full = state == window_state::FULLSCREEN;
                if matches!(state, window_state::MAXIMIZE | window_state::FULLSCREEN) && w.fullscreen.get() != full {
                    if w.maximized() {
                        unsafe {
                            let _ = ShowWindow(w.hwnd.get(), SW_RESTORE);
                        }
                    }
                    w.fullscreen.set(full);
                }
                let cmd = match state {
                    window_state::MINIMIZE => SW_MINIMIZE,
                    window_state::MAXIMIZE | window_state::FULLSCREEN => SW_MAXIMIZE,
                    window_state::HIDE => SW_HIDE,
                    window_state::SHOW => if w.no_focus { SW_SHOWNOACTIVATE } else { SW_SHOW },
                    _ => SW_RESTORE,
                };
                unsafe {
                    let _ = ShowWindow(w.hwnd.get(), cmd);
                }
                publish_geom(&w);
            }
        }
        OsCmd::WindowTitle { id, title } => {
            if let Some(w) = target(id) {
                let t = wide(&title);
                unsafe {
                    let _ = SetWindowTextW(w.hwnd.get(), Some(PCWSTR(t.as_ptr())));
                }
            }
        }
        OsCmd::WindowTopmost { id, on } => {
            if let Some(w) = target(id) {
                unsafe {
                    let _ = SetWindowPos(w.hwnd.get(), Some(if on { HWND_TOPMOST } else { HWND_NOTOPMOST }), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
        OsCmd::Cursor { id, cursor } => {
            if let Some(w) = target(id) {
                w.cursor.set(cursor);
                set_cursor(cursor);
            }
        }
        OsCmd::ActivateApp => {
            if let Some(w) = APP.with_borrow(|a| a.wins.values().find(|w| w.own.0 == sid && !w.popup()).cloned()) {
                unsafe {
                    let _ = SetForegroundWindow(w.hwnd.get());
                }
            }
        }
        OsCmd::DragReply { id, to, answer } => {
            if let Some(w) = target(id) {
                match to {
                    input::reply::QUERY => query_answer(&w, answer),
                    _ => w.drag_answer.set(answer),
                }
            }
        }
        OsCmd::ClipSet { text } => clipboard::set_text(&text),
        OsCmd::ClipRead { req, .. } => clipboard::read(HWND(s.dispatch.load(Ordering::Acquire) as _), sid, req),
        OsCmd::ImeShow { id, area, .. } => {
            if let Some(w) = target(id) {
                ime::show(&w, area);
            }
        }
        OsCmd::ImeHide { id } => {
            if let Some(w) = target(id) {
                ime::hide(&w);
            }
        }
        OsCmd::ImeSync { id, selection, composition, .. } => {
            if let Some(w) = target(id) {
                ime::sync(&w, selection, composition);
            }
        }
        OsCmd::MenuSet { items } => menu::set(sid, items),
        OsCmd::FileDialog { req, kind, title, location, filename, filters } => {
            let owner = APP.with_borrow(|a| a.wins.values().find(|w| w.own.0 == sid && !w.popup()).map(|w| w.hwnd.get().0 as usize)).unwrap_or(0);
            dialog::open(dialog::Request { sid, req, kind, title, location, filename, filters, owner });
        }
        OsCmd::DragStart { id, items } => match target(id) {
            Some(w) => drag::start(&w, &items),
            None => s.push((sid, id), |q, id| q.drag_drop(id, -100000.0, -100000.0, false, 0, &[])),
        },
        OsCmd::PointerLock { id, mode } => {
            if let Some(w) = target(id) {
                rawinput::command(&w, mode);
            }
        }
        OsCmd::AppIcon { w, h, rgba } => set_app_icon(sid, w, h, &rgba),
        OsCmd::WindowVisuals { id, visuals, params } => {
            if let Some(w) = target(id) {
                set_visuals(&w, visuals, params[0]);
            }
        }
        OsCmd::ChromelessWhenMax { id, on } => {
            if let Some(w) = target(id) {
                if w.chromeless_when_max.replace(on) != on {
                    // the frame insets of a maximized window change now
                    unsafe {
                        let _ = SetWindowPos(w.hwnd.get(), None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED);
                    }
                    publish_geom(&w);
                }
            }
        }
        // the taskbar shows every app window; the primary selection is X11's
        OsCmd::ShowInDock { .. } | OsCmd::SelectionSet { .. } | OsCmd::MobileUi { .. } | OsCmd::BackUnhandled | OsCmd::SelectionHandles { .. } => {}
        // The app scales its own caption buttons; only AppKit positions native ones.
        OsCmd::WindowDpiOverride { .. } => {}
        other => {
            if log_enabled() {
                eprintln!("[hal-win32] session {sid}: {other:?} is not implemented by the Win32 shell");
            }
        }
    }
}

/// Whether the shell executes `cmd` (and answers it itself); the executor answers the rest.
/// Window buttons are drawn by the app; sharing and clipboard action bars are mobile services.
pub(super) fn handles(cmd: &OsCmd) -> bool {
    matches!(
        cmd,
        OsCmd::WindowCreate { .. }
            | OsCmd::WindowClose { .. }
            | OsCmd::WindowGeom { .. }
            | OsCmd::WindowState { .. }
            | OsCmd::WindowTitle { .. }
            | OsCmd::WindowTopmost { .. }
            | OsCmd::WindowVisuals { .. }
            | OsCmd::WindowDpiOverride { .. }
            | OsCmd::ChromelessWhenMax { .. }
            | OsCmd::Cursor { .. }
            | OsCmd::PointerLock { .. }
            | OsCmd::ActivateApp
            | OsCmd::ShowInDock { .. }
            | OsCmd::DragReply { .. }
            | OsCmd::DragStart { .. }
            | OsCmd::ClipSet { .. }
            | OsCmd::ClipRead { .. }
            | OsCmd::SelectionSet { .. }
            | OsCmd::FrameRequest { .. }
            | OsCmd::ImeShow { .. }
            | OsCmd::ImeHide { .. }
            | OsCmd::ImeSync { .. }
            | OsCmd::MenuSet { .. }
            | OsCmd::FileDialog { .. }
            | OsCmd::AppIcon { .. }
    )
}

fn set_icon(hwnd: HWND, icon: HICON) {
    unsafe {
        for which in [ICON_BIG, ICON_SMALL] {
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(which as usize)), Some(LPARAM(icon.0 as isize)));
        }
    }
}

/// `AppIcon`: the session's windows (and the taskbar) show `rgba`.
fn set_app_icon(sid: u32, w: u32, h: u32, rgba: &[u8]) {
    let (width, height) = (w as usize, h as usize);
    if width == 0 || height == 0 || rgba.len() < width * height * 4 {
        return;
    }
    // CreateIcon: a 32-bit BGRA colour plane and an all-zero (opaque) 1-bit mask, rows padded to 16 bits
    let mut bgra = Vec::with_capacity(width * height * 4);
    for p in rgba[..width * height * 4].chunks_exact(4) {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let mask = vec![0u8; width.div_ceil(16) * 2 * height];
    let icon = match unsafe { CreateIcon(None, width as i32, height as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) } {
        Ok(icon) => icon,
        Err(e) => {
            eprintln!("[hal-win32] CreateIcon {width}x{height} failed: {e:?}");
            return;
        }
    };
    let wins: Vec<HWND> = APP.with_borrow(|a| a.wins.values().filter(|w| w.own.0 == sid && !w.popup()).map(|w| w.hwnd.get()).collect());
    for hwnd in &wins {
        set_icon(*hwnd, icon);
    }
    if log_enabled() {
        eprintln!("[hal-win32] session {sid}: app icon {width}x{height} on {} windows", wins.len());
    }
    // the windows hold the new icon: the old one can go
    if let Some(old) = APP.with_borrow_mut(|a| a.icons.insert(sid, icon)) {
        unsafe {
            let _ = DestroyIcon(old);
        }
    }
}

/// `WindowVisuals`: bit 0 transparent, bits 8.. the backdrop (None 0, Auto 1, Mica 2, Acrylic 3,
/// Vibrancy 4, Blur 5) as the DWM system backdrop. A see-through window or one with a backdrop
/// extends the DWM frame over the whole client so the backdrop shows where the frame is clear.
fn set_visuals(w: &Win, visuals: u32, _intensity: f32) {
    if w.popup() {
        return;
    }
    let transparent = visuals & 1 != 0;
    let backdrop = (visuals >> 8) & 0xff;
    let hwnd = w.hwnd.get();
    let kind = match backdrop {
        0 => DWMSBT_NONE,
        1 | 2 => DWMSBT_MAINWINDOW,
        3 => DWMSBT_TRANSIENTWINDOW,
        _ => DWMSBT_TABBEDWINDOW,
    };
    let inset = if transparent || backdrop != 0 { -1 } else { 0 };
    if log_enabled() {
        eprintln!("[hal-win32] window {:?}: visuals transparent={transparent} backdrop={backdrop}", w.own);
    }
    let margins = MARGINS { cxLeftWidth: inset, cxRightWidth: inset, cyTopHeight: inset, cyBottomHeight: inset };
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        if let Err(e) = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &kind as *const _ as *const _, std::mem::size_of_val(&kind) as u32) {
            if log_enabled() {
                eprintln!("[hal-win32] system backdrop unavailable (before Windows 11 22H2): {e:?}");
            }
        }
    }
}

fn set_cursor(cursor: u32) {
    let name = match cursor {
        cursor::HIDDEN => {
            unsafe {
                SetCursor(None);
            }
            return;
        }
        cursor::CROSSHAIR => IDC_CROSS,
        cursor::HAND => IDC_HAND,
        cursor::MOVE => IDC_SIZEALL,
        cursor::TEXT => IDC_IBEAM,
        cursor::WAIT => IDC_WAIT,
        cursor::HELP => IDC_HELP,
        cursor::NOT_ALLOWED => IDC_NO,
        cursor::EW_RESIZE | cursor::E_RESIZE | cursor::W_RESIZE | cursor::COL_RESIZE => IDC_SIZEWE,
        cursor::NS_RESIZE | cursor::N_RESIZE | cursor::S_RESIZE | cursor::ROW_RESIZE => IDC_SIZENS,
        cursor::NESW_RESIZE | cursor::NE_RESIZE | cursor::SW_RESIZE => IDC_SIZENESW,
        cursor::NWSE_RESIZE | cursor::NW_RESIZE | cursor::SE_RESIZE => IDC_SIZENWSE,
        _ => IDC_ARROW,
    };
    let c = APP.with_borrow_mut(|a| {
        *a.cursors.entry(name.0 as u32).or_insert_with(|| unsafe { LoadCursorW(None, name).unwrap_or_default() })
    });
    unsafe {
        SetCursor(Some(c));
    }
}

/// The app's answer to a press's drag query: a caption drag hands the press to the system move
/// loop, anything else delivers it as a click.
fn query_answer(w: &Rc<Win>, answer: u32) {
    let Some(press) = w.pending.take() else { return };
    let hwnd = w.hwnd.get();
    if answer == input::drag_query::CAPTION && press.button == 1 {
        unsafe {
            let _ = ReleaseCapture();
            let lp = LPARAM(((press.screen.y as u16 as isize) << 16) | press.screen.x as u16 as isize);
            // the system's move loop (modal on this thread; ops keep arriving through it)
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, Some(WPARAM(HTCAPTION as usize)), Some(lp));
        }
    } else if answer == input::drag_query::SYS_MENU || (answer == input::drag_query::CAPTION && press.button == 2) {
        unsafe {
            let menu = GetSystemMenu(hwnd, false);
            let cmd = TrackPopupMenuEx(menu, TPM_RETURNCMD.0 as u32, press.screen.x, press.screen.y, hwnd, None);
            if cmd.0 != 0 {
                let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(cmd.0 as usize), LPARAM(0));
            }
        }
    } else {
        w.push(|q, id| q.mouse_down(id, press.x, press.y, press.button, press.mods));
    }
}

fn mouse_button(w: &Rc<Win>, button: u32, down: bool, lparam: LPARAM) {
    let (px, py) = lparam_point(lparam);
    let dpi = w.dpi.get();
    let hwnd = w.hwnd.get();
    // a held pointer's buttons act where the app believes it is
    let (x, y) = rawinput::locked_pos(hwnd).filter(|_| rawinput::held(hwnd)).unwrap_or((px as f64 / dpi, py as f64 / dpi));
    let mods = modifiers();
    w.alt_tap.set(false);
    if down {
        // losing the capture also loses the button-ups that balance the count (an OLE drag
        // takes the capture and consumes the release)
        if unsafe { GetCapture() } != hwnd {
            w.buttons.set(0);
        }
        if w.buttons.get() == 0 {
            unsafe {
                SetCapture(hwnd);
            }
        }
        w.buttons.set(w.buttons.get() | button);
        if !w.popup() {
            dismiss_popups(w.own.0);
        }
        if !w.popup() && matches!(button, 1 | 2) && w.pending.get().is_none() {
            let mut screen = POINT { x: px, y: py };
            unsafe {
                let _ = ClientToScreen(hwnd, &mut screen);
            }
            w.pending.set(Some(Press { button, x, y, mods, screen }));
            w.push(|q, id| q.drag_query(id, x, y));
        } else {
            w.push(|q, id| q.mouse_down(id, x, y, button, mods));
        }
    } else {
        w.buttons.set(w.buttons.get() & !button);
        if w.buttons.get() == 0 {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        // a release before the app answered the query: the press was a click
        if let Some(p) = w.pending.take() {
            w.push(|q, id| q.mouse_down(id, p.x, p.y, p.button, p.mods));
        }
        w.push(|q, id| q.mouse_up(id, x, y, button, mods));
        if button == 1 {
            rawinput::button_up(hwnd);
        }
    }
}

/// The session's popups are dismissed (a press outside them, a frame drag of a window).
fn dismiss_popups(sid: u32) {
    let popups: Vec<Rc<Win>> = APP.with_borrow(|a| a.wins.values().filter(|w| w.own.0 == sid && w.popup()).cloned().collect());
    for p in popups {
        p.push(|q, id| q.popup_dismissed(id));
    }
}

/// The app lost activation: every popup is dismissed, once (every top-level window hears it).
fn app_activation(active: bool) {
    if active {
        APP_ACTIVE.set(true);
    } else if APP_ACTIVE.replace(false) {
        let popups: Vec<Rc<Win>> = APP.with_borrow(|a| a.wins.values().filter(|w| w.popup()).cloned().collect());
        for p in popups {
            p.push(|q, id| q.popup_dismissed(id));
        }
    }
}

fn key_bit(vk: u16) -> (usize, u64) {
    ((vk as usize >> 6) & 3, 1u64 << (vk & 63))
}

fn set_forwarded(w: &Win, vk: u16, on: bool) -> bool {
    let (i, bit) = key_bit(vk);
    let mut f = w.forwarded.get();
    let was = f[i] & bit != 0;
    if on {
        f[i] |= bit;
    } else {
        f[i] &= !bit;
    }
    w.forwarded.set(f);
    was
}

/// The maximized rectangle: the monitor's work area with the frame overhanging it (the frame
/// inset of WM_NCCALCSIZE takes it back), or the whole monitor without a frame for a window
/// that is chromeless when maximized. Without this a frameless (WS_POPUP) window would cover
/// the taskbar.
unsafe fn min_max_info(w: &Win, info: &mut MINMAXINFO) {
    let hwnd = w.hwnd.get();
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    if !GetMonitorInfoW(monitor, &mut mi).as_bool() {
        return;
    }
    let (m, work) = (mi.rcMonitor, mi.rcWork);
    if w.covers_monitor() {
        info.ptMaxPosition = POINT { x: 0, y: 0 };
        info.ptMaxSize = POINT { x: m.right - m.left, y: m.bottom - m.top };
    } else {
        let mut frame = RECT::default();
        let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
        let _ = AdjustWindowRectExForDpi(&mut frame, WINDOW_STYLE(style.0 & !WS_CAPTION.0 | WS_BORDER.0 | WS_THICKFRAME.0), false, WINDOW_EX_STYLE(0), GetDpiForWindow(hwnd));
        info.ptMaxPosition = POINT { x: work.left - m.left + frame.left, y: work.top - m.top + frame.top };
        info.ptMaxSize = POINT { x: work.right - work.left + frame.right - frame.left, y: work.bottom - work.top + frame.bottom - frame.top };
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lparam.0 as *const CREATESTRUCTW);
        let raw = cs.lpCreateParams as *const Win;
        if !raw.is_null() {
            (*raw).hwnd.set(hwnd);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
            // the map holds the same Rc from here on
            Rc::increment_strong_count(raw);
            let rc = Rc::from_raw(raw);
            APP.with_borrow_mut(|a| a.wins.insert(rc.own, rc));
            let _ = EnableNonClientDpiScaling(hwnd);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let Some(w) = win_of(hwnd) else { return DefWindowProcW(hwnd, msg, wparam, lparam) };
    match msg {
        WM_NCCALCSIZE if wparam.0 != 0 && !w.popup() => {
            // the client covers the whole window; a maximized window keeps its frame's inset so
            // its client matches the work area (unless it is chromeless when maximized)
            if w.maximized() && !w.covers_monitor() {
                let params = &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS);
                let dpi = GetDpiForWindow(hwnd);
                let mut frame = RECT::default();
                let style = WINDOW_STYLE(GetWindowLongPtrW(hwnd, GWL_STYLE) as u32);
                let _ = AdjustWindowRectExForDpi(&mut frame, WINDOW_STYLE(style.0 & !WS_CAPTION.0 | WS_BORDER.0 | WS_THICKFRAME.0), false, WINDOW_EX_STYLE(0), dpi);
                let r = &mut params.rgrc[0];
                r.left -= frame.left;
                r.top -= frame.top;
                r.right -= frame.right;
                r.bottom -= frame.bottom;
            }
            return LRESULT(0);
        }
        WM_NCHITTEST if !w.popup() => {
            if w.maximized() {
                return LRESULT(HTCLIENT as isize);
            }
            let (sx, sy) = lparam_point(lparam);
            let mut r = RECT::default();
            let _ = GetWindowRect(hwnd, &mut r);
            let edge = (RESIZE_EDGE * w.dpi.get()) as i32;
            let (l, rt, t, b) = (sx < r.left + edge, sx >= r.right - edge, sy < r.top + edge, sy >= r.bottom - edge);
            let hit = match (l, rt, t, b) {
                (true, _, true, _) => HTTOPLEFT,
                (true, _, _, true) => HTBOTTOMLEFT,
                (_, true, true, _) => HTTOPRIGHT,
                (_, true, _, true) => HTBOTTOMRIGHT,
                (true, _, _, _) => HTLEFT,
                (_, true, _, _) => HTRIGHT,
                (_, _, true, _) => HTTOP,
                (_, _, _, true) => HTBOTTOM,
                _ => HTCLIENT,
            };
            return LRESULT(hit as isize);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_GETMINMAXINFO => {
            if !w.popup() {
                min_max_info(&w, &mut *(lparam.0 as *mut MINMAXINFO));
                return LRESULT(0);
            }
        }
        WM_SETCURSOR => {
            if rawinput::held(hwnd) {
                SetCursor(None);
                return LRESULT(1);
            }
            if (lparam.0 & 0xffff) as u32 == HTCLIENT {
                set_cursor(w.cursor.get());
                return LRESULT(1);
            }
        }
        WM_SIZE | WM_MOVE => publish_geom(&w),
        WM_DPICHANGED => {
            let r = &*(lparam.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            let dpi = dpi_of(hwnd);
            w.dpi.set(dpi);
            w.push(|q, id| q.display(id, dpi as f32, 0.0));
            publish_geom(&w);
            refresh_screens(Shared::global());
            return LRESULT(0);
        }
        WM_DISPLAYCHANGE => refresh_screens(Shared::global()),
        WM_SETTINGCHANGE => {
            // the user switched between the light and the dark app theme
            let what = lparam.0 as *const u16;
            if !what.is_null() {
                let name: Vec<u16> = (0..64).map(|i| *what.add(i)).take_while(|c| *c != 0).collect();
                if String::from_utf16_lossy(&name) == "ImmersiveColorSet" && !w.popup() {
                    apply_dark(hwnd);
                }
            }
        }
        WM_ENTERSIZEMOVE => {
            if !w.popup() {
                dismiss_popups(w.own.0);
            }
            w.push(|q, id| q.resize_loop(id, true));
        }
        WM_NCLBUTTONDOWN | WM_NCRBUTTONDOWN => {
            if !w.popup() {
                dismiss_popups(w.own.0);
            }
        }
        WM_EXITSIZEMOVE => {
            w.push(|q, id| q.resize_loop(id, false));
            publish_geom(&w);
        }
        WM_SETFOCUS => {
            ime::focus(&w);
            w.push(|q, id| q.focus(id, true));
        }
        WM_KILLFOCUS => {
            w.alt_tap.set(false);
            w.forwarded.set([0; 4]);
            w.push(|q, id| q.focus(id, false));
        }
        WM_ACTIVATE => rawinput::activated(hwnd, (wparam.0 & 0xffff) as u32 != WA_INACTIVE),
        WM_ACTIVATEAPP => app_activation(wparam.0 != 0),
        WM_INPUT => {
            rawinput::input(&w, lparam, modifiers());
            // DefWindowProc cleans the raw input buffer up
        }
        WM_COMMAND => {
            // a menu item (high word 0, no control)
            if (wparam.0 >> 16) & 0xffff == 0 && lparam.0 == 0 {
                menu::command(&w, (wparam.0 & 0xffff) as u16);
                return LRESULT(0);
            }
        }
        WM_IME_STARTCOMPOSITION | WM_IME_COMPOSITION | WM_IME_ENDCOMPOSITION => {
            if ime::message(&w, msg, lparam.0).is_some() {
                return LRESULT(0);
            }
        }
        WM_MOUSEACTIVATE if w.popup() => return LRESULT(MA_NOACTIVATE as isize),
        WM_CLOSE => {
            w.push(|q, id| q.close_requested(id));
            return LRESULT(0);
        }
        WM_DESTROY => {
            if !w.popup() {
                identity::window_destroyed(hwnd);
            }
            rawinput::destroyed(hwnd);
            drag::revoke(&w);
            let s = Shared::global();
            s.unregister(w.own);
            APP.with_borrow_mut(|a| a.wins.remove(&w.own));
            if !w.silent.get() {
                w.push(|q, id| q.closed(id));
            }
        }
        WM_NCDESTROY => {
            let raw = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Win;
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            if !raw.is_null() {
                drop(Rc::from_raw(raw));
            }
        }
        WM_MOUSEMOVE => {
            if !w.tracking.replace(true) {
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            // a held pointer moves only in raw input (WM_INPUT)
            if rawinput::held(hwnd) {
                return LRESULT(0);
            }
            let (px, py) = lparam_point(lparam);
            let dpi = w.dpi.get();
            let (x, y) = (px as f64 / dpi, py as f64 / dpi);
            w.last_pos.set((x, y));
            let mods = modifiers();
            w.push(|q, id| q.mouse_move(id, x, y, 0.0, 0.0, mods));
        }
        WM_MOUSELEAVE => {
            w.tracking.set(false);
            // the leave ends every hover: the cursor they asked for must not return on re-entry
            w.cursor.set(cursor::DEFAULT);
            let (x, y) = w.last_pos.get();
            w.push(|q, id| q.mouse_leave(id, x, y));
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xffff) as u16 as i16 as f64;
            let (sx, sy) = lparam_point(lparam);
            let mut pt = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut pt);
            let dpi = w.dpi.get();
            let (x, y) = (pt.x as f64 / dpi, pt.y as f64 / dpi);
            let mods = modifiers();
            let (dx, dy) = if msg == WM_MOUSEHWHEEL { (delta, 0.0) } else { (0.0, -delta) };
            w.push(|q, id| q.scroll(id, x, y, dx, dy, mods, false, input::scroll::NONE));
        }
        WM_LBUTTONDOWN => mouse_button(&w, 1, true, lparam),
        WM_LBUTTONUP => mouse_button(&w, 1, false, lparam),
        WM_RBUTTONDOWN => mouse_button(&w, 2, true, lparam),
        WM_RBUTTONUP => mouse_button(&w, 2, false, lparam),
        WM_MBUTTONDOWN => mouse_button(&w, 4, true, lparam),
        WM_MBUTTONUP => mouse_button(&w, 4, false, lparam),
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let x = ((wparam.0 >> 16) & 0xffff) as u16;
            let button = if x == XBUTTON1 { 8 } else { 16 };
            mouse_button(&w, button, msg == WM_XBUTTONDOWN, lparam);
            return LRESULT(1);
        }
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            let vk = wparam.0 as u16;
            // a key the IME takes (VK_PROCESSKEY): its press and release are the IME's
            if vk == VK_PROCESSKEY.0 {
                return LRESULT(0);
            }
            let key = keys::key_code(vk, lparam.0 & (1 << 24) != 0);
            let mods = modifiers();
            if w.popup() && key == makepad_key_code::KeyCode::Escape {
                w.push(|q, id| q.popup_dismissed(id));
                return LRESULT(0);
            }
            let repeat = lparam.0 & 0x4000_0000 != 0;
            w.alt_tap.set(vk == VK_MENU.0 && !repeat && mods & !input::MOD_ALT == 0);
            // a menu's key equivalent is the menu command, not a key
            if !repeat && menu::key_equivalent(&w, key, mods) {
                return LRESULT(0);
            }
            set_forwarded(&w, vk, true);
            w.push(|q, id| q.key_down(id, key.to_index(), repeat, mods));
            // the paste chord brings the clipboard's text (as AppKit's shell does)
            let paste = (mods & (input::MOD_CONTROL | input::MOD_LOGO) != 0 && key == makepad_key_code::KeyCode::KeyV)
                || (mods & input::MOD_SHIFT != 0 && key == makepad_key_code::KeyCode::Insert);
            if paste {
                if let Some(text) = clipboard::read_text() {
                    w.push(|q, id| q.text(id, input::text::PASTE, text.as_bytes()));
                }
            }
            // Alt+F4 stays the system's (it sends WM_CLOSE); other Alt chords open no menu
            if !(msg == WM_SYSKEYDOWN && key == makepad_key_code::KeyCode::F4) {
                return LRESULT(0);
            }
        }
        WM_KEYUP | WM_SYSKEYUP => {
            let vk = wparam.0 as u16;
            let key = keys::key_code(vk, lparam.0 & (1 << 24) != 0);
            let mods = modifiers();
            // a release pairs with a press the app heard (not one the IME or a menu took)
            if set_forwarded(&w, vk, false) {
                w.push(|q, id| q.key_up(id, key.to_index(), mods));
            }
            if vk == VK_MENU.0 && w.alt_tap.replace(false) && menu::has_menu(&w) {
                menu::show(&w);
            }
            if msg == WM_SYSKEYUP {
                return LRESULT(0);
            }
        }
        WM_CHAR | WM_SYSCHAR => {
            let unit = wparam.0 as u16;
            // UTF-16 assembly: a high surrogate waits for its low half
            let text = if (0xD800..0xDC00).contains(&unit) {
                w.high_surrogate.set(unit);
                None
            } else if (0xDC00..0xE000).contains(&unit) {
                let high = w.high_surrogate.replace(0);
                if high == 0 { None } else { String::from_utf16(&[high, unit]).ok() }
            } else {
                w.high_surrogate.set(0);
                String::from_utf16(&[unit]).ok()
            };
            if let Some(text) = text.filter(|t| t.chars().next().is_some_and(|c| c >= ' ' && c != '\u{7f}')) {
                // chords with Control/Alt type nothing (the app reads them as keys)
                let mods = modifiers();
                if mods & input::MOD_CONTROL == 0 || mods & input::MOD_ALT != 0 {
                    w.push(|q, id| q.text(id, input::text::TYPED, text.as_bytes()));
                }
            }
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
