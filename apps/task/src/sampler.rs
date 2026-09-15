//! The sampler worker: owns the OS backend, takes interval updates over a
//! channel, and wakes the UI with `SignalToUI`. The UI drains the result
//! channel on `Event::Signal`, so a slow syscall can never stall a frame.

use crate::backend::{self, Snapshot};
use makepad_widgets::log;
use makepad_widgets::makepad_platform::thread::{
    CancellationToken, SignalToUI, SpawnError, TaskHandle, ThreadOptions, ThreadSpawner,
};
use makepad_widgets::Cx;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};

/// Floor on the tick period, whatever the UI asks for.
const MIN_INTERVAL_SECS: f64 = 0.1;
/// How long the wait can go without re-reading `interval_ms`, so switching from
/// 10 s down to 0.5 s is felt immediately instead of after the current wait.
const WAIT_SLICE_SECS: f64 = 0.05;

/// Start the one lifetime sampler worker. The toolbar feeds interval changes
/// through `interval_rx`; dropping that sender shuts the worker down.
pub fn spawn(
    spawner: &ThreadSpawner,
    tx: Sender<Snapshot>,
    interval_rx: Receiver<u64>,
    initial_interval_ms: u64,
) -> Result<TaskHandle<()>, SpawnError> {
    spawner.spawn_worker(
        ThreadOptions {
            name: Some("task-sampler".into()),
            ..Default::default()
        },
        move || {
            let mut backend = backend::new_backend();
            log!("task: sampling via {}", backend.name());
            let wait = CancellationToken::new();
            let mut interval_ms = initial_interval_ms;
            loop {
                let started = Cx::monotonic_now();
                if tx.send(backend.sample()).is_err() {
                    // The UI dropped the receiver: the app is going away.
                    break;
                }
                SignalToUI::set_ui_signal();
                loop {
                    loop {
                        match interval_rx.try_recv() {
                            Ok(next) => interval_ms = next,
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => return,
                        }
                    }
                    let target = (interval_ms as f64 / 1000.0).max(MIN_INTERVAL_SECS);
                    let now = Cx::monotonic_now();
                    if now - started >= target {
                        break;
                    }
                    let deadline = (started + target).min(now + WAIT_SLICE_SECS);
                    let _ = wait.wait_until(deadline);
                }
            }
        },
    )
}
