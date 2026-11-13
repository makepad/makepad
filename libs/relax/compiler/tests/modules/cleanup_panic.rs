// A module's frames unwind and their own cleanup fails: the call ends with an error, never the
// process. Two ways in:
// - fuel is spent in a callee while the caller holds a value with a destructor: the caller's
//   cleanup pad is a twin's block like any other, it pays before it runs, and it cannot pay;
// - a destructor of the module panics while the call unwinds from a panic.
// For host code either is a panic during cleanup, and the process aborts as rustc's does. Here
// the frame's cleanup ends and the unwind goes on: the host frames above still run theirs, the
// first panic is the one the caller gets, and the module can be called again.
use std::cell::Cell;
use std::relax::{Budget, BudgetError, Module};

const CAP: u64 = 16 * 1024 * 1024;
const SOURCE: &str = r#"
pub fn spin(n: u64) -> u64 {
    let keep = vec![1u8; 1024];
    let mut x = 1u64;
    let mut i = 0;
    while i < n {
        x = x.wrapping_mul(3).wrapping_add(keep[(x & 1023) as usize] as u64);
        i += 1;
    }
    x
}
// (run-time checks: their messages reach the host, so it can tell the two panics apart)
pub struct Bomb(pub usize);
impl Drop for Bomb {
    fn drop(&mut self) {
        let one = [0u8; 1];
        std::hint::black_box(one[self.0]);
    }
}
fn fail(n: usize) -> usize {
    let three = [1u8, 2, 3];
    three[n * 9] as usize
}
pub fn bombs(n: usize) -> usize {
    let _a = Bomb(n);
    let _b = Bomb(n);
    fail(n) + 1
}
"#;

struct Mark<'a>(&'a Cell<u32>);
impl Drop for Mark<'_> {
    fn drop(&mut self) { self.0.set(self.0.get() + 1); }
}

#[test]
fn cleanup_panic() {
    let m = Module::load_source("unwinder", "unwinder.rs", SOURCE, &[]).unwrap();
    let spin: fn(u64) -> u64 = m.get("spin").unwrap();
    let bombs: fn(usize) -> usize = m.get("bombs").unwrap();
    let host_drops = Cell::new(0);

    let mut budget = Budget::new(200_000, CAP);
    let short = m.call_with(&mut budget, || spin(100)).unwrap();
    // (the budget's last unit falls in a different block each time: in `spin` itself, which has
    // no cleanup to run there, or in a function it calls while `keep` is alive, whose unwind
    // enters spin's cleanup pad)
    let mut stopped = 0;
    for extra in 0..32 {
        budget.reset(200_000 + extra, CAP);
        let long = m.call_with(&mut budget, || { let _mark = Mark(&host_drops); spin(100_000_000) });
        stopped += matches!(long, Err(BudgetError::Fuel { consumed }) if consumed == 200_000 + extra) as u32;
    }
    println!("out fuel spent under a destructor: {} of 32 calls stopped", stopped);
    budget.reset(200_000, CAP);
    println!("out callable after fuel: {}", m.call_with(&mut budget, || spin(100)).ok() == Some(short));

    println!("out no bombs: {:?}", std::relax::guard(0, || bombs(0)));
    let r = std::relax::guard(0, || { let _mark = Mark(&host_drops); bombs(1) });
    println!("out destructor panics while unwinding: {:?}", r);
    println!("out callable after bombs: {:?}", std::relax::guard(0, || bombs(0)));
    println!("out host frames cleaned up: {}", host_drops.get());
    m.unload();
}
