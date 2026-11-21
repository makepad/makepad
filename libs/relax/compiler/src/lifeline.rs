//! Long-lived JIT threads (owner, run monitor, O2/O3 and speculative workers, sampler): every
//! one runs its loop under `catch_unwind` through `spawn`, so none of them can end silently.
//!
//! - A panic is reported loudly: stderr (with the panic's source position, which the panic hook
//!   records per thread), a flight-recorder event `thread.panic`, and the registry row.
//! - `OnPanic::Restart`: the loop body runs again (after a short backoff; after `MAX_RESTARTS`
//!   panics the thread is given up: dead, reported). `OnPanic::Fatal`: the process ends with
//!   the report (the owner: every requester would wait for it forever).
//! - The registry keeps each live thread's state, restarts, last panic and heartbeat (`beat`).
//!   Ended threads contribute counts and their last end to one record per name.
//!
//! Nothing here runs on a hot path: `beat` is one thread-local read and one relaxed store.

use std::cell::Cell;
use std::collections::BTreeMap;
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

#[derive(Clone, Debug, Default)]
pub struct Ended {
    pub started: u64,
    pub ended: u64,
    pub dead: u64,
    pub panics: u64,
    pub last_end_ns: u64,
    pub last_end: u8,
    /// The most recent panicking end, retained across clean ends for the exit summary.
    pub last_panic: String,
    pub last_end_panicked: bool,
}

struct Registry {
    // A slot stays at its index until its owner finishes. The next registration takes the first vacancy.
    slots: Vec<Option<Entry>>,
    // Reusing a lower slot must not move its new owner ahead of older live threads.
    live_order: Vec<usize>,
    names: BTreeMap<String, Ended>,
}

static REG: relax_session::sync::Mutex<Registry> = relax_session::sync::Mutex::new(Registry {
    slots: Vec::new(),
    live_order: Vec::new(),
    names: BTreeMap::new(),
});

thread_local! {
    /// this thread's registry index (usize::MAX: not a registered thread)
    static INDEX: Cell<usize> = const { Cell::new(usize::MAX) };
    /// the last panic's position on this thread ("file.rs:line"), written by the panic hook with
    /// no allocation (it may run on any thread at any time)
    static PANIC_AT: Cell<PanicAt> = const { Cell::new(NO_PANIC_AT) };
}

/// A recorded panic position: (the path's tail, its length, the line).
pub type PanicAt = ([u8; 62], u8, u32);
pub const NO_PANIC_AT: PanicAt = ([0; 62], 0, 0);

/// This thread's recorded panic position as it is stored (the owner hands it to a requester).
pub fn panic_at_raw() -> PanicAt {
    PANIC_AT.try_with(|c| c.get()).unwrap_or(NO_PANIC_AT)
}

/// Takes over a position recorded on another thread (a request's panic on the owner, resumed here).
pub fn set_panic_at_raw(at: PanicAt) {
    let _ = PANIC_AT.try_with(|c| c.set(at));
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
    REG.lock().unwrap_or_else(|e| e.into_inner()).register(name, policy, os_id)
}

impl Registry {
    fn register(&mut self, name: &str, policy: OnPanic, os_id: u64) -> usize {
        let i = crate::zone_alloc::kept_with(|| {
            self.names.entry(name.to_owned()).or_default().started += 1;
            let entry = Some(Entry {
                name: name.to_owned(),
                os_id,
                policy,
                restarts: 0,
                started_ns: crate::clock::wall_ns(),
                last_panic: String::new(),
            });
            let i = self.slots.iter().position(Option::is_none).unwrap_or(self.slots.len());
            if i == self.slots.len() {
                self.slots.push(entry);
            } else {
                self.slots[i] = entry;
            }
            self.live_order.push(i);
            i
        });
        if i < SLOTS {
            STATE[i].store(ALIVE, Ordering::Relaxed);
            BEAT[i].store(0, Ordering::Relaxed);
        }
        let _ = INDEX.try_with(|c| c.set(i));
        i
    }
}

fn finish(i: usize, state: u8) {
    let mut reg = REG.lock().unwrap_or_else(|e| e.into_inner());
    set_state(i, state);
    let _ = INDEX.try_with(|c| c.set(usize::MAX));
    let entry = reg.slots[i].take().unwrap();
    reg.live_order.retain(|&slot| slot != i);
    let record = reg.names.get_mut(&entry.name).unwrap();
    record.ended += 1;
    record.dead += u64::from(state == DEAD);
    record.panics += u64::from(entry.restarts);
    record.last_end_ns = crate::clock::wall_ns();
    record.last_end = state;
    record.last_end_panicked = entry.restarts != 0;
    if record.last_end_panicked {
        record.last_panic = entry.last_panic;
    }
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
    // a thread std has no name for (a test's, one native code started) takes this one at the OS:
    // a panic's note, `inspect threads` and stall lines show it
    if std::thread::current().name().is_none() {
        crate::os::thread::set_own_name(name);
    }
    let i = register(name, policy);
    let mut restarts = 0u32;
    loop {
        clear_panic_at();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut body));
        let p = match r {
            Ok(()) => {
                finish(i, EXITED);
                return;
            }
            Err(p) => p,
        };
        // control-flow unwinds that end the thread on purpose (the process is exiting)
        if p.is::<crate::owner::ExitParked>() {
            finish(i, EXITED);
            std::panic::resume_unwind(p);
        }
        // work that gave way (preempted, the owner busy) and unwound this far: the thread goes on,
        // nothing went wrong, and it uses up none of its restarts
        if crate::jit::panic_payload(&*p).1 {
            crate::inspect::flightrec::event("thread.bail", name, restarts as u64, policy as u64);
            continue;
        }
        restarts += 1;
        let line = crate::zone_alloc::kept_with(|| panic_line(&*p));
        {
            let mut reg = REG.lock().unwrap_or_else(|e| e.into_inner());
            let e = reg.slots[i].as_mut().unwrap();
            e.restarts = restarts;
            e.last_panic = crate::zone_alloc::kept_with(|| line.clone());
        }
        crate::inspect::flightrec::event("thread.panic", &format!("{}: {}", name, line), restarts as u64, policy as u64);
        match policy {
            OnPanic::Fatal => {
                set_state(i, DEAD);
                eprintln!("relax: FATAL: JIT thread `{}` panicked: {} -- it cannot be restarted, the process ends", name, line);
                crate::inspect::flightrec::fault_dump(&format!("thread {} panicked: {}", name, line));
                std::process::abort();
            }
            OnPanic::Restart if restarts > MAX_RESTARTS => {
                eprintln!("relax: JIT thread `{}` panicked {} times, last: {} -- given up (dead: `{tool} inspect threads`)", name, restarts, line, tool = crate::tool_name());
                finish(i, DEAD);
                return;
            }
            OnPanic::Restart => {
                set_state(i, RESTARTING);
                eprintln!("relax: JIT thread `{}` panicked: {} -- restarted ({} of {})", name, line, restarts, MAX_RESTARTS);
                std::thread::sleep(std::time::Duration::from_millis(if cfg!(test) { 1 } else { 10u64 << restarts.min(6) }));
                set_state(i, ALIVE);
            }
        }
    }
}

/// Spawns the long-lived JIT thread `name` running `body` under `run`. A failed spawn is said
/// (false), never ignored.
pub fn spawn(name: String, policy: OnPanic, stack: Option<usize>, body: impl FnMut() + Send + 'static) -> bool {
    let n2 = crate::zone_alloc::kept_with(|| name.clone());
    let started = match stack {
        Some(s) => crate::run_threads::spawn_sized(&name, s, move || run(&n2, policy, body)),
        None => crate::run_threads::spawn(&name, move || run(&n2, policy, body)),
    };
    match started {
        Ok(_) => true,
        Err(e) => {
            eprintln!("relax: cannot start JIT thread `{}`: {}", name, e);
            crate::inspect::flightrec::event("thread.spawn_failed", &name, 0, 0);
            false
        }
    }
}

impl Registry {
    fn live_rows(&self) -> Vec<Row> {
        self.live_order.iter().map(|&i| {
            let e = self.slots[i].as_ref().unwrap();
            Row {
                name: e.name.clone(),
                os_id: e.os_id,
                policy: e.policy,
                state: if i < SLOTS { STATE[i].load(Ordering::Relaxed) } else { ALIVE },
                restarts: e.restarts,
                started_ns: e.started_ns,
                beat_ns: if i < SLOTS { BEAT[i].load(Ordering::Relaxed) } else { 0 },
                last_panic: e.last_panic.clone(),
            }
        }).collect()
    }

    fn ended_names(&self) -> BTreeMap<String, Ended> {
        self.names.iter()
            .filter(|(_, r)| r.ended != 0)
            .map(|(name, r)| (name.clone(), r.clone()))
            .collect()
    }
}

/// Live threads, in registration order.
pub fn snapshot() -> Vec<Row> {
    REG.lock().unwrap_or_else(|e| e.into_inner()).live_rows()
}

/// One record for each name with at least one ended thread.
pub fn ended_snapshot() -> BTreeMap<String, Ended> {
    REG.lock().unwrap_or_else(|e| e.into_inner()).ended_names()
}

/// Copies both sides of each fold under the same registry lock.
pub(crate) fn snapshot_with_ended() -> (Vec<Row>, BTreeMap<String, Ended>) {
    let reg = REG.lock().unwrap_or_else(|e| e.into_inner());
    (reg.live_rows(), reg.ended_names())
}

/// One line for the run's exit summary when a registered thread panicked (None: none did).
pub fn summary() -> Option<String> {
    let (rows, ended) = snapshot_with_ended();
    let mut bad: Vec<String> = rows.iter().filter(|r| r.restarts > 0).map(|r| format!("{} {} ({} panics, last: {})", r.name, STATE_NAMES[r.state as usize & 3], r.restarts, r.last_panic)).collect();
    for (name, r) in ended.into_iter().filter(|(_, r)| r.panics != 0) {
        bad.push(format!("{} {} ({} panics, last: {})", name, STATE_NAMES[r.last_end as usize], r.panics, r.last_panic));
    }
    (!bad.is_empty()).then(|| format!("JIT threads that panicked: {}", bad.join("; ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn a_panicking_loop_restarts_and_is_reported() {
        static RUNS: AtomicU64 = AtomicU64::new(0);
        let name = format!("lifeline-test-restart-{}", RUNS.fetch_add(1, Ordering::SeqCst));
        let (tx, rx) = mpsc::channel();
        let mut calls = 0;
        assert!(spawn(name.clone(), OnPanic::Restart, None, move || {
            calls += 1;
            if calls == 1 {
                panic!("lifeline test panic");
            }
            beat();
            tx.send((calls, INDEX.with(Cell::get))).unwrap();
        }));
        let timeout = Duration::from_secs(10);
        let (calls, first) = rx.recv_timeout(timeout).unwrap();
        assert_eq!(calls, 2);
        // The sender drops with the body after run has folded the thread.
        assert_eq!(rx.recv_timeout(timeout), Err(mpsc::RecvTimeoutError::Disconnected));
        assert!(snapshot().iter().all(|r| r.name != name), "the ended thread still has a live row");
        let ended = ended_snapshot();
        let r = &ended[&name];
        assert_eq!((r.started, r.ended, r.dead, r.panics, r.last_end), (1, 1, 0, 1, EXITED));
        assert!(r.last_panic.contains("lifeline test panic"), "{}", r.last_panic);
        assert!(r.last_panic.contains("lifeline.rs:"), "the panic hook's position: {}", r.last_panic);

        let next_name = name.clone();
        std::thread::spawn(move || {
            let os_id = crate::inspect::threads::self_os_id();
            let (i, already_reused) = {
                let mut reg = REG.lock().unwrap_or_else(|e| e.into_inner());
                // Another concurrent registration may already occupy the freed slot.
                let already_reused = reg.slots[first].is_some();
                (reg.register(&next_name, OnPanic::Restart, os_id), already_reused)
            };
            let (rows, ended) = snapshot_with_ended();
            let before = rows.iter().find(|r| r.name == next_name).unwrap();
            beat();
            let after = snapshot().into_iter().find(|r| r.name == next_name).unwrap();
            finish(i, EXITED);
            assert!(already_reused || i <= first, "registration skipped the freed slot {first} for {i}");
            assert_eq!((before.state, before.beat_ns), (ALIVE, 0), "registration must reset the slot");
            assert_ne!(after.beat_ns, 0, "the next thread must have a working heartbeat");
            let r = &ended[&next_name];
            assert_eq!((r.started, r.ended), (2, 1), "started includes the live thread");
        }).join().unwrap();
        let ended = ended_snapshot();
        let clean = &ended[&name];
        assert_eq!((clean.started, clean.ended, clean.panics), (2, 2, 1));
        assert!(!clean.last_end_panicked);
        assert!(summary().unwrap().contains(&format!("{name} exited (1 panics, last: {})", r.last_panic)));
    }

    #[test]
    fn a_loop_that_keeps_panicking_is_given_up_as_dead() {
        static RUNS: AtomicU64 = AtomicU64::new(0);
        let name = format!("lifeline-test-dead-{}", RUNS.fetch_add(1, Ordering::SeqCst));
        let thread_name = name.clone();
        let mut k = 0u32;
        let h = std::thread::spawn(move || {
            run(&thread_name, OnPanic::Restart, || {
                k += 1;
                if k <= MAX_RESTARTS + 1 {
                    panic!("again {}", k);
                }
            });
            crate::os::thread::name(crate::os::thread::current())
        });
        let os_name = h.join().unwrap();
        let ended = ended_snapshot();
        let r = &ended[&name];
        assert_eq!((r.started, r.ended, r.dead, r.last_end), (1, 1, 1, DEAD));
        assert_eq!(r.panics, u64::from(MAX_RESTARTS + 1));
        // std left the thread unnamed: it took its lifeline name at the OS, which its panics' notes
        // show (Linux keeps 15 bytes of it)
        let want = if cfg!(any(target_os = "linux", target_os = "android")) { &name[..15] } else { &name };
        assert_eq!(os_name, want, "the thread's OS name");
    }
}
