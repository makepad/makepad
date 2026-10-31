//! The system tray (`TraySet` / `TrayRemove`) as notification-area icons (Shell_NotifyIconW,
//! NOTIFYICON_VERSION_4). The icons belong to one hidden tool window of the shell's thread
//! (made with the first tray) that takes their callback message: a left click or Enter answers
//! as `TrayClick`, the context menu (right click, Shift+F10) opens the tray's menu with
//! TrackPopupMenuEx and the chosen item answers as `TrayMenu`. The app's glyph is drawn white on a
//! dark taskbar and black on a light one (SystemUsesLightTheme), again when the theme changes;
//! when Explorer restarts (`TaskbarCreated`) the icons are added again. A `TrayClick` carries the
//! icon's rect (Shell_NotifyIconGetRect) and its monitor's work area in points; `WindowAtTray`
//! (`window_at`) puts a dropdown window next to the icon, inside that work area: above a bottom
//! taskbar, below a top one, beside a side one.
//!
//! `MAKEPAD_TRAY_SELFTEST=1`: after a tray is first added the shell logs whether the shell took
//! it and clicks it itself (posts `NIN_SELECT` to the icon's callback), so a test sees the path to
//! the `TrayClick` event without a user.

use super::{log_enabled, Shared};
use crate::types::*;
use crate::Id;
use makepad_windows_sys::core::{PCWSTR, PWSTR};
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, HMONITOR, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use makepad_windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use makepad_windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use makepad_windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use makepad_windows_sys::Win32::UI::Shell::*;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::*;
use std::cell::{Cell, RefCell};

/// The icons' callback message.
const WM_TRAY: u32 = WM_APP + 2;
const CLASS: &str = "MakepadHalTray";
/// the space between the taskbar and a dropdown, points
const DROPDOWN_GAP: f64 = 8.0;

struct Tray {
    sid: u32,
    id: u64,
    /// the icon's id with the shell (`NOTIFYICONDATAW::uID`)
    uid: u32,
    w: u32,
    h: u32,
    alpha: Vec<u8>,
    tooltip: String,
    items: Vec<(MenuItem, String)>,
    icon: Option<HICON>,
}

thread_local! {
    static TRAYS: RefCell<Vec<Tray>> = const { RefCell::new(Vec::new()) };
    static HWND_TRAY: Cell<usize> = const { Cell::new(0) };
    static NEXT_UID: Cell<u32> = const { Cell::new(1) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
    /// the icons' glyph colour: the taskbar is light (black glyphs)
    static LIGHT: Cell<bool> = const { Cell::new(false) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The taskbar theme (Settings > Personalization > Colors: SystemUsesLightTheme; dark when unset).
fn taskbar_light() -> bool {
    let key = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let value = wide("SystemUsesLightTheme");
    let mut data: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, Some(PCWSTR(key.as_ptr())), Some(PCWSTR(value.as_ptr())), RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size))
    };
    r.0 == 0 && data != 0
}

/// The tray window, made on first use.
fn window() -> Option<HWND> {
    let made = HWND_TRAY.get();
    if made != 0 {
        return Some(HWND(made as _));
    }
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(tray_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            eprintln!("[hal-win32] RegisterClassExW(tray) failed");
            return None;
        }
        // a real (never shown) top-level window: TaskbarCreated is broadcast, and the context
        // menu needs a window that can be made the foreground one
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, Some(PCWSTR(class.as_ptr())), None, WS_POPUP, 0, 0, 0, 0, None, None, Some(HINSTANCE(instance.0)), None);
        if hwnd.0.is_null() {
            eprintln!("[hal-win32] CreateWindowExW(tray) failed");
            return None;
        }
        TASKBAR_CREATED.set(RegisterWindowMessageW(PCWSTR(wide("TaskbarCreated").as_ptr())));
        LIGHT.set(taskbar_light());
        HWND_TRAY.set(hwnd.0 as usize);
        Some(hwnd)
    }
}

/// The glyph as an icon, white or black with the app's alpha.
fn make_icon(w: u32, h: u32, alpha: &[u8], light: bool) -> Option<HICON> {
    let (width, height) = (w as usize, h as usize);
    if width == 0 || height == 0 || alpha.len() < width * height {
        return None;
    }
    let c = if light { 0 } else { 255 };
    let mut bgra = Vec::with_capacity(width * height * 4);
    for a in &alpha[..width * height] {
        bgra.extend_from_slice(&[c, c, c, *a]);
    }
    // a 32-bit colour plane with alpha and an all-zero 1-bit mask, rows padded to 16 bits
    let mask = vec![0u8; width.div_ceil(16) * 2 * height];
    match unsafe { CreateIcon(None, width as i32, height as i32, 1, 32, mask.as_ptr(), bgra.as_ptr()) } {
        Ok(icon) => Some(icon),
        Err(e) => {
            eprintln!("[hal-win32] tray CreateIcon {width}x{height} failed: {e:?}");
            None
        }
    }
}

fn notify_data(hwnd: HWND, t: &Tray) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: t.uid, ..Default::default() };
    data.uFlags = NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
    data.uCallbackMessage = WM_TRAY;
    if let Some(icon) = t.icon {
        data.uFlags = data.uFlags | NIF_ICON;
        data.hIcon = icon;
    }
    for (dst, src) in data.szTip.iter_mut().zip(t.tooltip.encode_utf16().take(127)) {
        *dst = src;
    }
    data
}

/// Adds the icon to the notification area (again, after Explorer restarted).
fn add(hwnd: HWND, t: &Tray) -> bool {
    let mut data = notify_data(hwnd, t);
    unsafe {
        if !Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            return false;
        }
        data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
    }
    true
}

fn destroy_icon(icon: Option<HICON>) {
    if let Some(icon) = icon {
        unsafe {
            let _ = DestroyIcon(icon);
        }
    }
}

/// `TraySet` of session `sid`: creates the icon or updates it in place.
pub(super) fn set(sid: u32, id: u64, w: u32, h: u32, alpha: Vec<u8>, tooltip: String, items: Vec<(MenuItem, String)>) {
    let Some(hwnd) = window() else { return };
    let icon = make_icon(w, h, &alpha, LIGHT.get());
    let existing = TRAYS.with_borrow(|t| t.iter().position(|t| t.sid == sid && t.id == id));
    match existing {
        Some(index) => {
            let old = TRAYS.with_borrow_mut(|trays| {
                let t = &mut trays[index];
                t.w = w;
                t.h = h;
                t.alpha = alpha;
                t.tooltip = tooltip;
                t.items = items;
                let old = std::mem::replace(&mut t.icon, icon);
                let data = notify_data(hwnd, t);
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
                }
                old
            });
            destroy_icon(old);
        }
        None => {
            let uid = NEXT_UID.get();
            NEXT_UID.set(uid + 1);
            let t = Tray { sid, id, uid, w, h, alpha, tooltip, items, icon };
            let added = add(hwnd, &t);
            if log_enabled() || !added {
                eprintln!("[hal-win32] session {sid}: tray {id:#x} added={added}");
            }
            let count = t.items.len();
            TRAYS.with_borrow_mut(|trays| trays.push(t));
            if std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some() {
                // the shell's own click on the new icon, through the icon's callback message
                eprintln!("tray selftest: notify icon added={added}, {count} menu items; clicking it");
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_TRAY, WPARAM(0), LPARAM(((uid << 16) | NIN_SELECT) as isize));
                }
            }
        }
    }
}

fn delete(t: Tray) {
    let hwnd = HWND(HWND_TRAY.get() as _);
    let data = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: t.uid, ..Default::default() };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
    destroy_icon(t.icon);
}

fn remove_where(pick: impl Fn(&Tray) -> bool) {
    let gone: Vec<Tray> = TRAYS.with_borrow_mut(|t| {
        let (gone, keep) = std::mem::take(t).into_iter().partition(|t| pick(t));
        *t = keep;
        gone
    });
    gone.into_iter().for_each(delete);
}

pub(super) fn remove(sid: u32, id: u64) {
    remove_where(|t| t.sid == sid && t.id == id);
}

pub(super) fn session_closed(sid: u32) {
    remove_where(|t| t.sid == sid);
}

/// The tray's menu at `at` (screen pixels); the chosen item answers as `TrayMenu`.
fn show_menu(hwnd: HWND, uid: u32, at: POINT) {
    let Some((sid, tray, items)) = TRAYS.with_borrow(|t| t.iter().find(|t| t.uid == uid).map(|t| (t.sid, t.id, t.items.clone()))) else { return };
    let Ok(menu) = (unsafe { CreatePopupMenu() }) else { return };
    // WM_COMMAND-style ids: index + 1 (0 is "nothing chosen")
    for (index, (item, label)) in items.iter().enumerate() {
        let mut info = MENUITEMINFOW { cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32, ..Default::default() };
        let mut text = wide(label);
        if item.kind & 0xff == menu_kind::ITEM {
            info.fMask = MIIM_ID | MIIM_STRING | MIIM_STATE | MIIM_FTYPE;
            info.fType = MFT_STRING;
            info.wID = index as u32 + 1;
            if item.kind & menu_kind::DISABLED != 0 {
                info.fState = MFS_DISABLED;
            }
            if item.kind & menu_kind::CHECKABLE != 0 && item.kind & menu_kind::CHECKED != 0 {
                info.fState = MENU_ITEM_STATE(info.fState.0 | MFS_CHECKED.0);
            }
            info.dwTypeData = PWSTR(text.as_mut_ptr());
        } else {
            info.fMask = MIIM_FTYPE;
            info.fType = MFT_SEPARATOR;
        }
        unsafe {
            let _ = InsertMenuItemW(menu, u32::MAX, true, &info);
        }
    }
    let chosen = unsafe {
        // the menu closes when the user clicks elsewhere only if its window is the foreground one
        let _ = SetForegroundWindow(hwnd);
        let flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN;
        let chosen = TrackPopupMenuEx(menu, flags.0, at.x, at.y, hwnd, None).0;
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        chosen
    };
    if chosen > 0 {
        if let Some((item, _)) = items.get(chosen as usize - 1) {
            let cmd = item.cmd;
            Shared::global().push((sid, Id::NONE), |q, _| q.tray_menu(tray, cmd));
        }
    }
}

fn click(uid: u32) {
    if let Some((sid, tray)) = TRAYS.with_borrow(|t| t.iter().find(|t| t.uid == uid).map(|t| (t.sid, t.id))) {
        let (anchor, screen) = match place(uid) {
            Some(p) => (p.points(p.icon), p.points(p.work)),
            None => ([0.0; 4], [0.0; 4]),
        };
        Shared::global().push((sid, Id::NONE), |q, _| q.tray_click(tray, anchor[0] as f64, anchor[1] as f64, anchor[2] as f64, anchor[3] as f64, screen));
    }
}

/// Where an icon is: its rect and its monitor's work area (screen pixels) and that monitor's scale.
struct Place {
    icon: RECT,
    work: RECT,
    scale: f64,
}

impl Place {
    /// x, y, w, h in points (pixels / the monitor's scale)
    fn points(&self, r: RECT) -> [f32; 4] {
        let s = self.scale;
        [(r.left as f64 / s) as f32, (r.top as f64 / s) as f32, ((r.right - r.left) as f64 / s) as f32, ((r.bottom - r.top) as f64 / s) as f32]
    }
}

fn place(uid: u32) -> Option<Place> {
    let id = NOTIFYICONIDENTIFIER { cbSize: std::mem::size_of::<NOTIFYICONIDENTIFIER>() as u32, hWnd: HWND(HWND_TRAY.get() as _), uID: uid, ..Default::default() };
    unsafe {
        let icon = Shell_NotifyIconGetRect(&id).ok()?;
        let monitor: HMONITOR = MonitorFromRect(&icon, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Some(Place { icon, work: info.rcWork, scale: dx.max(1) as f64 / 96.0 })
    }
}

/// `WindowAtTray`: dropdown `hwnd` becomes `w` x `h` points (the icon's monitor's scale), fitted
/// into that monitor's work area next to tray `tray`'s icon, topmost and in the foreground.
pub(super) fn window_at(sid: u32, hwnd: HWND, tray: u64, w: f32, h: f32) {
    let uid = TRAYS.with_borrow(|t| t.iter().find(|t| t.sid == sid && t.id == tray).map(|t| t.uid));
    let Some(p) = uid.and_then(place) else {
        // no icon to go next to: shown where it is
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    };
    let (icon, work) = (p.icon, p.work);
    let gap = (DROPDOWN_GAP * p.scale).round() as i32;
    let width = ((w as f64 * p.scale).round() as i32).min(work.right - work.left - 2 * gap).max(1);
    let height = ((h as f64 * p.scale).round() as i32).min(work.bottom - work.top - 2 * gap).max(1);
    let clamp = |v: i32, lo: i32, hi: i32| v.max(lo).min(hi.max(lo));
    let (cx, cy) = ((icon.left + icon.right) / 2, (icon.top + icon.bottom) / 2);
    let (x, y) = if icon.top >= work.bottom {
        // a bottom taskbar: above it, centred on the icon
        (clamp(cx - width / 2, work.left + gap, work.right - width - gap), work.bottom - gap - height)
    } else if icon.bottom <= work.top {
        (clamp(cx - width / 2, work.left + gap, work.right - width - gap), work.top + gap)
    } else if icon.left >= work.right {
        (work.right - gap - width, clamp(cy - height / 2, work.top + gap, work.bottom - height - gap))
    } else if icon.right <= work.left {
        (work.left + gap, clamp(cy - height / 2, work.top + gap, work.bottom - height - gap))
    } else {
        // the overflow flyout (inside the work area): above the icon
        (clamp(cx - width / 2, work.left + gap, work.right - width - gap), clamp(icon.top - gap - height, work.top + gap, work.bottom - height - gap))
    };
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, width, height, SWP_SHOWWINDOW);
        let _ = SetForegroundWindow(hwnd);
    }
    if std::env::var_os("MAKEPAD_TRAY_SELFTEST").is_some() {
        eprintln!(
            "tray selftest: dropdown {width}x{height} px at ({x}, {y}), icon {}x{} px at ({}, {}), work area ({}, {})-({}, {}), scale {}",
            icon.right - icon.left, icon.bottom - icon.top, icon.left, icon.top, work.left, work.top, work.right, work.bottom, p.scale
        );
    }
}

/// The taskbar theme changed: the glyphs are drawn again in the other colour.
fn theme_changed(hwnd: HWND) {
    let light = taskbar_light();
    if LIGHT.replace(light) == light {
        return;
    }
    let old: Vec<Option<HICON>> = TRAYS.with_borrow_mut(|trays| {
        trays
            .iter_mut()
            .map(|t| {
                let old = std::mem::replace(&mut t.icon, make_icon(t.w, t.h, &t.alpha, light));
                let data = notify_data(hwnd, t);
                unsafe {
                    let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
                }
                old
            })
            .collect()
    });
    old.into_iter().for_each(destroy_icon);
}

unsafe extern "system" fn tray_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_TRAY {
        // NOTIFYICON_VERSION_4: the event in LOWORD(lparam), the icon in HIWORD(lparam), the
        // anchor point (screen pixels) in wparam
        let event = (lparam.0 as u32) & 0xffff;
        let uid = ((lparam.0 as u32) >> 16) & 0xffff;
        let at = POINT { x: (wparam.0 & 0xffff) as i16 as i32, y: ((wparam.0 >> 16) & 0xffff) as i16 as i32 };
        match event {
            NIN_SELECT => click(uid),
            e if e == NIN_SELECT | NINF_KEY => click(uid),
            WM_CONTEXTMENU => show_menu(hwnd, uid, at),
            _ => {}
        }
        return LRESULT(0);
    }
    if msg != 0 && msg == TASKBAR_CREATED.get() {
        TRAYS.with_borrow(|trays| {
            for t in trays {
                add(hwnd, t);
            }
        });
        return LRESULT(0);
    }
    if msg == WM_SETTINGCHANGE {
        theme_changed(hwnd);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
