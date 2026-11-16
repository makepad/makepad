// An FnMut closure called through `RefCell::borrow()`'s shared guard cannot be borrowed mutably,
// as under rustc (E0596): a call that takes its receiver through `DerefMut` only where the
// receiver has it (fnmut_call_mutable_place_ops.rs) does not accept a shared one.
// rejects: E0596
use std::cell::RefCell;

pub fn call_through_a_shared_guard() -> u32 {
    let mut n = 0;
    let cell = RefCell::new(|| {
        n += 1;
        n
    });
    let r = (cell.borrow())();
    r
}
