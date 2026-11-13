// A generic slice indexed by std's `(Bound<usize>, Bound<usize>)` (relax issue 102): the index type is not
// a builtin one, so it selects `Index`/`IndexMut` (`impl<T, I: SliceIndex<[T]>> Index<I> for [T]`) as
// rustc's lookup_indexing does, also inside a macro-generated `Index` impl over a slice newtype.
use std::ops::{Bound, Index, Range};

#[repr(transparent)]
pub struct Slice<T> {
    entries: [T],
}
impl<T> Slice<T> {
    fn from_slice(entries: &[T]) -> &Self {
        // SAFETY: Slice<T> is a transparent wrapper of [T]
        unsafe { &*(entries as *const [T] as *const Self) }
    }
}
macro_rules! impl_index {
    ($($range:ty),*) => {$(
        impl<T> Index<$range> for Slice<T> {
            type Output = Slice<T>;
            fn index(&self, range: $range) -> &Self::Output {
                Slice::from_slice(&self.entries[range])
            }
        }
    )*};
}
impl_index!(Range<usize>, (Bound<usize>, Bound<usize>));

pub fn bounded<T>(s: &mut [T], range: (Bound<usize>, Bound<usize>)) -> &mut [T] {
    &mut s[range]
}

#[test]
fn bound_pairs_index_generic_slices() {
    let mut v = [1u32, 2, 3, 4, 5];
    let s = Slice::from_slice(&v);
    assert_eq!(&s[(Bound::Excluded(0), Bound::Included(2))].entries, &[2, 3]);
    assert_eq!(&s[1..3].entries, &[2, 3]);
    bounded(&mut v, (Bound::Unbounded, Bound::Excluded(2))).fill(0);
    assert_eq!(v, [0, 0, 3, 4, 5]);
}
