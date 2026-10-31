// rustc_middle/src/ty/instance.rs:955-992: instantiate before normalizing.
trait Source { type Item; }
struct A;
struct B;
impl Source for A { type Item = u32; }
impl Source for B { type Item = u64; }
#[inline(never)]
fn wrap<T: Source>(x: T::Item) -> Option<T::Item> { Some(x) }
#[test]
fn rigid_projection_in_generic_body() {
    assert_eq!(wrap::<A>(7), Some(7u32));
    assert_eq!(wrap::<B>(11), Some(11u64));
}

#[inline(never)]
fn slice_part<I: std::slice::SliceIndex<[u8]>>(s: &mut [u8], i: I) -> &mut I::Output {
    &mut s[i]
}
#[test]
fn projected_index_preserves_slice_metadata() {
    let mut bytes = [1, 2, 3, 4];
    let part = slice_part(&mut bytes, 1..3);
    assert_eq!(part.len(), 2);
    part[1] = 9;
    assert_eq!(bytes[2], 9);
}

#[inline(never)]
fn generic_assign<T: std::ops::AddAssign<T>>(values: &mut [T], rhs: T, order: &std::cell::Cell<u8>) {
    values[{ order.set(1); 0 }] += { assert_eq!(order.get(), 1); rhs };
}

#[test]
fn generic_compound_assignment_keeps_declared_evaluation_order() {
    let order = std::cell::Cell::new(0);
    let mut values = [1u32];
    generic_assign(&mut values, 2, &order);
    assert_eq!(values[0], 3);
    order.set(0);
    values[{ order.set(1); 0 }] += { assert_eq!(order.get(), 0); 4 };
    assert_eq!(values[0], 7);
}
