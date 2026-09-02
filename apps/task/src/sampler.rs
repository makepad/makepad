//! The sampler thread: owns the OS backend, ticks on the interval the toolbar
//! picker sets, and wakes the UI with `SignalToUI`. Nothing here touches `Cx` —
//! the UI drains the channel on `Event::Signal`, so a slow syscall can never
//! stall a frame.

use crate::backend::{self, Snapshot};
use makepad_widgets::log;
use makepad_widgets::makepad_platform::thread::SignalToUI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

/// Floor on the tick period, whatever the UI asks for.
const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// How long the wait can go without re-reading `interval_ms`, so switching from
/// 10 s down to 0.5 s is felt immediately instead of after the current wait.
const WAIT_SLICE: Duration = Duration::from_millis(50);

/// `interval_ms` is shared with the UI; the loop re-reads it every slice.
pub fn spawn(tx: Sender<Snapshot>, interval_ms: Arc<AtomicU64>) {
    thread::Builder::new()
        .name("task-sampler".to_string())
        .spawn(move || {
            let mut backend = backend::new_backend();
            log!("task: sampling via {}", backend.name());
            loop {
                let started = Instant::now();
                if tx.send(backend.sample()).is_err() {
                    // The UI dropped the receiver: the app is going away.
                    break;
                }
                SignalToUI::set_ui_signal();
                loop {
                    let target =
                        Duration::from_millis(interval_ms.load(Ordering::Relaxed)).max(MIN_INTERVAL);
                    let elapsed = started.elapsed();
                    if elapsed >= target {
                        break;
                    }
                    thread::sleep((target - elapsed).min(WAIT_SLICE));
                }
            }
        })
        .expect("task sampler thread");
}
