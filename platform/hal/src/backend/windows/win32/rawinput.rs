//! Raw mouse motion (WM_INPUT) and the pointer lock (`PointerLock`), a Windows addition (the old
//! backend had neither). LOCK (a game's look mode): the cursor hides and is held mid-window by a
//! one-pixel ClipCursor, `MouseMove` keeps the position where the lock began and carries the raw,
//! unaccelerated motion in `dx` / `dy`. PIN (value scrubbing): the cursor is held where it is and
//! the motion integrates into a virtual position; UNPIN or the button release puts the cursor back
//! there. REPIN re-asserts the clip and the position (Windows drops a clip on activation changes).
//!
//! Only the active window takes the user's pointer: a lock asked by an unfocused (bridge-driven,
//! hidden) instance stays logical until the window is activated. Focus loss, the window closing
//! and the session ending release the pointer altogether, restoring the clip region and the cursor.

use super::window::Win;
use makepad_windows_sys::Win32::Foundation::*;
use makepad_windows_sys::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use makepad_windows_sys::Win32::UI::Input::*;
use makepad_windows_sys::Win32::UI::WindowsAndMessaging::{ClipCursor, GetClipCursor, GetCursorPos, SetCursorPos, ShowCursor, GetClientRect};
use crate::types::pointer_lock;
use std::cell::{Cell, RefCell};

struct Lock {
    hwnd: HWND,
    /// scrub pin (virtual position integrates) rather than a game lock
    pin: bool,
    /// the physical effects are on (hidden, clipped, raw input registered)
    applied: bool,
    /// where the cursor is held, screen pixels
    at: POINT,
    /// the clip region before the lock
    saved_clip: Option<RECT>,
    /// pin: where the cursor goes back to
    restore: Option<POINT>,
    /// the position the app hears, window points
    virtual_pos: (f64, f64),
}

thread_local! {
    static LOCK: RefCell<Option<Lock>> = const { RefCell::new(None) };
    /// the active window of the app (0: none)
    static ACTIVE: Cell<usize> = const { Cell::new(0) };
}

fn register(hwnd: Option<HWND>) -> bool {
    let dev = RAWINPUTDEVICE {
        usUsagePage: 0x01, // generic desktop
        usUsage: 0x02,     // mouse
        dwFlags: if hwnd.is_some() { RAWINPUTDEVICE_FLAGS(0) } else { RIDEV_REMOVE },
        hwndTarget: hwnd.unwrap_or(HWND(std::ptr::null_mut())),
    };
    let r = unsafe { RegisterRawInputDevices(&[dev], std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if let Err(e) = &r {
        eprintln!("[hal-win32] raw mouse registration failed: {e:?}");
    }
    r.is_ok()
}

fn clip_to(at: POINT) {
    let r = RECT { left: at.x, top: at.y, right: at.x + 1, bottom: at.y + 1 };
    unsafe {
        let _ = SetCursorPos(at.x, at.y);
        let _ = ClipCursor(Some(&r));
    }
}

/// Turns the physical effects on or off.
fn apply(l: &mut Lock, on: bool) {
    if l.applied == on {
        return;
    }
    if on && ACTIVE.get() != l.hwnd.0 as usize {
        return;
    }
    l.applied = on;
    unsafe {
        if on {
            let mut saved = RECT::default();
            l.saved_clip = GetClipCursor(&mut saved).ok().map(|_| saved);
            if !l.pin {
                // mid-window: never an edge pixel where clicks fall outside
                let mut c = RECT::default();
                let _ = GetClientRect(l.hwnd, &mut c);
                let mut p = POINT { x: (c.left + c.right) / 2, y: (c.top + c.bottom) / 2 };
                let _ = ClientToScreen(l.hwnd, &mut p);
                l.at = p;
            }
            register(Some(l.hwnd));
            ShowCursor(false);
            clip_to(l.at);
        } else {
            let _ = ClipCursor(l.saved_clip.as_ref().map(|r| r as *const RECT));
            register(None);
            ShowCursor(true);
        }
    }
}

fn release() {
    let Some(mut l) = LOCK.take() else { return };
    apply(&mut l, false);
    if let Some(p) = l.restore {
        unsafe {
            let _ = SetCursorPos(p.x, p.y);
        }
    }
}

fn cursor_point(hwnd: HWND, dpi: f64) -> (POINT, (f64, f64)) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    let mut c = p;
    unsafe {
        let _ = ScreenToClient(hwnd, &mut c);
    }
    (p, (c.x as f64 / dpi, c.y as f64 / dpi))
}

/// `PointerLock`.
pub(super) fn command(w: &Win, mode: u32) {
    let hwnd = w.hwnd.get();
    match mode {
        pointer_lock::LOCK => {
            if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| !l.pin && l.hwnd == hwnd)) {
                return;
            }
            release();
            let (at, pos) = cursor_point(hwnd, w.dpi.get());
            let mut l = Lock { hwnd, pin: false, applied: false, at, saved_clip: None, restore: None, virtual_pos: pos };
            apply(&mut l, true);
            LOCK.set(Some(l));
        }
        pointer_lock::PIN => {
            // a game lock already holds the pointer
            if LOCK.with_borrow(|l| l.is_some()) {
                return;
            }
            let (at, pos) = cursor_point(hwnd, w.dpi.get());
            let mut l = Lock { hwnd, pin: true, applied: false, at, saved_clip: None, restore: Some(at), virtual_pos: pos };
            apply(&mut l, true);
            LOCK.set(Some(l));
        }
        pointer_lock::UNPIN => {
            if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| l.pin)) {
                release();
            }
        }
        pointer_lock::REPIN => LOCK.with_borrow_mut(|l| {
            if let Some(l) = l.as_mut().filter(|l| l.applied && l.hwnd == hwnd) {
                clip_to(l.at);
            }
        }),
        _ => {
            if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| !l.pin)) {
                release();
            }
        }
    }
}

/// Whether the window's pointer is held (its WM_MOUSEMOVEs are the held cursor's, not motion).
pub(super) fn held(hwnd: HWND) -> bool {
    LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| l.applied && l.hwnd == hwnd))
}

/// The position the app hears while the pointer is locked.
pub(super) fn locked_pos(hwnd: HWND) -> Option<(f64, f64)> {
    LOCK.with_borrow(|l| l.as_ref().filter(|l| l.hwnd == hwnd).map(|l| l.virtual_pos))
}

/// WM_INPUT: raw relative motion becomes a `MouseMove` with `dx` / `dy`.
pub(super) fn input(w: &Win, lparam: LPARAM, mods: u32) {
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let n = unsafe { GetRawInputData(HRAWINPUT(lparam.0 as _), RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32) };
    if n == u32::MAX || n == 0 || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let mouse = unsafe { raw.data.mouse };
    if mouse.usFlags.0 & MOUSE_MOVE_ABSOLUTE.0 != 0 || (mouse.lLastX == 0 && mouse.lLastY == 0) {
        return;
    }
    let (dx, dy) = (mouse.lLastX as f64, mouse.lLastY as f64);
    let pos = LOCK.with_borrow_mut(|l| {
        let l = l.as_mut().filter(|l| l.applied && l.hwnd == w.hwnd.get())?;
        if l.pin {
            l.virtual_pos.0 += dx;
            l.virtual_pos.1 += dy;
        }
        Some(l.virtual_pos)
    });
    if let Some((x, y)) = pos {
        w.push(|q, id| q.mouse_move(id, x, y, dx, dy, mods));
    }
}

/// WM_ACTIVATE: the app's active window (`None`: none of the app's).
pub(super) fn activated(hwnd: HWND, active: bool) {
    if active {
        ACTIVE.set(hwnd.0 as usize);
        // a lock asked while inactive takes effect now
        LOCK.with_borrow_mut(|l| {
            if let Some(l) = l.as_mut().filter(|l| l.hwnd == hwnd) {
                apply(l, true);
            }
        });
    } else {
        if ACTIVE.get() == hwnd.0 as usize {
            ACTIVE.set(0);
        }
        // a lock held past focus loss leaves the user without a mouse
        if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| l.hwnd == hwnd)) {
            release();
        }
    }
}

/// The primary button went up: a scrub pin ends with the gesture.
pub(super) fn button_up(hwnd: HWND) {
    if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| l.pin && l.hwnd == hwnd)) {
        release();
    }
}

/// The window is going away.
pub(super) fn destroyed(hwnd: HWND) {
    if ACTIVE.get() == hwnd.0 as usize {
        ACTIVE.set(0);
    }
    if LOCK.with_borrow(|l| l.as_ref().is_some_and(|l| l.hwnd == hwnd)) {
        release();
    }
}

