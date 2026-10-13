//! Pointer preferences for the native Linux direct backend. The WM applies
//! these immediately; its system worker owns persistence. Windowed backends
//! leave pointer configuration to their compositor.
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::direct::{POINTER_SPEED_MIN, POINTER_SPEED_MAX, POINTER_SPEED_DEFAULT};

static MOUSE_SPEED: AtomicU32 = AtomicU32::new(POINTER_SPEED_DEFAULT);
static TOUCHPAD_SPEED: AtomicU32 = AtomicU32::new(POINTER_SPEED_DEFAULT);

pub fn pointer_speed(touchpad: bool) -> u32 {
    (if touchpad { &TOUCHPAD_SPEED } else { &MOUSE_SPEED }).load(Ordering::Relaxed)
}

pub fn set_pointer_speed(touchpad: bool, hundredths: u32) {
    (if touchpad { &TOUCHPAD_SPEED } else { &MOUSE_SPEED }).store(
        hundredths.clamp(POINTER_SPEED_MIN, POINTER_SPEED_MAX), Ordering::Relaxed,
    );
}

/// Natural scrolling for touchpads: the content follows the fingers, as on
/// macOS (on by default). Mouse wheels keep their own direction either way.
static NATURAL_SCROLL: AtomicBool = AtomicBool::new(true);

pub fn natural_scroll() -> bool {
    NATURAL_SCROLL.load(Ordering::Relaxed)
}

pub fn set_natural_scroll(on: bool) {
    NATURAL_SCROLL.store(on, Ordering::Relaxed);
}
