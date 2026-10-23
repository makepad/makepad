//! `impl<T> [T]` and `impl [u8]` (Relax only: inherent impls on primitive types). Bodies
//! forward to the free functions of this module tree, which the rustc shim checks.

use super::iter as it;
use super::sort;
use super::{
    ChunkBy, ChunkByMut, Chunks, ChunksExact, ChunksExactMut, ChunksMut, Iter, IterMut, RChunks,
    RChunksExact, RChunksExactMut, RChunksMut, RSplit, RSplitMut, RSplitN, RSplitNMut, Split,
    SplitInclusive, SplitInclusiveMut, SplitMut, SplitN, SplitNMut, Windows,
};
use super::{Concat, Join, SliceIndex};
use crate::cmp::Ordering;
use crate::intrinsics_mem as im;
use crate::vec::Vec;

impl<T> [T] {
    /// Real core's `align_to`: the longest middle slice of `U` whose start is aligned for `U`
    /// and whose end falls on a `T` boundary (slice/mod.rs align_to + align_to_offsets).
    pub unsafe fn align_to<U>(&self) -> (&[T], &[U], &[T]) {
        let (t, u) = (crate::mem::size_of::<T>(), crate::mem::size_of::<U>());
        if t == 0 || u == 0 {
            return (self, &[], &[]);
        }
        let offset = self.as_ptr().align_offset(crate::mem::align_of::<U>());
        if offset > self.len() {
            return (self, &[], &[]);
        }
        let (left, rest) = self.split_at(offset);
        let (us_len, ts_len) = align_to_offsets(t, u, rest.len());
        let mid = crate::slice::from_raw_parts(rest.as_ptr() as *const U, us_len);
        let right = &rest[rest.len() - ts_len..];
        (left, mid, right)
    }
    pub unsafe fn align_to_mut<U>(&mut self) -> (&mut [T], &mut [U], &mut [T]) {
        let (t, u) = (crate::mem::size_of::<T>(), crate::mem::size_of::<U>());
        if t == 0 || u == 0 {
            return (self, &mut [], &mut []);
        }
        let offset = self.as_ptr().align_offset(crate::mem::align_of::<U>());
        if offset > self.len() {
            return (self, &mut [], &mut []);
        }
        let (left, rest) = self.split_at_mut(offset);
        let rest_len = rest.len();
        let (us_len, ts_len) = align_to_offsets(t, u, rest_len);
        let ptr = rest.as_mut_ptr();
        let mid = crate::slice::from_raw_parts_mut(ptr as *mut U, us_len);
        let right = crate::slice::from_raw_parts_mut(ptr.add(rest_len - ts_len), ts_len);
        (left, mid, right)
    }
    #[cfg_attr(not(relax_check), lang = "slice_len_fn")]
    pub const fn len(&self) -> usize {
        unsafe { im::slice_len(self) }
    }
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub const fn first(&self) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            Some(&self[0])
        }
    }
    pub const fn first_mut(&mut self) -> Option<&mut T> {
        if self.is_empty() {
            None
        } else {
            Some(&mut self[0])
        }
    }
    pub const fn last(&self) -> Option<&T> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some(&self[n - 1])
        }
    }
    pub const fn last_mut(&mut self) -> Option<&mut T> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some(&mut self[n - 1])
        }
    }
    pub const fn split_first(&self) -> Option<(&T, &[T])> {
        if self.is_empty() {
            None
        } else {
            Some((&self[0], &self[1..]))
        }
    }
    pub const fn split_first_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        if self.is_empty() {
            None
        } else {
            let (a, b) = super::split_at_mut(self, 1);
            Some((&mut a[0], b))
        }
    }
    pub const fn split_last(&self) -> Option<(&T, &[T])> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some((&self[n - 1], &self[..n - 1]))
        }
    }
    pub const fn split_last_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            let (a, b) = super::split_at_mut(self, n - 1);
            Some((&mut b[0], a))
        }
    }
    pub const fn get<I: SliceIndex<[T]>>(&self, index: I) -> Option<&I::Output> {
        index.get(self)
    }
    pub fn get_disjoint_mut<I, const N: usize>(&mut self, indices: [I; N]) -> Result<[&mut I::Output; N], super::GetDisjointMutError>
    where
        I: super::GetDisjointMutIndex + SliceIndex<[T]>,
    {
        super::get_disjoint_mut(self, indices)
    }
    #[track_caller]
    pub unsafe fn get_disjoint_unchecked_mut<I, const N: usize>(&mut self, indices: [I; N]) -> [&mut I::Output; N]
    where
        I: super::GetDisjointMutIndex + SliceIndex<[T]>,
    {
        super::get_disjoint_unchecked_mut(self, indices)
    }
    pub fn get_mut<I: SliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output> {
        index.get_mut(self)
    }
    pub const unsafe fn get_unchecked<I: SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        &*index.get_unchecked(self)
    }
    pub const unsafe fn get_unchecked_mut<I: SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        &mut *index.get_unchecked_mut(self)
    }
    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    pub const fn as_ptr(&self) -> *const T {
        self as *const [T] as *const T
    }
    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut [T] as *mut T
    }
    pub const fn as_ptr_range(&self) -> crate::ops::Range<*const T> {
        let start = self.as_ptr();
        let end = unsafe { im::ptr_add(start, self.len()) };
        start..end
    }
    #[track_caller]
    pub fn swap(&mut self, a: usize, b: usize) {
        super::swap(self, a, b)
    }
    pub const fn reverse(&mut self) {
        super::reverse(self)
    }
    pub const fn iter(&self) -> Iter<'_, T> {
        Iter::new(self)
    }
    pub const fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut::new(self)
    }
    #[track_caller]
    pub const fn windows(&self, size: usize) -> Windows<'_, T> {
        it::windows(self, size)
    }
    #[track_caller]
    pub const fn chunks(&self, chunk_size: usize) -> Chunks<'_, T> {
        it::chunks(self, chunk_size)
    }
    #[track_caller]
    pub const fn chunks_mut(&mut self, chunk_size: usize) -> ChunksMut<'_, T> {
        it::chunks_mut(self, chunk_size)
    }
    #[track_caller]
    pub const fn chunks_exact(&self, chunk_size: usize) -> ChunksExact<'_, T> {
        it::chunks_exact(self, chunk_size)
    }
    #[track_caller]
    pub const fn chunks_exact_mut(&mut self, chunk_size: usize) -> ChunksExactMut<'_, T> {
        it::chunks_exact_mut(self, chunk_size)
    }
    #[track_caller]
    pub const fn rchunks(&self, chunk_size: usize) -> RChunks<'_, T> {
        it::rchunks(self, chunk_size)
    }
    #[track_caller]
    pub const fn rchunks_mut(&mut self, chunk_size: usize) -> RChunksMut<'_, T> {
        it::rchunks_mut(self, chunk_size)
    }
    #[track_caller]
    pub const fn rchunks_exact(&self, chunk_size: usize) -> RChunksExact<'_, T> {
        it::rchunks_exact(self, chunk_size)
    }
    #[track_caller]
    pub const fn rchunks_exact_mut(&mut self, chunk_size: usize) -> RChunksExactMut<'_, T> {
        it::rchunks_exact_mut(self, chunk_size)
    }
    pub const fn chunk_by<P: FnMut(&T, &T) -> bool>(&self, pred: P) -> ChunkBy<'_, T, P> {
        it::chunk_by(self, pred)
    }
    pub const fn chunk_by_mut<P: FnMut(&T, &T) -> bool>(&mut self, pred: P) -> ChunkByMut<'_, T, P> {
        it::chunk_by_mut(self, pred)
    }
    pub fn split_inclusive_mut<P: FnMut(&T) -> bool>(
        &mut self,
        pred: P,
    ) -> SplitInclusiveMut<'_, T, P> {
        it::split_inclusive_mut(self, pred)
    }
    pub fn splitn_mut<P: FnMut(&T) -> bool>(&mut self, n: usize, pred: P) -> SplitNMut<'_, T, P> {
        it::splitn_mut(self, n, pred)
    }
    pub fn rsplit_mut<P: FnMut(&T) -> bool>(&mut self, pred: P) -> RSplitMut<'_, T, P> {
        it::rsplit_mut(self, pred)
    }
    pub fn rsplitn_mut<P: FnMut(&T) -> bool>(&mut self, n: usize, pred: P) -> RSplitNMut<'_, T, P> {
        it::rsplitn_mut(self, n, pred)
    }
    #[track_caller]
    pub const fn split_at(&self, mid: usize) -> (&[T], &[T]) {
        if mid > self.len() {
            crate::panicking::panic_str("mid > len");
        }
        (&self[..mid], &self[mid..])
    }
    #[track_caller]
    pub const fn split_at_mut(&mut self, mid: usize) -> (&mut [T], &mut [T]) {
        super::split_at_mut(self, mid)
    }
    pub const fn split_at_checked(&self, mid: usize) -> Option<(&[T], &[T])> {
        if mid > self.len() {
            None
        } else {
            Some((&self[..mid], &self[mid..]))
        }
    }
    pub fn split<F: FnMut(&T) -> bool>(&self, pred: F) -> Split<'_, T, F> {
        it::split(self, pred)
    }
    pub fn split_mut<F: FnMut(&T) -> bool>(&mut self, pred: F) -> SplitMut<'_, T, F> {
        it::split_mut(self, pred)
    }
    pub fn split_inclusive<F: FnMut(&T) -> bool>(&self, pred: F) -> SplitInclusive<'_, T, F> {
        it::split_inclusive(self, pred)
    }
    pub fn splitn<F: FnMut(&T) -> bool>(&self, n: usize, pred: F) -> SplitN<'_, T, F> {
        it::splitn(self, n, pred)
    }
    pub fn rsplit<F: FnMut(&T) -> bool>(&self, pred: F) -> RSplit<'_, T, F> {
        it::rsplit(self, pred)
    }
    pub fn rsplitn<F: FnMut(&T) -> bool>(&self, n: usize, pred: F) -> RSplitN<'_, T, F> {
        it::rsplitn(self, n, pred)
    }
    pub fn contains(&self, x: &T) -> bool
    where
        T: PartialEq,
    {
        super::contains(self, x)
    }
    pub fn starts_with(&self, needle: &[T]) -> bool
    where
        T: PartialEq,
    {
        super::starts_with(self, needle)
    }
    pub fn ends_with(&self, needle: &[T]) -> bool
    where
        T: PartialEq,
    {
        super::ends_with(self, needle)
    }
    pub fn strip_prefix(&self, prefix: &[T]) -> Option<&[T]>
    where
        T: PartialEq,
    {
        if super::starts_with(self, prefix) {
            Some(&self[prefix.len()..])
        } else {
            None
        }
    }
    pub fn strip_suffix(&self, suffix: &[T]) -> Option<&[T]>
    where
        T: PartialEq,
    {
        if super::ends_with(self, suffix) {
            Some(&self[..self.len() - suffix.len()])
        } else {
            None
        }
    }
    pub fn binary_search(&self, x: &T) -> Result<usize, usize>
    where
        T: Ord,
    {
        super::binary_search_by(self, |p| T::cmp(p, x))
    }
    pub fn binary_search_by<'a, F: FnMut(&'a T) -> Ordering>(&'a self, f: F) -> Result<usize, usize> {
        super::binary_search_by(self, f)
    }
    pub fn binary_search_by_key<'a, B: Ord, F: FnMut(&'a T) -> B>(&'a self, b: &B, f: F) -> Result<usize, usize> {
        let mut f = f;
        super::binary_search_by(self, |k| B::cmp(&f(k), b))
    }
    pub fn partition_point<P: FnMut(&T) -> bool>(&self, pred: P) -> usize {
        super::partition_point(self, pred)
    }
    pub fn sort(&mut self)
    where
        T: Ord,
    {
        sort::merge_sort(self, &mut |a: &T, b: &T| T::lt(a, b))
    }
    pub fn sort_by<F: FnMut(&T, &T) -> Ordering>(&mut self, compare: F) {
        let mut compare = compare;
        sort::merge_sort(self, &mut |a: &T, b: &T| compare(a, b) == Ordering::Less)
    }
    pub fn sort_by_key<K: Ord, F: FnMut(&T) -> K>(&mut self, f: F) {
        let mut f = f;
        sort::merge_sort(self, &mut |a: &T, b: &T| K::lt(&f(a), &f(b)))
    }
    pub fn sort_by_cached_key<K: Ord, F: FnMut(&T) -> K>(&mut self, f: F) {
        sort::sort_by_cached_key(self, f)
    }
    pub fn sort_unstable(&mut self)
    where
        T: Ord,
    {
        sort::quicksort(self, &mut |a: &T, b: &T| T::lt(a, b))
    }
    pub fn sort_unstable_by<F: FnMut(&T, &T) -> Ordering>(&mut self, compare: F) {
        let mut compare = compare;
        sort::quicksort(self, &mut |a: &T, b: &T| compare(a, b) == Ordering::Less)
    }
    pub fn sort_unstable_by_key<K: Ord, F: FnMut(&T) -> K>(&mut self, f: F) {
        let mut f = f;
        sort::quicksort(self, &mut |a: &T, b: &T| K::lt(&f(a), &f(b)))
    }
    #[track_caller]
    pub fn select_nth_unstable(&mut self, index: usize) -> (&mut [T], &mut T, &mut [T])
    where
        T: Ord,
    {
        sort::select_nth(self, index, &mut |a: &T, b: &T| T::lt(a, b));
        let (left, rest) = super::split_at_mut(self, index);
        let (mid, right) = super::split_at_mut(rest, 1);
        (left, &mut mid[0], right)
    }
    #[track_caller]
    pub fn select_nth_unstable_by<F: FnMut(&T, &T) -> Ordering>(&mut self, index: usize, compare: F) -> (&mut [T], &mut T, &mut [T]) {
        let mut compare = compare;
        sort::select_nth(self, index, &mut |a: &T, b: &T| compare(a, b) == Ordering::Less);
        let (left, rest) = super::split_at_mut(self, index);
        let (mid, right) = super::split_at_mut(rest, 1);
        (left, &mut mid[0], right)
    }
    pub fn is_sorted(&self) -> bool
    where
        T: PartialOrd,
    {
        self.is_sorted_by(|a, b| a <= b)
    }
    pub fn is_sorted_by<'a, F>(&'a self, compare: F) -> bool
    where
        F: FnMut(&'a T, &'a T) -> bool,
    {
        let mut compare = compare;
        let mut i = 1;
        while i < self.len() {
            if !compare(&self[i - 1], &self[i]) {
                return false;
            }
            i += 1;
        }
        true
    }
    pub fn is_sorted_by_key<'a, F, K>(&'a self, f: F) -> bool
    where
        F: FnMut(&'a T) -> K,
        K: PartialOrd,
    {
        let mut f = f;
        let mut i = 1;
        while i < self.len() {
            if !(f(&self[i - 1]) <= f(&self[i])) {
                return false;
            }
            i += 1;
        }
        true
    }
    #[track_caller]
    pub const fn rotate_left(&mut self, mid: usize) {
        super::rotate_left(self, mid)
    }
    #[track_caller]
    pub const fn rotate_right(&mut self, k: usize) {
        super::rotate_right(self, k)
    }
    pub fn fill(&mut self, value: T)
    where
        T: Clone,
    {
        super::fill(self, value)
    }
    pub fn fill_with<F: FnMut() -> T>(&mut self, f: F) {
        let mut f = f;
        let mut i = 0;
        while i < self.len() {
            self[i] = f();
            i += 1;
        }
    }
    #[track_caller]
    pub const fn copy_from_slice(&mut self, src: &[T])
    where
        T: Copy,
    {
        super::copy_from_slice(self, src)
    }
    #[track_caller]
    pub const fn clone_from_slice(&mut self, src: &[T])
    where
        T: Clone,
    {
        super::clone_from_slice(self, src)
    }
    #[track_caller]
    pub fn copy_within<R: crate::ops::RangeBounds<usize>>(&mut self, src: R, dest: usize)
    where
        T: Copy,
    {
        super::copy_within(self, src, dest)
    }
    #[track_caller]
    pub const fn swap_with_slice(&mut self, other: &mut [T]) {
        if self.len() != other.len() {
            crate::panicking::panic_str("destination and source slices have different lengths");
        }
        let mut i = 0;
        while i < self.len() {
            crate::mem::swap(&mut self[i], &mut other[i]);
            i += 1;
        }
    }
    pub fn to_vec(&self) -> Vec<T>
    where
        T: Clone,
    {
        super::to_vec(self)
    }
    pub fn into_vec(self: crate::boxed::Box<[T]>) -> Vec<T> {
        let len = self.len();
        let p = crate::boxed::Box::into_raw(self) as *mut T;
        unsafe { Vec::from_raw_parts(p, len, len) }
    }
    pub fn repeat(&self, n: usize) -> Vec<T>
    where
        T: Copy,
    {
        super::repeat(self, n)
    }
    pub fn concat<Item: ?Sized>(&self) -> <[T] as Concat<Item>>::Output
    where
        [T]: Concat<Item>,
    {
        Concat::concat(self)
    }
    pub fn join<Separator>(&self, sep: Separator) -> <[T] as Join<Separator>>::Output
    where
        [T]: Join<Separator>,
    {
        Join::join(self, sep)
    }
    #[deprecated(since = "1.3.0", note = "renamed to join")]
    pub fn connect<Separator>(&self, sep: Separator) -> <[T] as Join<Separator>>::Output
    where
        [T]: Join<Separator>,
    {
        Join::join(self, sep)
    }
    pub const fn first_chunk<const N: usize>(&self) -> Option<&[T; N]> {
        if self.len() < N {
            None
        } else {
            Some(unsafe { &*(self.as_ptr() as *const [T; N]) })
        }
    }
    pub const fn first_chunk_mut<const N: usize>(&mut self) -> Option<&mut [T; N]> {
        if self.len() < N {
            None
        } else {
            Some(unsafe { &mut *(self.as_mut_ptr() as *mut [T; N]) })
        }
    }
    pub const fn last_chunk<const N: usize>(&self) -> Option<&[T; N]> {
        if self.len() < N {
            None
        } else {
            Some(unsafe { &*(self.as_ptr().add(self.len() - N) as *const [T; N]) })
        }
    }
    pub const fn last_chunk_mut<const N: usize>(&mut self) -> Option<&mut [T; N]> {
        let len = self.len();
        if len < N {
            None
        } else {
            Some(unsafe { &mut *(self.as_mut_ptr().add(len - N) as *mut [T; N]) })
        }
    }
    pub const fn split_first_chunk<const N: usize>(&self) -> Option<(&[T; N], &[T])> {
        if self.len() < N {
            None
        } else {
            let (a, b) = self.split_at(N);
            Some((unsafe { &*(a.as_ptr() as *const [T; N]) }, b))
        }
    }
    pub const fn split_last_chunk<const N: usize>(&self) -> Option<(&[T], &[T; N])> {
        if self.len() < N {
            None
        } else {
            let (a, b) = self.split_at(self.len() - N);
            Some((a, unsafe { &*(b.as_ptr() as *const [T; N]) }))
        }
    }

    #[track_caller]
    pub const fn as_chunks<const N: usize>(&self) -> (&[[T; N]], &[T]) {
        it::as_chunks(self)
    }

    #[track_caller]
    pub const fn as_chunks_mut<const N: usize>(&mut self) -> (&mut [[T; N]], &mut [T]) {
        it::as_chunks_mut(self)
    }

    #[track_caller]
    pub const fn as_rchunks<const N: usize>(&self) -> (&[T], &[[T; N]]) {
        it::as_rchunks(self)
    }

    #[track_caller]
    pub const fn as_rchunks_mut<const N: usize>(&mut self) -> (&mut [T], &mut [[T; N]]) {
        it::as_rchunks_mut(self)
    }

    #[track_caller]
    pub const fn split_first_chunk_mut<const N: usize>(&mut self) -> Option<(&mut [T; N], &mut [T])> {
        it::split_first_chunk_mut(self)
    }

    #[track_caller]
    pub const fn split_last_chunk_mut<const N: usize>(&mut self) -> Option<(&mut [T], &mut [T; N])> {
        it::split_last_chunk_mut(self)
    }
}

impl [u8] {
    pub fn utf8_chunks(&self) -> crate::str::Utf8Chunks<'_> {
        crate::str::utf8_chunks(self)
    }
    pub const fn is_ascii(&self) -> bool {
        super::is_ascii(self)
    }
    pub const fn eq_ignore_ascii_case(&self, other: &[u8]) -> bool {
        super::eq_ignore_ascii_case(self, other)
    }
    pub const fn make_ascii_uppercase(&mut self) {
        super::make_ascii_uppercase(self)
    }
    pub const fn make_ascii_lowercase(&mut self) {
        super::make_ascii_lowercase(self)
    }
    pub fn to_ascii_uppercase(&self) -> Vec<u8> {
        let mut v = self.to_vec();
        super::make_ascii_uppercase(&mut v);
        v
    }
    pub fn to_ascii_lowercase(&self) -> Vec<u8> {
        let mut v = self.to_vec();
        super::make_ascii_lowercase(&mut v);
        v
    }
    pub const fn trim_ascii_start(&self) -> &[u8] {
        super::trim_ascii_start(self)
    }
    pub const fn trim_ascii_end(&self) -> &[u8] {
        super::trim_ascii_end(self)
    }
    pub const fn trim_ascii(&self) -> &[u8] {
        super::trim_ascii(self)
    }
}

/// (number of U, number of trailing T) for `len` Ts starting U-aligned: whole groups of
/// lcm(t, u) bytes become Us (real core's align_to_offsets).
fn align_to_offsets(t: usize, u: usize, len: usize) -> (usize, usize) {
    let (mut a, mut b) = (t, u);
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    let gcd = a;
    let ts = u / gcd;
    let us = t / gcd;
    (len / ts * us, len % ts)
}

impl<T, const N: usize> [[T; N]] {
    /// The arrays' elements as one slice (real core; `[T]` is layout-identical to `[T; N]`).
    pub const fn as_flattened(&self) -> &[T] {
        let len = if crate::mem::size_of::<T>() == 0 {
            match self.len().checked_mul(N) {
                Some(l) => l,
                None => crate::panicking::panic_str("slice len overflow"),
            }
        } else {
            self.len() * N
        };
        unsafe { crate::slice::from_raw_parts(self.as_ptr() as *const T, len) }
    }
    pub const fn as_flattened_mut(&mut self) -> &mut [T] {
        let len = if crate::mem::size_of::<T>() == 0 {
            match self.len().checked_mul(N) {
                Some(l) => l,
                None => crate::panicking::panic_str("slice len overflow"),
            }
        } else {
            self.len() * N
        };
        unsafe { crate::slice::from_raw_parts_mut(self.as_mut_ptr() as *mut T, len) }
    }
}
