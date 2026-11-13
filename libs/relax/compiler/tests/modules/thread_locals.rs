// Thread-locals in sandboxed code: each thread's entry has its own, every claim (a thread's
// first frame in the module) starts them from their initialisers, and their destructors run
// when the claim ends. All three kinds: a `#[thread_local]` static (const, no drop glue), a
// lazy runtime key without drop glue and one with a destructor; and std::thread::current().
// Run with RELAX_SANDBOX_NOCHECK=1 (the module names std::thread and std::sync, which the
// module scan refuses).
use std::relax::Module;

const M: &str = r#"
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

pub static DROPS: AtomicU64 = AtomicU64::new(0);
static ARRIVED: AtomicU64 = AtomicU64::new(0);

pub struct Counted(u64);
impl Drop for Counted {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0, Ordering::SeqCst);
    }
}

thread_local! {
    static FAST: Cell<u64> = const { Cell::new(7) };
    static LAZY: Cell<u64> = Cell::new(1000);
    static OWNED: Counted = Counted(1);
}

/// Counts `n` on two thread-locals, then waits until `2 * round` calls have counted (the other
/// thread's call is inside meanwhile): FAST, LAZY and this thread's id, packed.
pub fn bump(n: u64, round: u64) -> u64 {
    for _ in 0..n {
        FAST.with(|c| c.set(c.get() + 1));
        LAZY.with(|c| c.set(c.get() + 1));
    }
    OWNED.with(|_| ());
    ARRIVED.fetch_add(1, Ordering::SeqCst);
    while ARRIVED.load(Ordering::SeqCst) < 2 * round {
        std::hint::spin_loop();
    }
    let id = std::thread::current().id().as_u64().get();
    FAST.with(|c| c.get()) * 1_000_000_000 + LAZY.with(|c| c.get()) * 1_000 + id
}

pub fn drops() -> u64 {
    DROPS.load(Ordering::SeqCst)
}
"#;

#[test]
fn thread_locals_per_claim() {
    let m = Module::load_source("tls_twins", "tls_twins.rs", M, &[]).unwrap();
    let bump: fn(u64, u64) -> u64 = m.get("bump").unwrap();
    let drops: fn() -> u64 = m.get("drops").unwrap();
    let unpack = |v: u64| (v / 1_000_000_000, v / 1_000 % 1_000_000, v % 1_000);
    let mut rounds = Vec::new();
    for round in 1..=2 {
        let a = std::thread::spawn(move || bump(5, round));
        let b = std::thread::spawn(move || bump(3, round));
        rounds.push((unpack(a.join().unwrap()), unpack(b.join().unwrap())));
    }
    let (((fa, la, ia), (fb, lb, ib)), ((fa2, la2, _), (fb2, lb2, _))) = (rounds[0], rounds[1]);
    println!("out each thread its own: {}", (fa, la, fb, lb) == (12, 1005, 10, 1003));
    println!("out thread ids differ: {}", ia != ib);
    println!("out initialised per claim: {}", (fa2, la2, fb2, lb2) == (12, 1005, 10, 1003));
    println!("out destructors at release: {}", drops());
    m.unload();
}
