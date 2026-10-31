//! `MAKEPAD_TEST_LIVE_RESIZE=1`: after the first frames, drag the first window's bottom-right
//! resize corner with events posted in-process (`[NSApp postEvent:atStart:]`; no Accessibility
//! or Screen Recording permission). The mouse-down on the resize edge enters AppKit's live
//! resize tracking loop, which `setFrame:display:` does not. Logs `live_resize: start/stop`, the
//! frames presented during the loop and the window size before and after. Moved from
//! os/apple/macos/macos_window.rs (`test_live_resize`, branch hal-relaxcfg); the frames counted
//! are the presented ones (the sessions run while the main thread tracks), and the drag starts a
//! moment after the first one (an idle app presents only a few frames, however long it waits).

use super::app::{app, run_on_main, wake_event_loop};
use makepad_apple_sys::*;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

const IDLE: u8 = 0;
const POSTED: u8 = 1;
const DONE: u8 = 2;
static STATE: AtomicU8 = AtomicU8::new(IDLE);
static LOOP: AtomicU8 = AtomicU8::new(0);
static LOOP_PRESENTED: AtomicU32 = AtomicU32::new(0);
/// How long after the first presented frame the drag starts.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MAKEPAD_TEST_LIVE_RESIZE").is_ok_and(|v| v == "1"))
}

fn frame(window: ObjcId) -> NSRect {
    unsafe { msg_send![window, frame] }
}

/// The live-resize loop of a window began / ended (main thread).
pub fn note_loop(window: ObjcId, start: bool) {
    if !enabled() {
        return;
    }
    if start {
        if STATE.load(Ordering::Relaxed) != POSTED {
            return;
        }
        let f = frame(window);
        LOOP.store(1, Ordering::Relaxed);
        LOOP_PRESENTED.store(0, Ordering::Relaxed);
        eprintln!("live_resize: start size {}x{}", f.size.width, f.size.height);
    } else {
        if LOOP.swap(0, Ordering::Relaxed) != 1 {
            return;
        }
        let f = frame(window);
        STATE.store(DONE, Ordering::Relaxed);
        eprintln!("live_resize: stop paints {} size {}x{}", LOOP_PRESENTED.load(Ordering::Relaxed), f.size.width, f.size.height);
    }
}

/// A frame of a window reached the screen (any thread): counts the frames of the loop and, at the
/// first one, arranges for the drag to start once the app has settled.
pub fn paint() {
    if !enabled() {
        return;
    }
    if LOOP.load(Ordering::Relaxed) == 1 {
        LOOP_PRESENTED.fetch_add(1, Ordering::Relaxed);
    }
    if STATE.swap(POSTED, Ordering::Relaxed) != IDLE {
        return;
    }
    let spawned = std::thread::Builder::new().name("test-live-resize-start".into()).spawn(|| {
        std::thread::sleep(SETTLE);
        start_drag();
    });
    if let Err(e) = spawned {
        eprintln!("live_resize: cannot start the test: {e}");
        STATE.store(DONE, Ordering::Relaxed);
    }
}

fn start_drag() {
    // the window is read on the main thread; one thread paces the ~1 s drag
    run_on_main(|| {
        let win = app().first_window();
        let Some(win) = win else {
            STATE.store(DONE, Ordering::Relaxed);
            return;
        };
        let f = frame(win.window);
        let number: isize = unsafe { msg_send![win.window, windowNumber] };
        let w = f.size.width;
        eprintln!("live_resize: posting drag from {w}x{}, window {number}", f.size.height);
        let spawned = std::thread::Builder::new().name("test-live-resize".into()).spawn(move || drag(number, w));
        if let Err(e) = spawned {
            eprintln!("live_resize: cannot start the drag thread: {e}");
            STATE.store(DONE, Ordering::Relaxed);
        }
    });
}

fn post(kind: NSEventType, x: f64, y: f64, number: isize, n: isize, pressure: f32) {
    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let info: ObjcId = msg_send![class!(NSProcessInfo), processInfo];
        let time: f64 = msg_send![info, systemUptime];
        let event: ObjcId = msg_send![
            class!(NSEvent),
            mouseEventWithType: kind
            location: NSPoint { x, y }
            modifierFlags: 0u64
            timestamp: time
            windowNumber: number
            context: nil
            eventNumber: n
            clickCount: 1isize
            pressure: pressure
        ];
        let ns_app: ObjcId = msg_send![class!(NSApplication), sharedApplication];
        let () = msg_send![ns_app, postEvent: event atStart: false];
        let () = msg_send![pool, release];
    }
    wake_event_loop();
}

fn drag(number: isize, w: f64) {
    // window coordinates: origin bottom-left; the corner is the frame's bottom-right
    let (x0, y0) = (w - 2.0, 2.0);
    let steps = 60;
    post(NSEventType::NSLeftMouseDown, x0, y0, number, 1, 1.0);
    for i in 1..=steps {
        std::thread::sleep(std::time::Duration::from_millis(16));
        let t = i as f64 / steps as f64;
        post(NSEventType::NSLeftMouseDragged, x0 + 120.0 * t, y0 - 80.0 * t, number, 1 + i, 1.0);
    }
    post(NSEventType::NSLeftMouseUp, x0 + 120.0, y0 - 80.0, number, 2 + steps, 0.0);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    if STATE.load(Ordering::Relaxed) == POSTED {
        eprintln!("live_resize: no live resize loop was entered");
    }
}
