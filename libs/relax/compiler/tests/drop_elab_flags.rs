// Drop elaboration at compile time (`makepad test`; rustc: every test passes). Lowering's drop flags
// are decided statically where the dataflow knows them (rustc elaborate_drops: Static / Dead) and
// stay runtime flags only where a place is maybe initialized (Conditional) or partly moved on
// some paths (Open). Each case has a path where the flag must stay dynamic next to one where it
// is known, and the unwind path (a panic between them) must see the same state.
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

thread_local! { static LOG: RefCell<Vec<u32>> = RefCell::new(Vec::new()); }
fn take() -> Vec<u32> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }
struct D(u32);
impl Drop for D { fn drop(&mut self) { LOG.with(|l| l.borrow_mut().push(self.0)); } }
fn sink(d: D) -> u32 { d.0 }
fn boom(p: bool) { if p { panic!("boom"); } }

// maybe moved: the scope end tests the flag
fn cond_move(c: bool) -> u32 { let a = D(1); let b = D(2); if c { sink(a); } b.0 }
// moved on every path: no drop at the end
fn both_move(c: bool) -> u32 { let a = D(1); if c { sink(a) } else { sink(a) + 10 } }
// moved and assigned again in a loop: initialized at the loop head on every path, moved in between
fn loop_reinit(n: u32) -> u32 { let mut a = D(100); for i in 0..n { if i % 2 == 0 { sink(a); a = D(i); } } a.0 }
// a field moved out on one path (open drop: the other field alone, else the whole value)
#[allow(dead_code)]
struct P(D, D);
fn part_move(c: bool) -> u32 { let p = P(D(1), D(2)); if c { sink(p.0); } 7 }
// enum payload moved out on one path
fn enum_move(c: bool) -> u32 { let o = Some(D(5)); if c { if let Some(d) = o { return sink(d); } } 0 }
// a panic after a conditional move: the cleanup drops what is still owned
fn panic_after(c: bool) { let a = D(1); let _b = D(2); if c { sink(a); } boom(true); }
// temporaries: one with a maybe flag (only evaluated on one side of `&&`)
fn temp_cond(c: bool) -> bool { c && D(9).0 == 9 }

#[test] fn conditional_move() {
    take();
    assert_eq!(cond_move(true), 2); assert_eq!(take(), vec![1, 2]);
    assert_eq!(cond_move(false), 2); assert_eq!(take(), vec![2, 1]);
}
#[test] fn moved_on_both_paths() {
    take();
    assert_eq!(both_move(true), 1); assert_eq!(take(), vec![1]);
    assert_eq!(both_move(false), 11); assert_eq!(take(), vec![1]);
}
#[test] fn loop_reinitialisation() {
    take();
    assert_eq!(loop_reinit(4), 2); assert_eq!(take(), vec![100, 0, 2]);
    assert_eq!(loop_reinit(0), 100); assert_eq!(take(), vec![100]);
}
#[test] fn partial_move() {
    take();
    assert_eq!(part_move(true), 7); assert_eq!(take(), vec![1, 2]);
    assert_eq!(part_move(false), 7); assert_eq!(take(), vec![1, 2]);
}
#[test] fn enum_payload_move() {
    take();
    assert_eq!(enum_move(true), 5); assert_eq!(take(), vec![5]);
    assert_eq!(enum_move(false), 0); assert_eq!(take(), vec![5]);
}
#[test] fn unwind_after_conditional_move() {
    take();
    assert!(catch_unwind(AssertUnwindSafe(|| panic_after(true))).is_err()); assert_eq!(take(), vec![1, 2]);
    assert!(catch_unwind(AssertUnwindSafe(|| panic_after(false))).is_err()); assert_eq!(take(), vec![2, 1]);
}
#[test] fn conditional_temporary() {
    take();
    assert!(temp_cond(true)); assert_eq!(take(), vec![9]);
    assert!(!temp_cond(false)); assert_eq!(take(), Vec::<u32>::new());
}
