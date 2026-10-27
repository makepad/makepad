//! `makepad inspect <app> session view [open|dismiss]`: the run's view over the app (the halted
//! view: compile errors, a parked panic, the JIT's state) from outside, and what it shows now.
//! The JIT ring in the app's caption is display only, so this is how an agent or the user opens
//! the view without a stop. The run's host loop (`makepad run`'s hal_run) takes the request on
//! its next turn and publishes the view's state after every change.
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

const NONE: u8 = 0;
const OPEN: u8 = 1;
const DISMISS: u8 = 2;

static REQUEST: AtomicU8 = AtomicU8::new(NONE);
static STATE: Mutex<String> = Mutex::new(String::new());
static WAKE: OnceLock<fn()> = OnceLock::new();

/// The host loop's wake (it may sleep until an event): set once by the run.
pub fn set_wake(wake: fn()) {
    let _ = WAKE.set(wake);
}

/// Asks the host loop to open (`true`) or dismiss the view. False when no run hosts a view
/// (a direct program, bare relax).
pub fn request(open: bool) -> bool {
    let Some(wake) = WAKE.get() else { return false };
    REQUEST.store(if open { OPEN } else { DISMISS }, Ordering::Release);
    wake();
    true
}

/// The host loop: the request since its last turn (`Some(true)` open, `Some(false)` dismiss).
pub fn take() -> Option<bool> {
    match REQUEST.swap(NONE, Ordering::AcqRel) {
        OPEN => Some(true),
        DISMISS => Some(false),
        _ => None,
    }
}

/// The host loop: what the view shows now (a JSON object's fields, no braces).
pub fn publish(state: String) {
    *STATE.lock().unwrap_or_else(|e| e.into_inner()) = state;
}

pub fn state() -> String {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
