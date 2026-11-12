// A generic body whose local has a field typed by a projection over a const parameter
// (`W<S<N>>` with `struct W<T: Tr>(T::A)`): the liveness lint asks whether the local needs
// drop, in the body's own predicates, where `N: usize` selects `impl<const N: usize> Tr for S<N>`
// (relax issue 8: `makepad check` stopped with an internal error).
trait Tr { type A; }
struct S<const N: usize>;
impl<const N: usize> Tr for S<N> { type A = [u8; N]; }
struct V<const N: usize>;
impl<const N: usize> Tr for V<N> { type A = Vec<[u8; N]>; }
struct W<T: Tr>(T::A);

fn len<const N: usize>(w: W<S<N>>) -> usize { w.0.len() }
fn moved<const N: usize>(w: W<S<N>>) -> usize { let _a = w.0; N }
fn split<const N: usize>(w: W<S<N>>) -> usize { let W(a) = w; a.len() }
fn owned<const N: usize>(w: W<V<N>>) -> usize { let mut w = w; w.0.push([0; N]); w.0.len() * N }

#[test]
fn const_param_projection_fields() {
    assert_eq!(len(W::<S<2>>([1, 2])), 2, "issue form");
    assert_eq!(moved(W::<S<3>>([0; 3])), 3, "the field moved out");
    assert_eq!(split(W::<S<4>>([0; 4])), 4, "a tuple-struct pattern");
    assert_eq!(owned(W::<V<2>>(vec![[1, 2]])), 4, "a field with drop glue");
}
