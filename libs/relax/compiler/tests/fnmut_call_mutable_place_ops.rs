//! A call of an FnMut callee reached through an overloaded deref or index borrows it mutably, as
//! under rustc: `DerefMut::deref_mut` and `IndexMut::index_mut` run, never `deref` and `index`;
//! a Fn callee keeps the shared ones. A closure whose kind is known only after its body was
//! checked, called through `RefCell::borrow_mut()`'s guard, was refused (E0596), and a generic
//! FnMut reached through `Deref` ran `deref`.
use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut, Index, IndexMut};

/// A callee behind its own place operators, counting which ran: (shared, mutable).
struct Counted<F> {
    f: F,
    shared: Cell<u32>,
    mutable: Cell<u32>,
}

impl<F> Counted<F> {
    fn new(f: F) -> Self {
        Counted { f, shared: Cell::new(0), mutable: Cell::new(0) }
    }
    fn counts(&self) -> (u32, u32) {
        (self.shared.get(), self.mutable.get())
    }
}

impl<F> Deref for Counted<F> {
    type Target = F;
    fn deref(&self) -> &F {
        self.shared.set(self.shared.get() + 1);
        &self.f
    }
}

impl<F> DerefMut for Counted<F> {
    fn deref_mut(&mut self) -> &mut F {
        self.mutable.set(self.mutable.get() + 1);
        &mut self.f
    }
}

impl<F> Index<usize> for Counted<F> {
    type Output = F;
    fn index(&self, _: usize) -> &F {
        self.shared.set(self.shared.get() + 1);
        &self.f
    }
}

impl<F> IndexMut<usize> for Counted<F> {
    fn index_mut(&mut self, _: usize) -> &mut F {
        self.mutable.set(self.mutable.get() + 1);
        &mut self.f
    }
}

#[test]
fn a_closure_called_through_a_refmut_guard() {
    let mut n = 0;
    let cell = RefCell::new(|| {
        n += 1;
        n
    });
    assert_eq!((cell.borrow_mut())(), 1);
    assert_eq!((cell.borrow_mut())(), 2);
    drop(cell);
    assert_eq!(n, 2);
}

#[test]
fn fn_mut_callees_take_the_mutable_operators() {
    let mut n = 0;
    let mut c = Counted::new(move || {
        n += 1;
        n
    });
    assert_eq!(c(), 1);
    assert_eq!(c[0](), 2);
    assert_eq!(c.counts(), (0, 2), "a closure's FnMut call ran Deref::deref or Index::index");
    // a generic FnMut, whose call's kind typeck knows while it checks the body
    fn through<F: FnMut() -> u32>(c: &mut Counted<F>) -> u32 {
        c() + c[0]()
    }
    assert_eq!(through(&mut c), 3 + 4);
    assert_eq!(c.counts(), (0, 4), "a generic FnMut call ran Deref::deref or Index::index");
}

#[test]
fn fn_callees_keep_the_shared_operators() {
    let c = Counted::new(|| 7);
    assert_eq!(c() + c[0](), 14);
    assert_eq!(c.counts(), (2, 0));
}
