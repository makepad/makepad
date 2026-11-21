//! First `SignalToUI::set_ui_signal` from a worker in a headless process
//! must not create the platform event loop (on macOS: NSApplication), and returns.
//!
//! `Cx::new` installs the real UI waker. Headless tests currently work around
//! this with `install_ui_waker(None)` after `Cx::new`; this test does not.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use makepad_platform::{Cx, SignalToUI};

/// AppKit's `NSApp`, which `+[NSApplication sharedApplication]` sets: null while no NSApplication
/// exists, and when AppKit is not loaded at all.
#[cfg(target_os = "macos")]
fn ns_app() -> *const std::ffi::c_void {
    use std::ffi::{c_char, c_void};
    extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    let slot = unsafe { dlsym(RTLD_DEFAULT, b"NSApp\0".as_ptr().cast()) } as *const *const c_void;
    if slot.is_null() {
        std::ptr::null()
    } else {
        unsafe { *slot }
    }
}

#[test]
fn first_set_ui_signal_from_worker_creates_no_event_loop() {
    let _cx = Cx::new(Box::new(|_, _| {}));
    #[cfg(target_os = "macos")]
    assert!(ns_app().is_null(), "Cx::new created an NSApplication");

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        SignalToUI::set_ui_signal();
        let _ = tx.send(());
    });
    // the signal never waits on an event loop: 10 s is a hang, not a timing
    rx.recv_timeout(Duration::from_secs(10))
        .expect("first set_ui_signal from a worker did not return within 10 s");

    #[cfg(target_os = "macos")]
    assert!(
        ns_app().is_null(),
        "the first set_ui_signal from a worker created an NSApplication"
    );
}
