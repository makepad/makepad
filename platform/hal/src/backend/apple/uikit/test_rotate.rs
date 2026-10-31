//! `MAKEPAD_TEST_ROTATE=1` (iOS): a moment after the first presented frame the shell asks the
//! window scene for landscape (the same path `MobileUi` ORIENTATION takes), and a while later
//! back to portrait; each turn logs `rotate: <orientation> requested` and every window's size as
//! UIKit lays it out (`rotate: window <sid>:<id> <w>x<h>`). The app hears the turns as Geom /
//! SafeArea events like a user rotating the device. The counterpart of AppKit's live-resize
//! test (`appkit/test_resize.rs`): automation has no other way to rotate a simulator.

use super::app::{app, run_on_main};
use makepad_apple_sys::*;
use std::sync::atomic::{AtomicBool, Ordering};

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_TEST_ROTATE").is_ok_and(|v| v == "1"))
}

static STARTED: AtomicBool = AtomicBool::new(false);
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

/// A frame reached the screen (any thread): the first one starts the turns.
pub fn presented() {
    if !enabled() || STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new().name("test-rotate".into()).spawn(|| {
        for orientation in [crate::types::mobile_ui::orientation::LANDSCAPE, crate::types::mobile_ui::orientation::PORTRAIT] {
            std::thread::sleep(SETTLE);
            run_on_main(move || turn(orientation));
            std::thread::sleep(SETTLE);
            run_on_main(report);
        }
    });
}

fn turn(orientation: u32) {
    eprintln!("rotate: {} requested", if orientation == crate::types::mobile_ui::orientation::LANDSCAPE { "landscape" } else { "portrait" });
    super::app::mobile_ui(crate::types::mobile_ui::ORIENTATION, orientation);
}

fn report() {
    for win in app().live_windows() {
        let bounds: NSRect = unsafe { msg_send![win.view, bounds] };
        eprintln!("rotate: window {}:{} {}x{}", win.sid.get(), win.id.get().index, bounds.size.width, bounds.size.height);
    }
}
