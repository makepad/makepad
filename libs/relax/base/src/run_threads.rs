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

/// The threads running now: (token, name, requested stack size in bytes).
static LIVE: Mutex<Vec<(u64, String, usize)>> = Mutex::new(Vec::new());
/// Signalled when a thread ends, and when the run begins to end.
static CHANGED: Condvar = Condvar::new();
static NEXT: AtomicU64 = AtomicU64::new(0);
static ENDING: AtomicBool = AtomicBool::new(false);
/// What each kind of thread that waits on something of its own needs to be woken (`on_end`).
static WAKE: Mutex<Vec<fn()>> = Mutex::new(Vec::new());

fn live() -> relax_session::sync::MutexGuard<'static, Vec<(u64, String, usize)>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

struct Running(u64);

/// Run on each of the run's threads as it ends (`on_thread_end`).
static THREAD_END: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

/// Its thread is ending: the end hook runs on it (`on_thread_end`).
struct Ending;

impl Drop for Ending {
    fn drop(&mut self) {
        if let Some(end) = THREAD_END.get() {
            end();
        }
    }
}

/// `end` runs on each thread `spawn` started as it ends, its function returned or unwound (once
/// for the process: a second hook is not taken).
pub fn on_thread_end(end: fn()) {
    let _ = THREAD_END.set(end);
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut l = live();
        l.retain(|t| t.0 != self.0);
        CHANGED.notify_all();
    }
}

/// Starts a thread of the run on [`COMPILER_STACK`]. One started while the run ends is one more
/// `end_all` waits for.
pub fn spawn<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    start(name, COMPILER_STACK, f)
}

/// The stack of a compiler thread (the owner, the check, parse and pool workers, and every run
/// thread started via [`spawn`]). The parser, expander, typeck, borrowck and lowering recurse once
/// per nesting level of the source, so how deep a program may nest never depends on which thread
/// took its work. A thread commits only the pages it touches.
pub const COMPILER_STACK: usize = if cfg!(target_pointer_width = "64") { 256 << 20 } else { 16 << 20 };

/// `spawn`, with a stack of `stack` bytes.
pub fn spawn_sized<T: Send + 'static>(name: &str, stack: usize, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    start(name, stack, f)
}

/// Snapshot of live run threads and their requested stack sizes in bytes.
pub fn live_stacks() -> Vec<(String, usize)> {
    live().iter().map(|(_, name, stack)| (name.clone(), *stack)).collect()
}

fn start<T: Send + 'static>(name: &str, stack: usize, f: impl FnOnce() -> T + Send + 'static) -> std::io::Result<std::thread::JoinHandle<T>> {
    let _k = zone_alloc::kept();
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    live().push((token, name.to_string(), stack));
    let running = Running(token);
    std::thread::Builder::new().name(name.to_string()).stack_size(stack).spawn(move || {
        let _running = running;
        let _ending = Ending;
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

    // These tests share the process's LIVE and ENDING.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn spawn_defaults_to_compiler_stack_and_records_size() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel();
        let h = spawn("stack-default-probe", move || {
            let os_stack = crate::stack::thread_stack_size();
            let reg = live_stacks();
            tx.send((os_stack, reg)).unwrap();
            let _ = go_rx.recv();
        }).unwrap();
        let (os_stack, reg) = rx.recv().unwrap();
        go_tx.send(()).unwrap();
        h.join().unwrap();

        assert!(reg.iter().any(|(n, s)| n == "stack-default-probe" && *s == COMPILER_STACK),
            "default spawn must register COMPILER_STACK in LIVE: {reg:?}");
        if let Some(bytes) = os_stack {
            // Windows excludes its guard region from the usable stack size.
            let min = COMPILER_STACK.saturating_sub(1 << 20);
            assert!(bytes >= min, "default spawn OS stack {bytes} < {min} (COMPILER_STACK={COMPILER_STACK})");
        }

        let small = 4 << 20;
        let (tx, rx) = std::sync::mpsc::channel();
        let h = spawn_sized("stack-sized-probe", small, move || {
            let os_stack = crate::stack::thread_stack_size();
            let reg = live_stacks();
            tx.send((os_stack, reg)).unwrap();
        }).unwrap();
        let (os_stack, reg) = rx.recv().unwrap();
        h.join().unwrap();

        assert!(reg.iter().any(|(n, s)| n == "stack-sized-probe" && *s == small),
            "spawn_sized must register explicit size {small}: {reg:?}");
        if let Some(bytes) = os_stack {
            // Under Relax the thread runs on a separate program stack.
            if !cfg!(relax) {
                assert!(bytes < COMPILER_STACK, "spawn_sized(4 MiB) unexpectedly got {bytes}");
            }
        }
    }

    #[test]
    fn a_run_ends_with_its_threads_and_says_which_stayed() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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

        // while a run ends, a request of another process waits for the next run; one this process
        // asks itself is turned away at once, so a request of the ending run that waits for it
        // (holding the gate) ends, and so does the run
        let held = request(false).expect("no run ends");
        run_ends();
        assert!(request(true).is_none());
        let gone: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let other = std::thread::spawn(move || {
            let g = request(false);
            gone.store(true, Ordering::SeqCst);
            g.is_some()
        });
        drop(held);
        let gate = reset_gate();
        for _ in 0..1000 {
            std::thread::yield_now();
        }
        assert!(!gone.load(Ordering::SeqCst), "another's request did not wait for the next run");
        drop(gate);
        begin_run();
        assert!(other.join().unwrap());
        assert!(request(true).is_some());
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
/// about the next one. Not one this process asks itself (`own`): None, at once. That one comes from
/// the ending run, whose next run cannot begin before it is answered: the platform of a run asks the
/// JIT how busy it is (`jit`, `owner`) while a reply from its event loop is late, and it does so
/// for a request that holds the gate (an app command, such as `text`, asked as the run's end began),
/// which then never ended, nor did the run, and the next one never drew (an aichat, 11-08).
pub fn request(own: bool) -> Option<std::sync::RwLockReadGuard<'static, ()>> {
    let mut l = live();
    while RUN_ENDS.load(Ordering::SeqCst) {
        if own {
            return None;
        }
        l = CHANGED.wait_timeout(l, Duration::from_millis(50)).unwrap_or_else(|e| e.into_inner()).0;
    }
    drop(l);
    Some(process_work())
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
            eprintln!("relax: a run's end waits for a request or a watcher that has not finished");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
