//! Identifies threads that run a user interface and counts work that belongs on a worker.
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

thread_local! {
    static UI_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// Mark the calling thread when it starts running a UI.
pub fn mark_ui_thread() {
    UI_THREAD.set(true);
}

pub fn is_ui_thread() -> bool {
    UI_THREAD.get()
}

/// One counter per guarded operation. Only a violation touches the atomic.
pub struct WorkerOnly {
    message: &'static str,
    hits: AtomicUsize,
}

impl WorkerOnly {
    pub const fn new(message: &'static str) -> Self {
        Self { message, hits: AtomicUsize::new(0) }
    }

    pub fn check(&'static self) {
        if is_ui_thread() && self.hits.fetch_add(1, Ordering::Relaxed) == 0 {
            let mut reported = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
            if reported.is_empty() {
                crate::process_end::at_exit(end_run);
            }
            reported.push(self);
            drop(reported);
            eprintln!("{} ran on the UI thread", self.message);
        }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }
}

// Only operations that hit the UI thread enter this list. Worker calls never lock it.
static REPORTED: Mutex<Vec<&'static WorkerOnly>> = Mutex::new(Vec::new());

/// The guarded operations observed on UI threads, with their counts since run start.
pub fn violations() -> Vec<(&'static str, usize)> {
    REPORTED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|op| (op.message, op.hits())).collect()
}

extern "C" fn end_run() {
    for op in std::mem::take(&mut *REPORTED.lock().unwrap_or_else(|e| e.into_inner())) {
        op.hits.store(0, Ordering::Relaxed);
    }
}
