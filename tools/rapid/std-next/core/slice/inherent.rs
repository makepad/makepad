//! `impl<T> [T]` and `impl [u8]` (Rapid only: inherent impls on primitive types). Bodies
//! forward to the free functions of this module tree, which the rustc shim checks.

use super::iter as it;
use super::sort;
use super::{Chunks, ChunksExact, ChunksExactMut, ChunksMut, Iter, IterMut, RChunks, RSplit, RSplitN, Split, SplitInclusive, SplitMut, SplitN, Windows};
use super::{Concat, Join, SliceIndex};
use crate::cmp::Ordering;
use crate::intrinsics_mem as im;
use crate::vec::Vec;

impl<T> [T] {
    pub const fn len(&self) -> usize {
        unsafe { im::slice_len(self) }
    }
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn first(&self) -> Option<&T> {
        if self.is_empty() {
            None
        } else {
            Some(&self[0])
        }
    }
    pub fn first_mut(&mut self) -> Option<&mut T> {
        if self.is_empty() {
            None
        } else {
            Some(&mut self[0])
        }
    }
    pub fn last(&self) -> Option<&T> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some(&self[n - 1])
        }
    }
    pub fn last_mut(&mut self) -> Option<&mut T> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some(&mut self[n - 1])
        }
    }
    pub fn split_first(&self) -> Option<(&T, &[T])> {
        if self.is_empty() {
            None
        } else {
            Some((&self[0], &self[1..]))
        }
    }
    pub fn split_first_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        if self.is_empty() {
            None
        } else {
            let (a, b) = super::split_at_mut(self, 1);
            Some((&mut a[0], b))
        }
    }
    pub fn split_last(&self) -> Option<(&T, &[T])> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            Some((&self[n - 1], &self[..n - 1]))
        }
    }
    pub fn split_last_mut(&mut self) -> Option<(&mut T, &mut [T])> {
        let n = self.len();
        if n == 0 {
            None
        } else {
            let (a, b) = super::split_at_mut(self, n - 1);
            Some((&mut b[0], a))
        }
    }
    pub fn get<I: SliceIndex<[T]>>(&self, index: I) -> Option<&I::Output> {
        index.get(self)
    }
    pub fn get_mut<I: SliceIndex<[T]>>(&mut self, index: I) -> Option<&mut I::Output> {
        index.get_mut(self)
    }
    pub unsafe fn get_unchecked<I: SliceIndex<[T]>>(&self, index: I) -> &I::Output {
        &*index.get_unchecked(self)
    }
    pub unsafe fn get_unchecked_mut<I: SliceIndex<[T]>>(&mut self, index: I) -> &mut I::Output {
        &mut *index.get_unchecked_mut(self)
    }
    pub const fn as_ptr(&self) -> *const T {
        self as *const [T] as *const T
    }
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut [T] as *mut T
    }
    pub fn as_ptr_range(&self) -> crate::ops::Range<*const T> {
        let start = self.as_ptr();
        let end = unsafe { im::ptr_add(start, self.len()) };
        start..end
    }
    #[track_caller]
    pub fn swap(&mut self, a: usize, b: usize) {
        super::swap(self, a, b)
    }
    pub fn reverse(&mut self) {
        super::reverse(self)
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter::new(self)
    }
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut::new(self)
    }
    #[track_caller]
    pub fn windows(&self, size: usize) -> Windows<'_, T> {
        it::windows(self, size)
    }
    #[track_caller]
    pub fn chunks(&self, chunk_size: usize) -> Chunks<'_, T> {
        it::chunks(self, chunk_size)
    }
    #[track_caller]
    pub fn chunks_mut(&mut self, chunk_size: usize) -> ChunksMut<'_, T> {
        it::chunks_mut(self, chunk_size)
    }
    #[track_caller]
    pub fn chunks_exact(&self, chunk_size: usize) -> ChunksExact<'_, T> {
        it::chunks_exact(self, chunk_size)
    }
    #[track_caller]
    pub fn chunks_exact_mut(&mut self, chunk_size: usize) -> ChunksExactMut<'_, T> {
        it::chunks_exact_mut(self, chunk_size)
    }
    #[track_caller]
    pub fn rchunks(&self, chunk_size: usize) -> RChunks<'_, T> {
        it::rchunks(self, chunk_size)
    }
    #[track_caller]
    pub fn split_at(&self, mid: usize) -> (&[T], &[T]) {
        if mid > self.len() {
            crate::panicking::panic_str("mid > len");
        }
        (&self[..mid], &self[mid..])
    }
    #[track_caller]
    pub fn split_at_mut(&mut self, mid: usize) -> (&mut [T], &mut [T]) {
        super::split_at_mut(self, mid)
    }
    pub fn split_at_checked(&self, mid: usize) -> Option<(&[T], &[T])> {
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
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, f: F) -> Result<usize, usize> {
        super::binary_search_by(self, f)
    }
    pub fn binary_search_by_key<B: Ord, F: FnMut(&T) -> B>(&self, b: &B, f: F) -> Result<usize, usize> {
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
    pub fn is_sorted_by<F: FnMut(&T, &T) -> bool>(&self, compare: F) -> bool {
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
    pub fn is_sorted_by_key<K: PartialOrd, F: FnMut(&T) -> K>(&self, f: F) -> bool {
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
    pub fn rotate_left(&mut self, mid: usize) {
        super::rotate_left(self, mid)
    }
    #[track_caller]
    pub fn rotate_right(&mut self, k: usize) {
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
    pub fn copy_from_slice(&mut self, src: &[T])
    where
        T: Copy,
    {
        super::copy_from_slice(self, src)
    }
    #[track_caller]
    pub fn clone_from_slice(&mut self, src: &[T])
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
    pub fn swap_with_slice(&mut self, other: &mut [T]) {
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
    #[cfg(any(rapid_check, rapid_const_generics))]
    pub fn first_chunk<const N: usize>(&self) -> Option<&[T; N]> {
        if self.len() < N {
            None
        } else {
            Some(unsafe { &*(self.as_ptr() as *const [T; N]) })
        }
    }
    #[cfg(any(rapid_check, rapid_const_generics))]
    pub fn as_chunks<const N: usize>(&self) -> (&[[T; N]], &[T]) {
        let n = self.len() / N;
        let (multiple, remainder) = self.split_at(n * N);
        (unsafe { im::slice_from_raw(multiple.as_ptr() as *const [T; N], n) }, remainder)
    }
}

impl [u8] {
    pub fn is_ascii(&self) -> bool {
        super::is_ascii(self)
    }
    pub fn eq_ignore_ascii_case(&self, other: &[u8]) -> bool {
        super::eq_ignore_ascii_case(self, other)
    }
    pub fn make_ascii_uppercase(&mut self) {
        super::make_ascii_uppercase(self)
    }
    pub fn make_ascii_lowercase(&mut self) {
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
    pub fn trim_ascii_start(&self) -> &[u8] {
        super::trim_ascii_start(self)
    }
    pub fn trim_ascii_end(&self) -> &[u8] {
        super::trim_ascii_end(self)
    }
    pub fn trim_ascii(&self) -> &[u8] {
        super::trim_ascii(self)
    }
}
