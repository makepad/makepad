//! Pointer preferences for the native Linux direct backend. The WM applies
//! these immediately; its system worker owns persistence. Windowed backends
//! leave pointer configuration to their compositor.
use std::sync::atomic::{AtomicU32, Ordering};

pub const POINTER_SPEED_MIN: u32 = 25;
pub const POINTER_SPEED_MAX: u32 = 300;
pub const POINTER_SPEED_DEFAULT: u32 = 100;

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
