//! The threads started for a run. A run that ends in place (a restart in the same process) takes
//! them with it: what they hold in thread-locals goes with them, and nothing is left running while
//! the next run's state is set up. `spawn` starts one; `sleep` is the wait of those that loop;
//! `end_all` ends them. A thread that lives as long as the process (the window's, the remote
//! service's) does not start here.
//!
//! No cost while a run goes on: `spawn` is a lock and a list entry per thread, `sleep` is what
//! `std::thread::sleep` is, and `ending` is one relaxed load.

use crate::zone_alloc;
use relax_session::sync::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The threads running now: (token, name).
static LIVE: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
/// Signalled when a thread ends, and when the run begins to end.
static CHANGED: Condvar = Condvar::new();
static NEXT: AtomicU64 = AtomicU64::new(0);
static ENDING: AtomicBool = AtomicBool::new(false);
/// What each kind of thread that waits on something of its own needs to be woken (`on_end`).
static WAKE: Mutex<Vec<fn()>> = Mutex::new(Vec::new());

fn live() -> relax_session::sync::MutexGuard<'static, Vec<(u64, String)>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

struct Running(u64);

impl Drop for Running {
    fn drop(&mut self) {
        let mut l = live();
        l.retain(|t| t.0 != self.0);
        CHANGED.notify_all();
    }
}

/// Starts a thread of the run. One started while the run ends is one more `end_all` waits for.
pub fn spawn<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    start(name, None, f)
}

/// The stack of a compiler thread (the owner, the check workers, the parse and pool workers): the
/// parser, the expander, typeck and lowering recurse once per nesting level of the source, so
/// how deep a program may nest never depends on which of them took its work. Address space
/// only: a thread commits what it touches.
pub const COMPILER_STACK: usize = 256 << 20;

/// `spawn`, with a stack of `stack` bytes.
pub fn spawn_sized<T: Send + 'static>(name: &str, stack: usize, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    start(name, Some(stack), f)
}

fn start<T: Send + 'static>(name: &str, stack: Option<usize>, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    let _k = zone_alloc::kept();
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    live().push((token, name.to_string()));
    let running = Running(token);
    let mut builder = std::thread::Builder::new().name(name.to_string());
    if let Some(stack) = stack {
        builder = builder.stack_size(stack);
    }
    builder.spawn(move || {
        let _running = running;
        f()
    })
}

/// The run is ending: a thread that loops leaves its loop.
pub fn ending() -> bool {
    ENDING.load(Ordering::Relaxed)
}

/// Waits `d`, or until the run begins to end. False: it did, the thread leaves.
pub fn sleep(d: Duration) -> bool {
    let deadline = Instant::now() + d;
    let mut l = live();
    loop {
        if ending() {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        l = CHANGED.wait_timeout(l, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
    }
}

/// `wake` runs when the run begins to end, for a thread that waits on something of its own.
pub fn on_end(wake: fn()) {
    let _k = zone_alloc::kept();
    let mut w = WAKE.lock().unwrap_or_else(|e| e.into_inner());
    if !w.contains(&wake) {
        w.push(wake);
    }
}

/// Ends the run's threads: each sees `ending` and is woken, and this waits for them. The names of
/// those still running after `give_up` (empty: all gone, thread-locals included once each joined
/// or returned). The caller says what that means; nothing here pretends they ended.
pub fn end_all(give_up: Duration) -> Vec<String> {
    ENDING.store(true, Ordering::SeqCst);
    let wake: Vec<fn()> = {
        let _k = zone_alloc::kept();
        WAKE.lock().unwrap_or_else(|e| e.into_inner()).clone()
    };
    for w in wake {
        w();
    }
    let deadline = Instant::now() + give_up;
    let mut l = live();
    CHANGED.notify_all();
    loop {
        if l.is_empty() {
            return Vec::new();
        }
        let now = Instant::now();
        if now >= deadline {
            let _k = zone_alloc::kept();
            return l.iter().map(|t| t.1.clone()).collect();
        }
        l = CHANGED.wait_timeout(l, deadline - now).unwrap_or_else(|e| e.into_inner()).0;
        CHANGED.notify_all();
    }
}

/// The next run may start its threads, and the requests that waited go on.
pub fn begin_run() {
    ENDING.store(false, Ordering::SeqCst);
    RUN_ENDS.store(false, Ordering::SeqCst);
    CHANGED.notify_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (One test: `ENDING` is the process's.)
    #[test]
    fn a_run_ends_with_its_threads_and_says_which_stayed() {
        let started = Instant::now();
        let looper = spawn("looper", || {
            while sleep(Duration::from_secs(600)) {}
        }).unwrap();
        let stuck_go: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let stuck = spawn("stuck", move || {
            while !stuck_go.load(Ordering::SeqCst) {
                std::thread::yield_now();
            }
        }).unwrap();
        assert!(live().iter().any(|t| t.1 == "looper"));
        assert_eq!(end_all(Duration::from_millis(50)), ["stuck"]);
        assert!(started.elapsed() < Duration::from_secs(60), "the sleeper did not wake");
        looper.join().unwrap();
        stuck_go.store(true, Ordering::SeqCst);
        stuck.join().unwrap();
        assert_eq!(end_all(Duration::from_secs(60)), Vec::<String>::new());
        begin_run();
        assert!(!ending());
        assert!(sleep(Duration::from_millis(1)));
    }
}

/// A thread of the process that looks at what a run holds does it inside this: a run's end in place
/// puts that back to a fresh process's values while none is in here.
static GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());
/// A run began to end (before its threads are told): a new request waits for the next run.
static RUN_ENDS: AtomicBool = AtomicBool::new(false);

/// Held while a thread of the process (the run window's loop, the stale watcher) is at the run's
/// state, and never twice on one thread. What it waits for inside must end once `ending()`, or the
/// run's end waits with it.
pub fn process_work() -> std::sync::RwLockReadGuard<'static, ()> {
    GATE.read().unwrap_or_else(|e| e.into_inner())
}

/// `process_work` for a request of `makepad inspect`: one that comes while a run ends is answered
/// about the next one.
pub fn request() -> std::sync::RwLockReadGuard<'static, ()> {
    let mut l = live();
    while RUN_ENDS.load(Ordering::SeqCst) {
        l = CHANGED.wait_timeout(l, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(l);
    process_work()
}

/// The first thing a run's end does.
pub fn run_ends() {
    RUN_ENDS.store(true, Ordering::SeqCst);
}

/// A run's end began: a program thread parked for a decision (a panic, a breakpoint) stops waiting.
pub fn run_ending() -> bool {
    RUN_ENDS.load(Ordering::Relaxed)
}

/// No thread of the process is at the run's state until this is dropped. (`ending()` holds by now,
/// so what is inside finishes.) A holder that does not finish keeps the restart waiting: said here.
pub fn reset_gate() -> std::sync::RwLockWriteGuard<'static, ()> {
    let start = Instant::now();
    let mut said = false;
    loop {
        match GATE.try_write() {
            Ok(g) => return g,
            Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {}
        }
        if !said && start.elapsed() >= Duration::from_secs(5) {
            said = true;
            eprintln!("makepad-run: a run's end waits for a request or a watcher that has not finished");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
