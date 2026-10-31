//! Allocator swaps: code that is about to load something that replaces the process allocator
//! calls [`before_allocator_swap`] first and [`after_allocator_swap`] once it is loaded.
//!
//! Why: Chromium (makepad-cef) installs PartitionAlloc's malloc shim from a library constructor
//! when its framework is loaded (macOS: `InitializeDefaultMallocZoneWithPartitionAlloc`,
//! `constructor(0)`). The install briefly unregisters and re-registers the system zone; a free
//! on another thread inside that window finds no zone and crashes the process (Chromium's own
//! comment). Chrome does it before other threads exist; an embedder loads it mid-run. After the
//! swap, frees of earlier pointers are forwarded to their original zone, so only the window
//! needs care: threads that allocate on their own should be parked across it.
//!
//! Two kinds of listeners are told: in-process ones ([`register`], app threads that opt in), and
//! the loader's hook when the program runs under one that provides it (the `makepad` loader
//! exports `relax_allocator_swap(phase)`; a normal build has none). Calls nest: only the
//! outermost pair reaches the listeners.

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The allocator is about to be replaced: park allocating threads.
    Before,
    /// The new allocator is in place: resume.
    After,
}

static LISTENERS: Mutex<Vec<fn(Phase)>> = Mutex::new(Vec::new());
static DEPTH: AtomicU32 = AtomicU32::new(0);

/// Adds an in-process listener (called on the swapping thread, Before then After).
pub fn register(listener: fn(Phase)) {
    LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).push(listener);
}

/// The loader's hook, looked up once (0 = none).
fn loader_hook() -> Option<extern "C" fn(u32)> {
    static HOOK: AtomicUsize = AtomicUsize::new(1);
    let mut h = HOOK.load(Ordering::Acquire);
    if h == 1 {
        h = lookup_hook();
        HOOK.store(h, Ordering::Release);
    }
    if h == 0 {
        None
    } else {
        Some(unsafe { std::mem::transmute::<usize, extern "C" fn(u32)>(h) })
    }
}

#[cfg(unix)]
fn lookup_hook() -> usize {
    extern "C" {
        fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }
    #[cfg(target_os = "macos")]
    let default = -2isize as *mut std::ffi::c_void;
    #[cfg(not(target_os = "macos"))]
    let default = std::ptr::null_mut();
    unsafe { dlsym(default, c"relax_allocator_swap".as_ptr()) as usize }
}

#[cfg(not(unix))]
fn lookup_hook() -> usize {
    0
}

fn notify(phase: Phase) {
    let listeners: Vec<fn(Phase)> = LISTENERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match phase {
        Phase::Before => {
            for l in &listeners {
                l(phase);
            }
            if let Some(h) = loader_hook() {
                h(0);
            }
        }
        Phase::After => {
            if let Some(h) = loader_hook() {
                h(1);
            }
            for l in listeners.iter().rev() {
                l(phase);
            }
        }
    }
}

/// The process allocator is about to be replaced (e.g. before loading Chromium).
pub fn before_allocator_swap() {
    if DEPTH.fetch_add(1, Ordering::AcqRel) == 0 {
        notify(Phase::Before);
    }
}

/// The replacement is in place.
pub fn after_allocator_swap() {
    if DEPTH.fetch_sub(1, Ordering::AcqRel) == 1 {
        notify(Phase::After);
    }
}
