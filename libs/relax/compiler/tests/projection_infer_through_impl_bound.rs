// A projection on a body's inference variable whose impl answers through a where-clause
// projection (`impl<T: Value<Out = U>, U: ?Sized> Indirect for (T,) { type Out = U; }`):
// selecting the impl for `(Wrap<?0>,)` relates ?0 with the impl's fresh `U`. A context-free
// normalization handed that `U` (a variable of its own) back to the body, whose checker
// indexed its variable table with it (typeck.rs shallow_region_vars: index out of bounds,
// a speculative typeck worker's panic in a cold makepad-stage run); the body normalizes it
// in its own inference context, so `Output<_>` is u8.
trait Value { type Out: ?Sized; }
struct Wrap<T: ?Sized>(*const T);
impl<T: ?Sized> Value for Wrap<T> { type Out = T; }
trait Indirect { type Out: ?Sized; }
impl<T: Value<Out = U>, U: ?Sized> Indirect for (T,) { type Out = U; }
type Output<T> = <(Wrap<T>,) as Indirect>::Out;

#[test]
fn projected_annotation() {
    let result: Output<_> = 7u8;
    assert_eq!(result, 7u8);
}

#[test]
fn projected_annotation_unsized() {
    let s: &Output<_> = "relax";
    assert_eq!(s.len(), 5);
    let v: &Output<[u16]> = &[1u16, 2, 3][..];
    assert_eq!(v[2], 3);
}
