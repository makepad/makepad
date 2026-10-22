//! Lock-free building blocks of the JIT's tier and owner queues (local/agent_state/relax-campaign/
//! OWNERARCH.DESIGN.md §5): well-known patterns only, the memory ordering of every step named where
//! it happens, each tested under contention below.
//!
//! - `IdStack`: a Treiber stack of u32 ids, intrusive (each id's link word lives with the id, e.g. in
//!   its tier record), popped only as a whole (`take_all`), so there is no single-node pop and no ABA.
//!   A count stub (`rt_hot`, on the program's own threads) pushes with one CAS loop: no lock, no wait.
//! - `Sleepers`: the idle workers of such a stack, parked with `std::thread::park`; a producer wakes
//!   one with a load and, only when one sleeps, a `fetch_and` + `unpark` (no mutex on the producer's
//!   side). The Dekker pair (worker: set idle bit, then read the stack; producer: push, then read the
//!   idle bits, all SeqCst) rules out a lost wakeup; the park token covers an unpark that comes first.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// A Treiber stack of ids (OWNERARCH.DESIGN.md §5.1). `head` holds the newest id + 1 (0: empty);
/// each id's link word holds the next id + 1. The caller guarantees an id is in the stack at most
/// once (its own state CAS deduplicates, e.g. the tier record's T_COUNTING -> T_QUEUED) and pushes
/// it again only after a `take_all` handed it out.
pub struct IdStack {
    head: AtomicU32,
}

impl IdStack {
    pub const fn new() -> IdStack {
        IdStack { head: AtomicU32::new(0) }
    }

    /// Pushes `id`, whose link word is `link`. True: the stack was empty (a consumer may sleep).
    /// The CAS is SeqCst: it is the release that publishes `link` and whatever the caller wrote for
    /// `id` before it (the takers' acquire pairs with it, also through later pushes: each push is a
    /// read-modify-write, so it continues the release sequence of the ones before), and it is the
    /// producer's half of the Dekker pair with `Sleepers::sleep`.
    #[inline]
    pub fn push(&self, id: u32, link: &AtomicU32) -> bool {
        debug_assert!(id != u32::MAX);
        let mut h = self.head.load(Ordering::Relaxed);
        loop {
            // (Relaxed: published by the CAS below)
            link.store(h, Ordering::Relaxed);
            match self.head.compare_exchange_weak(h, id + 1, Ordering::SeqCst, Ordering::Relaxed) {
                Ok(_) => return h == 0,
                Err(x) => h = x,
            }
        }
    }

    /// Takes every id pushed so far: the token of the newest (0: none), walked with `walk`. SeqCst
    /// (an acquire: the links and the callers' writes before each push are visible).
    #[inline]
    pub fn take_all(&self) -> u32 {
        self.head.swap(0, Ordering::SeqCst)
    }

    /// Nothing is pushed now (SeqCst: the worker's half of the Dekker pair with `push`).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::SeqCst) == 0
    }

    /// Hands out the ids of a `take_all` token, newest first. `link(id)` is id's link word; it is
    /// read BEFORE `f(id)` runs, since `f` may let `id` be pushed again (which rewrites its link).
    pub fn walk(token: u32, link: impl Fn(u32) -> u32, mut f: impl FnMut(u32)) {
        let mut t = token;
        while t != 0 {
            let id = t - 1;
            // (Relaxed: ordered by the take's acquire)
            t = link(id);
            f(id);
        }
    }
}

impl Default for IdStack {
    fn default() -> Self {
        IdStack::new()
    }
}

/// Up to 64 parked consumers of a lock-free queue (OWNERARCH.DESIGN.md §5.1, worker wakeup).
/// Worker `k` registers its thread once (`register`), then sleeps with `sleep(k, ready)`; a producer
/// calls `wake_one` after its SeqCst publish. Wakeups may be spurious: a woken worker looks again.
pub struct Sleepers {
    idle: AtomicU64,
    threads: [std::sync::OnceLock<std::thread::Thread>; 64],
}

impl Sleepers {
    pub const fn new() -> Sleepers {
        Sleepers { idle: AtomicU64::new(0), threads: [const { std::sync::OnceLock::new() }; 64] }
    }

    /// Worker `k` (< 64) is the calling thread.
    pub fn register(&self, k: usize) {
        let _ = self.threads[k].set(std::thread::current());
    }

    /// Worker `k` sleeps until a producer wakes it, unless `ready()` (the queue's emptiness test,
    /// SeqCst) says there is work: the idle bit is set first (SeqCst), then `ready` is read, so a
    /// push the read misses sees the bit (`wake_one`).
    pub fn sleep(&self, k: usize, ready: impl Fn() -> bool) {
        let bit = 1u64 << k;
        self.idle.fetch_or(bit, Ordering::SeqCst);
        if !ready() {
            std::thread::park();
        }
        self.idle.fetch_and(!bit, Ordering::SeqCst);
    }

    /// `sleep` that also returns after `timeout` (a worker that polls besides its wakeups).
    pub fn sleep_timeout(&self, k: usize, timeout: std::time::Duration, ready: impl Fn() -> bool) {
        let bit = 1u64 << k;
        self.idle.fetch_or(bit, Ordering::SeqCst);
        if !ready() {
            std::thread::park_timeout(timeout);
        }
        self.idle.fetch_and(!bit, Ordering::SeqCst);
    }

    /// Wakes one sleeping worker, if any. One SeqCst load when none sleeps (the count stub's path).
    #[inline]
    pub fn wake_one(&self) {
        let mut m = self.idle.load(Ordering::SeqCst);
        while m != 0 {
            let k = m.trailing_zeros() as usize;
            let bit = 1u64 << k;
            // whoever clears the bit owns the wakeup of worker k
            if self.idle.fetch_and(!bit, Ordering::SeqCst) & bit != 0 {
                if let Some(t) = self.threads[k].get() {
                    t.unpark();
                }
                return;
            }
            m = self.idle.load(Ordering::SeqCst);
        }
    }

    /// Wakes every sleeping worker (shutdown, a policy change).
    pub fn wake_all(&self) {
        let m = self.idle.swap(0, Ordering::SeqCst);
        for k in 0..64 {
            if m & (1u64 << k) != 0 {
                if let Some(t) = self.threads[k].get() {
                    t.unpark();
                }
            }
        }
    }
}

impl Default for Sleepers {
    fn default() -> Self {
        Sleepers::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize};
    use std::sync::Arc;

    const IDS: usize = 4096;

    struct Table {
        stack: IdStack,
        link: Vec<AtomicU32>,
        /// 0 idle, 1 queued (the caller-side dedup the stack relies on)
        state: Vec<AtomicU8>,
        pushed: Vec<AtomicUsize>,
        delivered: Vec<AtomicUsize>,
        sleepers: Sleepers,
    }

    fn table() -> Arc<Table> {
        Arc::new(Table {
            stack: IdStack::new(),
            link: (0..IDS).map(|_| AtomicU32::new(0)).collect(),
            state: (0..IDS).map(|_| AtomicU8::new(0)).collect(),
            pushed: (0..IDS).map(|_| AtomicUsize::new(0)).collect(),
            delivered: (0..IDS).map(|_| AtomicUsize::new(0)).collect(),
            sleepers: Sleepers::new(),
        })
    }

    fn produce(t: &Table, id: usize) {
        // the dedup: only the thread that moves idle -> queued pushes
        if t.state[id].compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            t.pushed[id].fetch_add(1, Ordering::Relaxed);
            t.stack.push(id as u32, &t.link[id]);
            t.sleepers.wake_one();
        }
    }

    fn consume(t: &Table) -> usize {
        let mut n = 0;
        IdStack::walk(t.stack.take_all(), |id| t.link[id as usize].load(Ordering::Relaxed), |id| {
            t.delivered[id as usize].fetch_add(1, Ordering::Relaxed);
            // handed out: it may be pushed again
            t.state[id as usize].store(0, Ordering::Release);
            n += 1;
        });
        n
    }

    /// 8 producers push overlapping ids many times over while 2 takers drain: every successful
    /// push is delivered exactly once, nothing is left behind.
    #[test]
    fn treiber_take_all_delivers_every_push_once() {
        let t = table();
        let stop = Arc::new(AtomicBool::new(false));
        let takers: Vec<_> = (0..2)
            .map(|_| {
                let (t, stop) = (t.clone(), stop.clone());
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        if consume(&t) == 0 {
                            std::thread::yield_now();
                        }
                    }
                })
            })
            .collect();
        let producers: Vec<_> = (0..8)
            .map(|p| {
                let t = t.clone();
                std::thread::spawn(move || {
                    for round in 0..200 {
                        for i in 0..IDS / 4 {
                            produce(&t, (i * 7 + p * 131 + round * 17) % IDS);
                        }
                    }
                })
            })
            .collect();
        for p in producers {
            p.join().unwrap();
        }
        stop.store(true, Ordering::Release);
        for k in takers {
            k.join().unwrap();
        }
        consume(&t);
        assert!(t.stack.is_empty());
        let mut total = 0;
        for id in 0..IDS {
            let (p, d) = (t.pushed[id].load(Ordering::Relaxed), t.delivered[id].load(Ordering::Relaxed));
            assert_eq!(p, d, "id {id}: {p} pushes, {d} deliveries");
            total += p;
        }
        assert!(total > IDS, "the test pushed too little to mean anything ({total})");
    }

    /// One consumer that sleeps whenever the stack is empty, producers that push one id at a time
    /// and wake it: a lost wakeup leaves the consumer parked and the main thread's bounded wait fails
    /// (an iteration budget, not a clock).
    #[test]
    fn sleepers_never_lose_a_wakeup() {
        let t = table();
        let total = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let consumer = {
            let (t, total, done) = (t.clone(), total.clone(), done.clone());
            std::thread::spawn(move || {
                t.sleepers.register(0);
                loop {
                    let n = consume(&t);
                    total.fetch_add(n, Ordering::Relaxed);
                    if n == 0 {
                        if done.load(Ordering::Acquire) && t.stack.is_empty() {
                            return;
                        }
                        t.sleepers.sleep(0, || !t.stack.is_empty() || done.load(Ordering::Acquire));
                    }
                }
            })
        };
        const PER: usize = 20_000;
        let producers: Vec<_> = (0..4)
            .map(|p| {
                let t = t.clone();
                std::thread::spawn(move || {
                    for i in 0..PER {
                        let id = (p * PER + i) % IDS;
                        // wait (bounded) until this id was handed out, so each push is a new one
                        let mut spins = 0u64;
                        while t.state[id].load(Ordering::Acquire) != 0 {
                            spins += 1;
                            assert!(spins < 2_000_000_000, "id {id} never handed out: a lost wakeup");
                            std::hint::spin_loop();
                        }
                        produce(&t, id);
                    }
                })
            })
            .collect();
        for p in producers {
            p.join().unwrap();
        }
        done.store(true, Ordering::Release);
        t.sleepers.wake_all();
        consumer.join().unwrap();
        let pushed: usize = t.pushed.iter().map(|x| x.load(Ordering::Relaxed)).sum();
        assert_eq!(total.load(Ordering::Relaxed), pushed);
        assert!(pushed >= PER, "the test pushed too little to mean anything ({pushed})");
    }
}
