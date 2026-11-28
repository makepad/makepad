//! A test ticks one real worker at a time. A tick returns when that worker reaches its next
//! input wait, so scheduling delays on the host cannot look like congestion on the media path.
use super::Spawn;
use std::{
    cell::{Cell, RefCell},
    sync::{mpsc::{channel, Receiver, Sender}, Arc},
};

struct Worker {
    ticks: Receiver<u64>,
    done: Sender<()>,
    started: Cell<bool>,
}

thread_local! {
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
    static NOW: Cell<Option<u64>> = const { Cell::new(None) };
}

pub(super) fn now() -> Option<u64> { NOW.with(Cell::get) }

/// Run a synchronous test call with its clock fixed at `now`, restoring the
/// previous clock when the call returns or panics.
pub(super) fn with_now<R>(now: u64, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<u64>);
    impl Drop for Reset {
        fn drop(&mut self) { NOW.set(self.0); }
    }
    let _reset = Reset(NOW.replace(Some(now)));
    f()
}

/// None for ordinary workers; false when the test has closed this worker's clock.
pub(super) fn tick() -> Option<bool> {
    WORKER.with_borrow(|worker| {
        let worker = worker.as_ref()?;
        if worker.started.replace(true) && worker.done.send(()).is_err() { return Some(false); }
        let at = worker.ticks.recv().ok();
        NOW.set(at);
        Some(at.is_some())
    })
}

pub(super) struct Ticks {
    ticks: Sender<u64>,
    done: Receiver<()>,
}

impl Ticks {
    pub(super) fn at(&self, now: u64) {
        self.ticks.send(now).expect("worker exited before its tick");
        self.done.recv().expect("worker exited during its tick");
    }
}

pub(super) struct CallTicks {
    pub network: Ticks,
    pub encode: Ticks,
    pub voice: Ticks,
}

impl CallTicks {
    pub(super) fn registered(workers: Receiver<(&'static str, Ticks)>) -> Self {
        let mut workers: std::collections::BTreeMap<_, _> = workers.try_iter().collect();
        Self {
            network: workers.remove("call network").expect("network worker registered"),
            encode: workers.remove("call encode").expect("camera worker registered"),
            voice: workers.remove("call synthetic voice").expect("microphone worker registered"),
        }
    }
}

/// Install the clock before each worker enters its loop. Decoding is event-driven already.
pub(super) fn spawn_on_ticks(spawn: Spawn) -> (Spawn, Receiver<(&'static str, Ticks)>) {
    let (register, registered) = channel();
    let spawn = Arc::new(move |name: &'static str, job: Box<dyn FnOnce() + Send>| {
        if !matches!(name, "call network" | "call encode" | "call synthetic voice") {
            spawn(name, job);
            return;
        }
        let (send, ticks) = channel();
        let (done, finished) = channel();
        register.send((name, Ticks { ticks: send, done: finished })).unwrap();
        spawn(name, Box::new(move || {
            WORKER.set(Some(Worker { ticks, done, started: Cell::new(false) }));
            job();
        }));
    });
    (spawn, registered)
}
