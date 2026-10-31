//! The flight recorder's program side (`makepad run` keeps the last minutes of a session in
//! memory: frames, every thread's waits, the JIT's work, input; `makepad inspect <app> rec save`
//! writes them out). Apps and games add their own events so their logic leaves a trace a bug
//! seen once can be read from afterwards:
//!
//! ```ignore
//! makepad_error_log::flight::mark("game.spawn", "follower 12 at (3, 4)");
//! makepad_error_log::flight_mark!("ai.state", id, 0, "follower {} -> {:?}", id, state);
//! ```
//!
//! A category is a short dotted name (`game.spawn`, `ai.state`, `ui.open`); the text says what
//! happened (cut at 168 bytes); `a` / `b` are free numbers (an entity id, a value) queries can
//! filter on. Outside `makepad run` (a rustc build) every call is one relaxed load and returns.

use std::sync::atomic::{AtomicUsize, Ordering};

/// the loader's entries: [on, mark, input] (0: not looked up yet, 1: absent)
static HOOKS: [AtomicUsize; 3] = [AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0)];

#[cfg(unix)]
fn lookup(name: &std::ffi::CStr) -> usize {
    extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    // RTLD_DEFAULT: -2 on Apple targets, null elsewhere
    #[cfg(target_vendor = "apple")]
    let default = -2isize as *mut std::ffi::c_void;
    #[cfg(not(target_vendor = "apple"))]
    let default = std::ptr::null_mut();
    unsafe { dlsym(default, name.as_ptr()) as usize }
}

#[cfg(windows)]
fn lookup(name: &std::ffi::CStr) -> usize {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
        fn GetProcAddress(module: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    unsafe { GetProcAddress(GetModuleHandleW(std::ptr::null()), name.as_ptr()) as usize }
}

#[cfg(not(any(unix, windows)))]
fn lookup(_name: &std::ffi::CStr) -> usize {
    0
}

fn hook(i: usize) -> usize {
    let h = HOOKS[i].load(Ordering::Relaxed);
    if h != 0 {
        return h;
    }
    let name: &std::ffi::CStr = match i {
        0 => c"relax_flight_on",
        1 => c"relax_flight_mark",
        _ => c"relax_flight_input",
    };
    let p = lookup(name);
    let v = if p == 0 { 1 } else { p };
    HOOKS[i].store(v, Ordering::Relaxed);
    v
}

/// The recorder records (a `makepad run` app with the recorder on): skip building texts otherwise.
#[inline]
pub fn on() -> bool {
    let h = hook(0);
    if h == 1 {
        return false;
    }
    let f: extern "C" fn() -> bool = unsafe { std::mem::transmute(h) };
    f()
}

/// An app or game event for the recording.
#[inline]
pub fn mark(cat: &str, text: &str) {
    mark_ab(cat, text, 0, 0);
}

/// An event with two numbers (an entity id, a value).
pub fn mark_ab(cat: &str, text: &str, a: u64, b: u64) {
    let h = hook(1);
    if h == 1 {
        return;
    }
    let f: extern "C" fn(*const u8, usize, *const u8, usize, u64, u64) = unsafe { std::mem::transmute(h) };
    f(cat.as_ptr(), cat.len(), text.as_ptr(), text.len(), a, b);
}

/// An input event the program dispatched (`injected`: by makepad remote, not the person), as
/// the remote command that injects it again (the platform's dispatch; a replay feeds these back).
pub fn input(frame: u64, injected: bool, command: &str) {
    let h = hook(2);
    if h == 1 {
        return;
    }
    let f: extern "C" fn(u64, u64, *const u8, usize) = unsafe { std::mem::transmute(h) };
    f(frame, injected as u64, command.as_ptr(), command.len());
}

/// `flight_mark!(cat, a, b, fmt, args..)`: an event whose text is formatted only while the
/// recorder records.
#[macro_export]
macro_rules! flight_mark {
    ($cat:expr, $a:expr, $b:expr, $($arg:tt)*) => {
        if $crate::flight::on() {
            $crate::flight::mark_ab($cat, &format!($($arg)*), $a as u64, $b as u64);
        }
    };
}
