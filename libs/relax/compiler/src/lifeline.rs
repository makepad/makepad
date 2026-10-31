//! Long-lived JIT threads (owner, run monitor, O2/O3 and speculative workers, sampler): every
//! one runs its loop under `catch_unwind` through `spawn`, so none of them can end silently.
//!
//! - A panic is reported loudly: stderr (with the panic's source position, which the panic hook
//!   records per thread), a flight-recorder event `thread.panic`, and the registry row.
//! - `OnPanic::Restart`: the loop body runs again (after a short backoff; after `MAX_RESTARTS`
//!   panics the thread is given up: dead, reported). `OnPanic::Fatal`: the process ends with
//!   the report (the owner: every requester would wait for it forever).
//! - The registry (`snapshot`) is what `makepad inspect threads` shows next to the OS view:
//!   alive / restarting / dead / exited, restarts, last panic, last heartbeat (`beat`).
//!
//! Nothing here runs on a hot path: `beat` is one thread-local read and one relaxed store.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OnPanic {
    /// the loop body runs again (its state starts over)
    Restart,
    /// the process ends with a report
    Fatal,
}

/// Panics after which a restarting thread is given up (dead, reported).
pub const MAX_RESTARTS: u32 = 16;
/// Registered threads with a heartbeat slot (more still register; they have no heartbeat).
const SLOTS: usize = 128;

pub const ALIVE: u8 = 0;
pub const RESTARTING: u8 = 1;
pub const DEAD: u8 = 2;
pub const EXITED: u8 = 3;
pub const STATE_NAMES: [&str; 4] = ["alive", "restarting", "dead", "exited"];

static STATE: [AtomicU8; SLOTS] = [const { AtomicU8::new(ALIVE) }; SLOTS];
static BEAT: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

#[derive(Clone, Debug)]
pub struct Row {
    pub name: String,
    pub os_id: u64,
    pub policy: OnPanic,
    pub state: u8,
    pub restarts: u32,
    pub started_ns: u64,
    /// wall_ns of the last `beat` (0: the thread does not beat)
    pub beat_ns: u64,
    /// the last panic: message and position
    pub last_panic: String,
}

struct Entry {
    name: String,
    os_id: u64,
    policy: OnPanic,
    restarts: u32,
    started_ns: u64,
    last_panic: String,
}

static REG: relax_session::sync::Mutex<Vec<Entry>> = relax_session::sync::Mutex::new(Vec::new());

thread_local! {
    /// this thread's registry index (usize::MAX: not a registered thread)
    static INDEX: Cell<usize> = const { Cell::new(usize::MAX) };
    /// the last panic's position on this thread ("file.rs:line"), written by the panic hook with
    /// no allocation (it may run on any thread at any time)
    static PANIC_AT: Cell<([u8; 62], u8, u32)> = const { Cell::new(([0; 62], 0, 0)) };
}

/// Records where a panic happened on this thread (the panic hook; no allocation, no lock).
pub fn note_panic_at(file: &str, line: u32) {
    // the source path as the panic reports it (its tail when longer than the cell)
    let name = file.as_bytes();
    let mut b = [0u8; 62];
    let n = name.len().min(b.len());
    b[..n].copy_from_slice(&name[name.len() - n..]);
    let _ = PANIC_AT.try_with(|c| c.set((b, n as u8, line)));
}

/// "file.rs:line" of the last panic on this thread ("" when none was recorded).
pub fn panic_at() -> String {
    PANIC_AT
        .try_with(|c| {
            let (b, n, line) = c.get();
            if n == 0 {
                String::new()
            } else {
                format!("{}:{}", String::from_utf8_lossy(&b[..n as usize]), line)
            }
        })
        .unwrap_or_default()
}

/// Forgets the recorded position (a scope that wants only its own panic's position).
pub fn clear_panic_at() {
    let _ = PANIC_AT.try_with(|c| c.set(([0; 62], 0, 0)));
}

/// This thread is alive and making progress (owner per request, monitor per round, ...).
#[inline]
pub fn beat() {
    let i = INDEX.try_with(|c| c.get()).unwrap_or(usize::MAX);
    if i < SLOTS {
        BEAT[i].store(crate::clock::wall_ns().max(1), Ordering::Relaxed);
    }
}

fn register(name: &str, policy: OnPanic) -> usize {
    // the hook that records each panic's position (installed once; never swapped)
    crate::jit::install_ice_hook();
    let os_id = crate::inspect::threads::self_os_id();
    let mut reg = REG.lock().unwrap_or_else(|e| e.into_inner());
    let i = crate::zone_alloc::kept_with(|| {
        reg.push(Entry { name: name.to_string(), os_id, policy, restarts: 0, started_ns: crate::clock::wall_ns(), last_panic: String::new() });
        reg.len() - 1
    });
    if i < SLOTS {
        STATE[i].store(ALIVE, Ordering::Relaxed);
        BEAT[i].store(0, Ordering::Relaxed);
    }
    let _ = INDEX.try_with(|c| c.set(i));
    i
}

fn set_state(i: usize, s: u8) {
    if i < SLOTS {
        STATE[i].store(s, Ordering::Relaxed);
    }
}

/// A caught panic as one line: its message, and "(at file:line)" when this thread's panic hook
/// recorded the position (`panic_at`).
pub fn panic_line(p: &(dyn std::any::Any + Send)) -> String {
    let msg = payload_text(p);
    let at = panic_at();
    if at.is_empty() { msg } else { format!("{} (at {})", msg, at) }
}

fn payload_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "(non-string panic payload)".to_string())
}

/// Runs `body` on this thread as the registered long-lived thread `name` (called first thing
/// inside the thread). A panic is reported and handled by `policy`; a return ends the thread
/// (state `exited`).
pub fn run(name: &str, policy: OnPanic, mut body: impl FnMut()) {
    let i = register(name, policy);
    let mut restarts = 0u32;
    loop {
        clear_panic_at();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body));
        let p = match r {
            Ok(()) => {
                set_state(i, EXITED);
                return;
            }
            Err(p) => p,
        };
        // control-flow unwinds that end the thread on purpose (the process is exiting)
        if p.is::<crate::owner::ExitParked>() {
            set_state(i, EXITED);
            std::panic::resume_unwind(p);
        }
        restarts += 1;
        let line = crate::zone_alloc::kept_with(|| panic_line(&*p));
        {
            let mut reg = REG.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = reg.get_mut(i) {
                e.restarts = restarts;
                e.last_panic = line.clone();
            }
        }
        crate::inspect::flightrec::event("thread.panic", &format!("{}: {}", name, line), restarts as u64, policy as u64);
        match policy {
            OnPanic::Fatal => {
                set_state(i, DEAD);
                eprintln!("makepad-run: FATAL: JIT thread `{}` panicked: {} -- it cannot be restarted, the process ends", name, line);
                crate::inspect::flightrec::fault_dump(&format!("thread {} panicked: {}", name, line));
                std::process::abort();
            }
            OnPanic::Restart if restarts > MAX_RESTARTS => {
                set_state(i, DEAD);
                eprintln!("makepad-run: JIT thread `{}` panicked {} times, last: {} -- given up (dead: `makepad inspect threads`)", name, restarts, line);
                return;
            }
            OnPanic::Restart => {
                set_state(i, RESTARTING);
                eprintln!("makepad-run: JIT thread `{}` panicked: {} -- restarted ({} of {})", name, line, restarts, MAX_RESTARTS);
                std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 1 } else { 10u64 << restarts.min(6) }));
                set_state(i, ALIVE);
            }
        }
    }
}

/// Spawns the long-lived JIT thread `name` running `body` under `run`. A failed spawn is said
/// (false), never ignored.
pub fn spawn(name: String, policy: OnPanic, stack: Option<usize>, body: impl FnMut() + Send + 'static) -> bool {
    let mut b = std::thread::Builder::new().name(name.clone());
    if let Some(s) = stack {
        b = b.stack_size(s);
    }
    let n2 = crate::zone_alloc::kept_with(|| name.clone());
    match b.spawn(move || run(&n2, policy, body)) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("makepad-run: cannot start JIT thread `{}`: {}", name, e);
            crate::inspect::flightrec::event("thread.spawn_failed", &name, 0, 0);
            false
        }
    }
}

/// Every registered thread, in registration order.
pub fn snapshot() -> Vec<Row> {
    let reg = REG.lock().unwrap_or_else(|e| e.into_inner());
    reg.iter()
        .enumerate()
        .map(|(i, e)| Row {
            name: e.name.clone(),
            os_id: e.os_id,
            policy: e.policy,
            state: if i < SLOTS { STATE[i].load(Ordering::Relaxed) } else { ALIVE },
            restarts: e.restarts,
            started_ns: e.started_ns,
            beat_ns: if i < SLOTS { BEAT[i].load(Ordering::Relaxed) } else { 0 },
            last_panic: e.last_panic.clone(),
        })
        .collect()
}

/// One line for the run's exit summary when a registered thread panicked (None: none did).
pub fn summary() -> Option<String> {
    let rows = snapshot();
    let bad: Vec<String> = rows.iter().filter(|r| r.restarts > 0).map(|r| format!("{} {} ({} panics, last: {})", r.name, STATE_NAMES[r.state as usize & 3], r.restarts, r.last_panic)).collect();
    (!bad.is_empty()).then(|| format!("JIT threads that panicked: {}", bad.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panicking_loop_restarts_and_is_reported() {
        let n = std::sync::Arc::new(AtomicU64::new(0));
        let n2 = n.clone();
        assert!(spawn("lifeline-test-restart".to_string(), OnPanic::Restart, None, move || {
            if n2.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("lifeline test panic");
            }
        }));
        let t0 = std::time::Instant::now();
        loop {
            let row = snapshot().into_iter().find(|r| r.name == "lifeline-test-restart");
            if let Some(r) = row.filter(|r| r.state == EXITED) {
                assert_eq!(r.restarts, 1);
                assert!(r.last_panic.contains("lifeline test panic"), "{}", r.last_panic);
                assert!(r.last_panic.contains("lifeline.rs:"), "the panic hook's position: {}", r.last_panic);
                break;
            }
            assert!(t0.elapsed().as_secs() < 10, "the restarted loop never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_loop_that_keeps_panicking_is_given_up_as_dead() {
        // (no backoff sleeps worth waiting for: the test runs the body directly)
        let mut k = 0u32;
        let h = std::thread::spawn(move || {
            run("lifeline-test-dead", OnPanic::Restart, || {
                k += 1;
                if k <= MAX_RESTARTS + 1 {
                    panic!("again {}", k);
                }
            })
        });
        h.join().unwrap();
        let r = snapshot().into_iter().find(|r| r.name == "lifeline-test-dead").unwrap();
        assert_eq!(r.state, DEAD);
        assert_eq!(r.restarts, MAX_RESTARTS + 1);
    }
}
