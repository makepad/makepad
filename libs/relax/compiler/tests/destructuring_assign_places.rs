//! Destructuring assignment of a place, and overloaded deref of an unsized place.
//! Run with `makepad test libs/relax/compiler/tests/destructuring_assign_places.rs` and with
//! rustc --test (cargo test -p relax-compiler --test destructuring_assign_places): both must agree.

use std::cell::Cell;
use std::ops::{AddAssign, Deref, DerefMut};
use std::rc::Rc;

struct Noisy(&'static str, Rc<Cell<u32>>);

impl Drop for Noisy {
    fn drop(&mut self) {
        self.1.set(self.1.get() + 1);
    }
}

fn mk(tag: &'static str, d: &Rc<Cell<u32>>) -> Noisy {
    Noisy(tag, d.clone())
}

/// `_ = place;` is a no-op on a place (rustc: `let _ = place;` neither moves nor drops it).
#[test]
fn a_wildcard_assignment_of_a_place_neither_moves_nor_drops_it() {
    let d = Rc::new(Cell::new(0));
    {
        let a = mk("a", &d);
        _ = a;
        assert_eq!(d.get(), 0, "still owned after `_ = a`");
        assert_eq!(a.0, "a");
        let pair = (1u8, mk("b", &d));
        _ = pair.1;
        assert_eq!(d.get(), 0);
        assert_eq!(pair.1 .0, "b");
        let boxed = Box::new(mk("c", &d));
        _ = *boxed;
        assert_eq!(d.get(), 0);
        for _ in 0..3 {
            _ = a;
        }
        assert_eq!(d.get(), 0, "a loop of `_ = a` drops nothing");
    }
    assert_eq!(d.get(), 3, "each value dropped once, at the end of its scope");
}

/// Parts matched by `_` / `..` of a place stay owned by it; a leaf assignee moves its part out
/// (a partial move: the rest is dropped with the place, the moved part with its new owner).
#[test]
fn unmatched_parts_of_a_place_stay_with_it_and_moved_leaves_drop_once() {
    let d = Rc::new(Cell::new(0));
    let moved;
    {
        let pt = (mk("x", &d), mk("y", &d));
        let (_, b) = (0, 0);
        let _ = b;
        let b2;
        (b2, _) = pt;
        assert_eq!(d.get(), 0, "`(b2, _) = pt` drops nothing");
        assert_eq!(b2.0, "x");
        assert_eq!(pt.1 .0, "y");
        moved = b2;
    }
    assert_eq!(d.get(), 1, "pt's remaining part dropped with pt; the moved one lives on");
    drop(moved);
    assert_eq!(d.get(), 2);
}

/// A temporary on the right still drops its unmatched parts at the end of the statement.
#[test]
fn unmatched_parts_of_a_temporary_drop_at_the_statement() {
    let d = Rc::new(Cell::new(0));
    let b;
    (_, b) = (mk("t0", &d), mk("t1", &d));
    assert_eq!(d.get(), 1, "the `_` part of the temporary is dropped at once");
    _ = mk("t2", &d);
    assert_eq!(d.get(), 2);
    drop(b);
    assert_eq!(d.get(), 3);
}

trait Tr {
    fn bump(&mut self) -> u32;
    fn peek(&self) -> u32;
}

impl Tr for u32 {
    fn bump(&mut self) -> u32 {
        *self += 1;
        *self
    }
    fn peek(&self) -> u32 {
        *self
    }
}

#[repr(transparent)]
struct W<T: ?Sized>(T);

impl<T: ?Sized> Deref for W<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: ?Sized> DerefMut for W<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<T: Tr + ?Sized> AddAssign<u32> for W<T> {
    fn add_assign(&mut self, n: u32) {
        for _ in 0..n {
            self.0.bump();
        }
    }
}

/// `&W<dyn Trait>` into a `Deref` / `DerefMut` / `op=` impl keeps its vtable (a fat `&self`).
#[test]
fn an_unsized_place_reaches_its_deref_and_op_assign_impls_as_a_fat_reference() {
    let mut w: W<u32> = W(41);
    let r: &mut W<dyn Tr> = &mut w;
    assert_eq!(r.bump(), 42, "autoderef through DerefMut");
    assert_eq!(r.peek(), 42, "autoderef through Deref");
    let inner: &mut dyn Tr = &mut **r;
    assert_eq!(inner.bump(), 43);
    *r += 2;
    assert_eq!(r.peek(), 45);
    let shared: &W<dyn Tr> = &w;
    assert_eq!(shared.peek(), 45);
}
