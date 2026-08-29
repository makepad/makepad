//! The last draw-shader compile error, for a live editor to show instead of
//! a silent blank widget: the splash→shader stage (`draw_vars`) and the
//! backend stage (Metal library compile) both report here.

use std::sync::{Mutex, OnceLock};

fn slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

pub fn note(error: String) {
    *slot().lock().unwrap() = Some(error);
}

/// The most recent error since the last take, if any.
pub fn take() -> Option<String> {
    slot().lock().unwrap().take()
}
