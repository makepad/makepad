//! The wait registry (makepad inspect, DESIGN.md §4): which thread waits for what, and who holds it.
//! Written only on blocking paths (a requester parking for the owner, a query latch, a program
//! futex wait), which cost a sleep anyway; read lock-free by the inspector, also while the waiting
//! threads and the owner are stuck. One fixed table, one row per thread that ever waited (claimed
//! on its first wait, released at thread exit).
//!
//! Threads are named by their pthread handle (`me()`, the id the owner and the query engine use);
//! the inspector maps handles to OS threads.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// What a thread waits for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    None = 0,
    /// a request to the owner thread (object: the request class)
    Owner,
    /// the owner itself, serving requests while it waits for `done` (object: what it waits for)
    Pump,
    /// a query another thread runs (object: the query's dep node)
    Query,
    /// a slot another thread builds
    Build,
    /// a program futex wait (object: the futex address; label: mutex, condvar, read, write, park,
    /// once). Its holder is an address on the holder thread's stack (0: unknown), not a thread id.
    Futex,
    /// a program thread join
    Join,
}

pub const KIND_NAMES: [&str; 7] = ["none", "owner", "pump", "query", "build", "futex", "join"];

const ROWS: usize = 1024;

struct Row {
    /// the waiting thread's handle (0: free row)
    thread: AtomicU64,
    /// Kind (low byte) and a sequence number (the rest): a reader sees a consistent record when the
    /// word is equal before and after it read the fields
    state: AtomicU64,
    object: AtomicU64,
    holder: AtomicU64,
    since_ns: AtomicU64,
    /// a static label (pointer, length): the class, the query's name
    label: AtomicUsize,
    label_len: AtomicUsize,
}

static TABLE: [Row; ROWS] = [const {
    Row {
        thread: AtomicU64::new(0),
        state: AtomicU64::new(0),
        object: AtomicU64::new(0),
        holder: AtomicU64::new(0),
        since_ns: AtomicU64::new(0),
        label: AtomicUsize::new(0),
        label_len: AtomicUsize::new(0),
    }
}; ROWS];

/// The current thread's id as the registry names it.
#[cfg(unix)]
pub fn me() -> u64 {
    extern "C" {
        fn pthread_self() -> usize;
    }
    unsafe { pthread_self() as u64 }
}

#[cfg(windows)]
pub fn me() -> u64 {
    unsafe { relax_session::win32::GetCurrentThreadId() as u64 }
}

struct Mine(Cell<usize>);

impl Drop for Mine {
    fn drop(&mut self) {
        let i = self.0.get();
        if i < ROWS {
            TABLE[i].state.store(0, Release);
            TABLE[i].thread.store(0, Release);
        }
    }
}

thread_local! {
    static MINE: Mine = const { Mine(Cell::new(usize::MAX)) };
}

fn row() -> Option<&'static Row> {
    let i = MINE.try_with(|m| {
        if m.0.get() == usize::MAX {
            let t = me();
            let start = (t as usize >> 4).wrapping_mul(0x9e37_79b9) % ROWS;
            for k in 0..ROWS {
                let j = (start + k) % ROWS;
                if TABLE[j].thread.compare_exchange(0, t, AcqRel, Relaxed).is_ok() {
                    m.0.set(j);
                    break;
                }
            }
        }
        m.0.get()
    });
    match i {
        Ok(i) if i < ROWS => Some(&TABLE[i]),
        _ => None,
    }
}

/// makepad inspect's timeline: called at each wait's start (`true`, label, kind name, object) and
/// end (`false`) while set (one relaxed load per wait otherwise).
pub static TRACE: AtomicUsize = AtomicUsize::new(0);
type TraceFn = fn(bool, &'static str, &'static str, u64);

#[inline]
fn trace(begin: bool, kind: Kind, label: &'static str, object: u64) {
    let h = TRACE.load(Relaxed);
    if h != 0 {
        let f: TraceFn = unsafe { std::mem::transmute(h) };
        f(begin, KIND_NAMES[kind as usize], label, object);
    }
}

/// While alive, this thread is recorded as waiting; dropping it restores the wait it was nested in
/// (the owner pumping inside a wait serves requests that wait again).
pub struct Guard {
    prev: Option<(u64, u64, u64, u64, usize, usize)>,
}

/// Records that this thread waits for `object` of `kind`, held by thread `holder` (0: unknown).
#[inline]
pub fn begin(kind: Kind, object: u64, holder: u64, label: &'static str) -> Guard {
    let Some(r) = row() else { return Guard { prev: None } };
    let s = r.state.load(Relaxed);
    let prev = Some((s, r.object.load(Relaxed), r.holder.load(Relaxed), r.since_ns.load(Relaxed), r.label.load(Relaxed), r.label_len.load(Relaxed)));
    write(r, kind as u64, object, holder, crate::clock::wall_ns(), label.as_ptr() as usize, label.len());
    trace(true, kind, label, object);
    Guard { prev }
}

fn write(r: &Row, kind: u64, object: u64, holder: u64, since: u64, label: usize, label_len: usize) {
    // odd sequence while writing: readers retry
    let seq = (r.state.load(Relaxed) >> 8) + 1;
    r.state.store((seq << 8) | 0xff, Release);
    r.object.store(object, Relaxed);
    r.holder.store(holder, Relaxed);
    r.since_ns.store(since, Relaxed);
    r.label.store(label, Relaxed);
    r.label_len.store(label_len, Relaxed);
    r.state.store(((seq + 1) << 8) | (kind & 0xff), Release);
}

impl Drop for Guard {
    #[inline]
    fn drop(&mut self) {
        let (Some((s, o, h, t, l, n)), Some(r)) = (self.prev, row()) else { return };
        trace(false, Kind::None, "", 0);
        write(r, s & 0xff, o, h, t, l, n);
    }
}

/// The holder of this thread's current wait became known (a latch's runner found after the wait
/// began).
pub fn set_holder(holder: u64) {
    if let Some(r) = row() {
        r.holder.store(holder, Relaxed);
    }
}

/// Starts a wait with no guard (program futex waits enter and leave through two C calls).
pub fn enter(kind: Kind, object: u64, holder: u64, label: &'static str) {
    if let Some(r) = row() {
        write(r, kind as u64, object, holder, crate::clock::wall_ns(), label.as_ptr() as usize, label.len());
        trace(true, kind, label, object);
    }
}

pub fn leave() {
    if let Some(r) = row() {
        trace(false, Kind::None, "", 0);
        write(r, 0, 0, 0, 0, 0, 0);
    }
}

/// Every thread that has a row (waited at least once, or registered).
pub fn threads() -> Vec<u64> {
    TABLE.iter().map(|r| r.thread.load(Acquire)).filter(|&t| t != 0).collect()
}

/// Claims this thread's row now (long-lived threads others wait for: the owner).
pub fn register() {
    let _ = row();
}

/// One thread's current wait.
#[derive(Clone, Debug)]
pub struct Wait {
    pub thread: u64,
    pub kind: Kind,
    pub object: u64,
    pub holder: u64,
    pub since_ns: u64,
    pub label: &'static str,
}

/// Every thread's current wait (consistent per row; lock-free).
pub fn snapshot() -> Vec<Wait> {
    let mut out = Vec::new();
    for r in TABLE.iter() {
        let thread = r.thread.load(Acquire);
        if thread == 0 {
            continue;
        }
        for _ in 0..8 {
            let s0 = r.state.load(Acquire);
            if s0 & 0xff == 0xff {
                std::hint::spin_loop();
                continue;
            }
            let (object, holder, since_ns) = (r.object.load(Relaxed), r.holder.load(Relaxed), r.since_ns.load(Relaxed));
            let (l, n) = (r.label.load(Relaxed), r.label_len.load(Relaxed));
            if r.state.load(Acquire) != s0 {
                continue;
            }
            let kind = match s0 & 0xff {
                1 => Kind::Owner,
                2 => Kind::Pump,
                3 => Kind::Query,
                4 => Kind::Build,
                5 => Kind::Futex,
                6 => Kind::Join,
                _ => break,
            };
            // labels are 'static strs written whole before the state word
            let label = if l == 0 { "" } else { unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(l as *const u8, n)) } };
            out.push(Wait { thread, kind, object, holder, since_ns, label });
            break;
        }
    }
    out
}
