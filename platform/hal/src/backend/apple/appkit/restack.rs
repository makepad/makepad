//! A restart's window takes the old one's place (macOS). The exec restart (relax's
//! `reload::exec_restart`) replaces the process, and its windows go with it; the restarted run's
//! first window opens where the old one was (its frame: the run host's remembered geometry) and,
//! with this, at the same place in the window stacking: just below the window that was just above
//! the old one, and without taking the focus the old one did not have. The new image learns that
//! from `MAKEPAD_RUN_RESTART_STACK` (`<pid> <focused 0|1> <window above>`): the pid stays across
//! an exec, so a process the restarted app starts never takes it for its own.
//!
//! A Space cannot be chosen through public API: the window opens on the active Space, which is
//! the old window's whenever the person was looking at it.

use makepad_apple_sys::*;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The first top-level window's number (0: none yet), and whether a top-level window of this
/// process is the key window: written on the main thread, read by `handover` on any thread.
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static KEY: AtomicBool = AtomicBool::new(false);
/// The restart's stacking was taken (by the first window).
static TAKEN: AtomicBool = AtomicBool::new(false);

const ON_SCREEN_ABOVE_WINDOW: u32 = 1 << 1;
const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
    static kCGWindowNumber: CFStringRef;
    static kCGWindowLayer: CFStringRef;
}

/// A top-level window was ordered in (main thread).
pub(super) fn window_shown(number: isize) {
    let _ = WINDOW.compare_exchange(0, number, Ordering::AcqRel, Ordering::Acquire);
}

/// A top-level window became or stopped being the key window (main thread).
pub(super) fn key_changed(on: bool) {
    KEY.store(on, Ordering::Release);
}

/// Right before an exec restart (any thread): the environment that puts the restarted run's
/// window where this process's first window is in the stacking, with its focus.
pub fn handover() -> Vec<(String, String)> {
    let number = WINDOW.load(Ordering::Acquire);
    if number <= 0 {
        return Vec::new();
    }
    let above = window_above(number as u32);
    let focused = KEY.load(Ordering::Acquire);
    vec![("MAKEPAD_RUN_RESTART_STACK".into(), format!("{} {} {}", std::process::id(), u8::from(focused), above))]
}

/// The normal-level window just above `number` on screen (0: none, it is frontmost). CoreGraphics
/// lists the windows above it front to back; window numbers and layers need no permission.
fn window_above(number: u32) -> u32 {
    unsafe {
        let list = CGWindowListCopyWindowInfo(ON_SCREEN_ABOVE_WINDOW | EXCLUDE_DESKTOP_ELEMENTS, number);
        if list.is_null() {
            return 0;
        }
        let read = |dict: *const c_void, key: CFStringRef| -> Option<i64> {
            let n = CFDictionaryGetValue(dict, key as *const c_void);
            let mut v = 0i64;
            (!n.is_null() && CFNumberGetValue(n, kCFNumberSInt64Type, &mut v as *mut i64 as *mut c_void) != 0).then_some(v)
        };
        let mut above = 0;
        for i in 0..CFArrayGetCount(list) {
            let dict = CFArrayGetValueAtIndex(list, i);
            if read(dict, kCGWindowLayer) == Some(0) {
                if let Some(n) = read(dict, kCGWindowNumber) {
                    above = n as u32;
                }
            }
        }
        CFRelease(list);
        above
    }
}

/// For the restarted run's first window (main thread, once): whether it takes the focus and the
/// window to order it below (0: frontmost). None: this run is no restart of a windowed process.
pub(super) fn take_restart() -> Option<(bool, isize)> {
    if TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    let v = std::env::var("MAKEPAD_RUN_RESTART_STACK").ok()?;
    let w: Vec<&str> = v.split_whitespace().collect();
    match w.as_slice() {
        [pid, focused, above] if pid.parse() == Ok(std::process::id()) => Some((*focused == "1", above.parse().ok()?)),
        _ => None,
    }
}
