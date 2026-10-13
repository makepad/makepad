// A type's TypeId must not move when an edit shifts the text before it: the TypeId is folded
// into code as a constant, and code built before the edit (a stored value's vtable `type_id`)
// and after it (a rebuilt or O3 caller of `TypeId::of`) must agree, else `downcast` of a
// value of that very type fails. The first run stores Payload's type id; v1 adds an item above
// Payload in payload.rs and edits `check`, which folds the type id again: they must be equal.
mod payload;

use std::cell::Cell;

thread_local! {
    pub static STORED: Cell<u64> = Cell::new(0);
}

#[test]
fn run() {
    if STORED.with(|s| s.get()) == 0 {
        let t = unsafe { core::intrinsics_mem::type_id::<payload::Payload>() };
        STORED.with(|s| s.set(t));
    }
    println!("out payload {}", payload::check());
}
