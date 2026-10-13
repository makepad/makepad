// Trait selection through impls whose parameter only a where-clause projection fixes
// (`impl<T: Value<Out = U>, U: ?Sized> .. for ..`) while the body still infers the type: for
// `Wrap<?0>` the impl's fresh `U` and the body's ?0 are related. Typeck selects a method in
// the body's own inference context (Fcx::select_trait); the context-free Tcx::select_trait
// sees such a type only as a property (a tuple's Clone candidates, a capture's Copy), whose
// answer is a probe's, and dispatches (impl substitutions) only once the types are known.
use std::ptr;

trait Value { type Out: ?Sized; }
struct Wrap<T: ?Sized>(*const T);
impl<T: ?Sized> Clone for Wrap<T> { fn clone(&self) -> Self { *self } }
impl<T: ?Sized> Copy for Wrap<T> {}
impl<T: ?Sized> Value for Wrap<T> { type Out = T; }

struct Pick<T>(T);
impl<T: Copy + Value<Out = U>, U: ?Sized> Clone for Pick<T> { fn clone(&self) -> Self { *self } }
impl<T: Copy + Value<Out = U>, U: ?Sized> Copy for Pick<T> {}

trait Indirect { fn tag(&self) -> u32; }
impl<T: Value<Out = U>, U: ?Sized> Indirect for (T,) { fn tag(&self) -> u32 { 7 } }

#[test]
fn method_on_inferred_receiver() {
    let t = (Wrap(ptr::null()),);
    assert_eq!(t.tag(), 7);
    let p: *const u8 = t.0 .0;
    assert!(p.is_null());
}

#[test]
fn tuple_clone_of_inferred_pick() {
    let t = (Pick(Wrap(ptr::null())),);
    let c: (Pick<Wrap<u16>>,) = t.clone();
    assert!(c.0 .0 .0.is_null());
}

#[test]
fn capture_of_projection_bound_copy() {
    let p = Pick(Wrap(ptr::null()));
    let f = move || p;
    let q: Pick<Wrap<u32>> = f();
    let again = p;
    assert!(q.0 .0.is_null() && again.0 .0.is_null());
}
