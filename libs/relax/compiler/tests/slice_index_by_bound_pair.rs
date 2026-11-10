// A pair of bounds indexes a slice in an index expression, as it does through `get` and through
// `Index::index`: core's `impl SliceIndex<[T]> for (Bound<usize>, Bound<usize>)` (indexmap:
// `&self.entries[range]` with `range: (Bound<usize>, Bound<usize>)`).
use core::ops::{Bound, Index};
use core::slice::SliceIndex;

fn through_a_bound<I: SliceIndex<[i32], Output = [i32]>>(v: &[i32], i: I) -> &[i32] {
    &v[i]
}

#[test]
fn the_impl_is_selected_by_a_call() {
    let v = [10, 20, 30, 40, 50];
    let s: &[i32] = &v[..];
    let b: (Bound<usize>, Bound<usize>) = (Bound::Excluded(0), Bound::Included(3));
    assert_eq!(s.get(b), Some(&[20, 30, 40][..]));
    assert_eq!(through_a_bound(s, b), &[20, 30, 40]);
    assert_eq!(Index::index(s, b), &[20, 30, 40]);
}

#[test]
fn the_index_expression() {
    let mut v = [10, 20, 30, 40, 50];
    let b: (Bound<usize>, Bound<usize>) = (Bound::Excluded(0), Bound::Included(3));
    {
        let s: &[i32] = &v[..];
        assert_eq!(&s[b], &[20, 30, 40]);
        assert_eq!(&s[(Bound::Unbounded, Bound::Excluded(2))], &[10, 20]);
    }
    // an array unsizes to the slice the impl is for, and the place is mutable
    assert_eq!(&v[(Bound::Included(4), Bound::Unbounded)], &[50]);
    v[b][0] = 21;
    let vec = v.to_vec();
    assert_eq!(&vec[b], &[21, 30, 40]);
}
