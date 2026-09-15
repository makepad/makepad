//! First `SignalToUI::set_ui_signal` from a worker in a headless process
//! must not create the platform event loop (on macOS: NSApplication).
//!
//! `Cx::new` installs the real UI waker. Headless tests currently work around
//! this with `install_ui_waker(None)` after `Cx::new`; this test does not.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use makepad_platform::{Cx, SignalToUI};

#[test]
fn first_set_ui_signal_from_worker_is_under_50ms() {
    let _cx = Cx::new(Box::new(|_, _| {}));

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let start = Instant::now();
        SignalToUI::set_ui_signal();
        let _ = tx.send(start.elapsed());
    });

    let elapsed = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(elapsed) => elapsed,
        Err(_) => {
            println!("first set_ui_signal latency: timed out after 10000.0 ms");
            panic!(
                "first set_ui_signal from a worker did not return within 10 s"
            );
        }
    };
    let ms = elapsed.as_secs_f64() * 1000.0;
    println!("first set_ui_signal latency: {ms:.1} ms");
    assert!(
        ms < 50.0,
        "first set_ui_signal from a worker took {ms:.1} ms, expected < 50 ms"
    );
}
