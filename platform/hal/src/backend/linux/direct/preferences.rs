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

/// Natural scrolling, as on macOS, per device class (`touchpad`, else mouse wheels): the
/// content follows the fingers on a touchpad and the wheel on a mouse. Both on by default in
/// the WM's session; the WM's devices settings turn each off. This is the one place makepad
/// picks a scroll direction itself: only this direct backend (our WM) applies it, windowed
/// backends on another desktop keep that desktop's own direction. Desk Link's injected wheel
/// is already in the content direction its viewer saw and is never turned (`raw_input`).
static NATURAL_SCROLL_MOUSE: AtomicBool = AtomicBool::new(true);
static NATURAL_SCROLL_TOUCHPAD: AtomicBool = AtomicBool::new(true);

pub fn natural_scroll(touchpad: bool) -> bool {
    (if touchpad { &NATURAL_SCROLL_TOUCHPAD } else { &NATURAL_SCROLL_MOUSE }).load(Ordering::Relaxed)
}

pub fn set_natural_scroll(touchpad: bool, on: bool) {
    (if touchpad { &NATURAL_SCROLL_TOUCHPAD } else { &NATURAL_SCROLL_MOUSE }).store(on, Ordering::Relaxed);
}

/// Tap to click on touchpads: a short touch clicks (one finger primary, two
/// secondary, three middle). Off by default, as on macOS.
static TAP_TO_CLICK: AtomicBool = AtomicBool::new(false);

pub fn tap_to_click() -> bool {
    TAP_TO_CLICK.load(Ordering::Relaxed)
}

pub fn set_tap_to_click(on: bool) {
    TAP_TO_CLICK.store(on, Ordering::Relaxed);
}
