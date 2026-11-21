// The stale direction where a frame keeps the copy (run.py's stale_* restart rows): a thread runs
// for ever inside code that holds a copy of what an edit changes, so the edit restarts: the frame
// still runs the code from before it. Each row edits one item here.
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Duration;

// stale_loop_copy: a reader with its own loop, inlined into a thread's closure
pub const F3_LOOP_K: u64 = 1;
pub static F3_LOOP_SEEN: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn f3_loop_reader() {
    loop {
        F3_LOOP_SEEN.store(F3_LOOP_K, Relaxed);
        std::thread::sleep(Duration::from_millis(5));
    }
}

// stale_blocked_copy: a host blocked in a call inside its copy: it waits there for ever, holding
// what it read before the edit. black_box uses the copy after the call, so the call keeps the
// host's frame (no sibling call), as rustc's black_box keeps it. The copy is the host's own: an
// inlined reader's was only when its build inlined it (a sibling call when it did, while
// black_box was an identity: the frame was gone, and a stale value never asked for a restart)
pub const F3_BLOCK_K: u64 = 1;
pub static F3_BLOCK_SEEN: AtomicU64 = AtomicU64::new(0);

#[inline(never)]
fn f3_wait() {
    loop {
        std::thread::park();
    }
}

#[inline(never)]
fn f3_block_host() {
    let k = F3_BLOCK_K;
    F3_BLOCK_SEEN.store(k, Relaxed);
    f3_wait();
    black_box(k);
}

// stale_std_loop: an associated const read by a std instance's loop: a thread drives std's
// `for_each` over an iterator whose `next`, inlined into it, reads the const
pub trait F3Kind {
    const K: u64;
}

pub struct F3K;

impl F3Kind for F3K {
    const K: u64 = 1;
}

pub static F3_STD_SEEN: AtomicU64 = AtomicU64::new(0);

pub struct F3It<T>(std::marker::PhantomData<T>);

impl<T: F3Kind> Iterator for F3It<T> {
    type Item = u64;
    #[inline(always)]
    fn next(&mut self) -> Option<u64> {
        F3_STD_SEEN.store(T::K, Relaxed);
        std::thread::sleep(Duration::from_millis(5));
        Some(T::K)
    }
}

#[inline(never)]
fn f3_std_loop() {
    F3It::<F3K>(std::marker::PhantomData).for_each(|v| {
        black_box(v);
    });
}

// stale_tiered_frame: a thread's loop host entered before its tier-up (run.py asks for its O3
// build before the edit): the frame runs the retired version, which holds the copy
pub const F4_K: u64 = 1;
pub static F4_SEEN: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
fn f4_leaf() -> u64 {
    F4_K
}

#[inline(never)]
pub fn f4_loop() {
    loop {
        F4_SEEN.store(f4_leaf(), Relaxed);
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn row() -> Vec<(&'static str, u64)> {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        std::thread::spawn(|| f3_loop_reader());
        std::thread::spawn(f3_block_host);
        std::thread::spawn(f3_std_loop);
        std::thread::spawn(f4_loop);
        // (the rows read what those threads stored)
        let seen = [&F3_LOOP_SEEN, &F3_BLOCK_SEEN, &F3_STD_SEEN, &F4_SEEN];
        while seen.iter().any(|s| s.load(Relaxed) == 0) {
            std::thread::yield_now();
        }
    });
    vec![
        ("stale_loop_copy", F3_LOOP_SEEN.load(Relaxed)),
        ("stale_blocked_copy", F3_BLOCK_SEEN.load(Relaxed)),
        ("stale_std_loop", F3_STD_SEEN.load(Relaxed)),
        ("stale_tiered_frame", F4_SEEN.load(Relaxed)),
    ]
}
