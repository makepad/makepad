//! Slices: free functions, comparison helpers, indexing, iterators, sorting.
//! The `impl<T> [T]` methods are in inherent.rs (HotRust only; the rustc shim checks the
//! free functions they forward to).

mod index;
#[cfg(not(hotrust_check))]
mod inherent;
mod iter;
mod join;
pub mod sort;

pub use index::SliceIndex;
pub use iter::{
    Chunks, ChunksExact, ChunksExactMut, ChunksMut, Iter, IterMut, RChunks, RSplit, RSplitN, Split, SplitInclusive, SplitMut,
    SplitN, Windows,
};
pub use join::{Concat, Join};
/// Constructors and helpers the rustc shim's tests call directly.
#[cfg(hotrust_check)]
pub use iter::{chunks, chunks_exact, rchunks, rsplit, rsplitn, split, split_inclusive, splitn, windows};
#[cfg(hotrust_check)]
pub use join::{concat_str, concat_vec, join_str, join_vec};

use crate::cmp::Ordering;
use crate::intrinsics_mem as im;

pub unsafe fn from_raw_parts<'a, T>(data: *const T, len: usize) -> &'a [T] {
    im::slice_from_raw(data, len)
}

pub unsafe fn from_raw_parts_mut<'a, T>(data: *mut T, len: usize) -> &'a mut [T] {
    im::slice_from_raw_mut(data, len)
}

pub fn from_ref<T>(s: &T) -> &[T] {
    unsafe { im::slice_from_raw(s as *const T, 1) }
}

pub fn from_mut<T>(s: &mut T) -> &mut [T] {
    unsafe { im::slice_from_raw_mut(s as *mut T, 1) }
}

pub(crate) fn slice_eq<A: PartialEq<B>, B>(a: &[A], b: &[B]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn slice_partial_cmp<T: PartialOrd>(a: &[T], b: &[T]) -> Option<Ordering> {
    let l = if a.len() < b.len() { a.len() } else { b.len() };
    let mut i = 0;
    while i < l {
        match a[i].partial_cmp(&b[i]) {
            Some(Ordering::Equal) => {}
            non_eq => return non_eq,
        }
        i += 1;
    }
    a.len().partial_cmp(&b.len())
}

pub(crate) fn slice_cmp<T: Ord>(a: &[T], b: &[T]) -> Ordering {
    let l = if a.len() < b.len() { a.len() } else { b.len() };
    let mut i = 0;
    while i < l {
        match a[i].cmp(&b[i]) {
            Ordering::Equal => {}
            non_eq => return non_eq,
        }
        i += 1;
    }
    a.len().cmp(&b.len())
}

/// Binary search with core's exact probe sequence (which index is returned among equal
/// elements is observable).
pub fn binary_search_by<T, F: FnMut(&T) -> Ordering>(s: &[T], f: F) -> Result<usize, usize> {
    let mut f = f;
    let mut size = s.len();
    if size == 0 {
        return Err(0);
    }
    let mut base = 0usize;
    while size > 1 {
        let half = size / 2;
        let mid = base + half;
        let cmp = f(&s[mid]);
        base = if cmp == Ordering::Greater { base } else { mid };
        size -= half;
    }
    let cmp = f(&s[base]);
    if cmp == Ordering::Equal {
        Ok(base)
    } else {
        Err(base + if cmp == Ordering::Less { 1 } else { 0 })
    }
}

pub fn partition_point<T, P: FnMut(&T) -> bool>(s: &[T], pred: P) -> usize {
    let mut pred = pred;
    match binary_search_by(s, |x| if pred(x) { Ordering::Less } else { Ordering::Greater }) {
        Ok(i) => i,
        Err(i) => i,
    }
}

pub fn reverse<T>(s: &mut [T]) {
    let n = s.len();
    let mut i = 0;
    while i < n / 2 {
        s.swap(i, n - 1 - i);
        i += 1;
    }
}

pub fn rotate_left<T>(s: &mut [T], mid: usize) {
    if mid > s.len() {
        crate::panicking::panic_str("assertion failed: mid <= self.len()");
    }
    let k = mid;
    let n = s.len();
    reverse(&mut s[..k]);
    reverse(&mut s[k..]);
    let _ = n;
    reverse(s);
}

pub fn rotate_right<T>(s: &mut [T], k: usize) {
    if k > s.len() {
        crate::panicking::panic_str("assertion failed: k <= self.len()");
    }
    let n = s.len();
    rotate_left(s, n - k);
}

#[track_caller]
pub fn copy_from_slice<T: Copy>(dst: &mut [T], src: &[T]) {
    if dst.len() != src.len() {
        len_mismatch_fail(dst.len(), src.len());
    }
    unsafe { im::copy_nonoverlapping(im::slice_ptr(src), dst as *mut [T] as *mut T, src.len()) }
}

#[track_caller]
pub fn clone_from_slice<T: Clone>(dst: &mut [T], src: &[T]) {
    if dst.len() != src.len() {
        crate::panicking::panic_str("destination and source slices have different lengths");
    }
    let mut i = 0;
    while i < dst.len() {
        dst[i].clone_from(&src[i]);
        i += 1;
    }
}

#[cold]
#[track_caller]
fn len_mismatch_fail(dst_len: usize, src_len: usize) -> ! {
    crate::panicking::panic_fmt(format_args!(
        "copy_from_slice: source slice length ({}) does not match destination slice length ({})",
        src_len, dst_len
    ))
}

#[track_caller]
pub fn copy_within<T: Copy, R: crate::ops::RangeBounds<usize>>(s: &mut [T], src: R, dest: usize) {
    let (start, end) = crate::slice::range(&src, s.len());
    let count = end - start;
    if dest > s.len() - count {
        crate::panicking::panic_str("dest is out of bounds");
    }
    unsafe {
        let p = s as *mut [T] as *mut T;
        im::copy(im::ptr_add(p as *const T, start), im::ptr_add_mut(p, dest), count);
    }
}

pub fn fill<T: Clone>(s: &mut [T], value: T) {
    let n = s.len();
    if n == 0 {
        return;
    }
    let mut i = 0;
    while i + 1 < n {
        s[i] = value.clone();
        i += 1;
    }
    s[n - 1] = value;
}

pub fn contains<T: PartialEq>(s: &[T], x: &T) -> bool {
    let mut i = 0;
    while i < s.len() {
        if s[i] == *x {
            return true;
        }
        i += 1;
    }
    false
}

pub fn starts_with<T: PartialEq>(s: &[T], needle: &[T]) -> bool {
    let n = needle.len();
    s.len() >= n && slice_eq(&s[..n], needle)
}

pub fn ends_with<T: PartialEq>(s: &[T], needle: &[T]) -> bool {
    let (m, n) = (s.len(), needle.len());
    m >= n && slice_eq(&s[m - n..], needle)
}

#[track_caller]
pub fn swap<T>(s: &mut [T], a: usize, b: usize) {
    let len = s.len();
    if a >= len {
        crate::panicking::panic_bounds(a, len);
    }
    if b >= len {
        crate::panicking::panic_bounds(b, len);
    }
    unsafe {
        let p = s as *mut [T] as *mut T;
        crate::ptr::swap(im::ptr_add_mut(p, a), im::ptr_add_mut(p, b));
    }
}

#[track_caller]
pub fn split_at_mut<T>(s: &mut [T], mid: usize) -> (&mut [T], &mut [T]) {
    let len = s.len();
    if mid > len {
        crate::panicking::panic_str("mid > len");
    }
    unsafe {
        let p = s as *mut [T] as *mut T;
        (im::slice_from_raw_mut(p, mid), im::slice_from_raw_mut(im::ptr_add_mut(p, mid), len - mid))
    }
}

pub fn to_vec<T: Clone>(s: &[T]) -> crate::vec::Vec<T> {
    let mut v = crate::vec::Vec::with_capacity(s.len());
    v.extend_from_slice(s);
    v
}

pub fn repeat<T: Copy>(s: &[T], n: usize) -> crate::vec::Vec<T> {
    let total = match s.len().checked_mul(n) {
        Some(t) => t,
        None => crate::panicking::panic_str("capacity overflow"),
    };
    let mut v = crate::vec::Vec::with_capacity(total);
    let mut i = 0;
    while i < n {
        v.extend_from_slice(s);
        i += 1;
    }
    v
}

/// Resolves a `usize` range against a length: (start, end), panicking like real core's
/// slice indexing does on bad ranges.
pub fn range<R: crate::ops::RangeBounds<usize>>(r: &R, len: usize) -> (usize, usize) {
    let start = match r.start_bound() {
        crate::ops::Bound::Included(s) => *s,
        crate::ops::Bound::Excluded(s) => match s.checked_add(1) {
            Some(v) => v,
            None => crate::panicking::panic_str("attempted to index slice from after maximum usize"),
        },
        crate::ops::Bound::Unbounded => 0,
    };
    let end = match r.end_bound() {
        crate::ops::Bound::Included(e) => match e.checked_add(1) {
            Some(v) => v,
            None => crate::panicking::panic_str("attempted to index slice up to maximum usize"),
        },
        crate::ops::Bound::Excluded(e) => *e,
        crate::ops::Bound::Unbounded => len,
    };
    if start > end {
        crate::panicking::slice_index_order_fail(start, end);
    }
    if end > len {
        crate::panicking::slice_end_index_len_fail(end, len);
    }
    (start, end)
}

// ---------------------------------------------------------------- [u8] ascii helpers

pub fn is_ascii(s: &[u8]) -> bool {
    let mut i = 0;
    while i < s.len() {
        if s[i] >= 0x80 {
            return false;
        }
        i += 1;
    }
    true
}

pub fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if ascii_lower(a[i]) != ascii_lower(b[i]) {
            return false;
        }
        i += 1;
    }
    true
}

pub(crate) fn ascii_lower(c: u8) -> u8 {
    if c >= b'A' && c <= b'Z' {
        c + 32
    } else {
        c
    }
}

pub(crate) fn ascii_upper(c: u8) -> u8 {
    if c >= b'a' && c <= b'z' {
        c - 32
    } else {
        c
    }
}

pub fn make_ascii_lowercase(s: &mut [u8]) {
    let mut i = 0;
    while i < s.len() {
        s[i] = ascii_lower(s[i]);
        i += 1;
    }
}

pub fn make_ascii_uppercase(s: &mut [u8]) {
    let mut i = 0;
    while i < s.len() {
        s[i] = ascii_upper(s[i]);
        i += 1;
    }
}

fn is_ascii_ws(c: u8) -> bool {
    c == b'\t' || c == b'\n' || c == 0x0c || c == b'\r' || c == b' '
}

pub fn trim_ascii_start(s: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < s.len() && is_ascii_ws(s[i]) {
        i += 1;
    }
    &s[i..]
}

pub fn trim_ascii_end(s: &[u8]) -> &[u8] {
    let mut n = s.len();
    while n > 0 && is_ascii_ws(s[n - 1]) {
        n -= 1;
    }
    &s[..n]
}

pub fn trim_ascii(s: &[u8]) -> &[u8] {
    trim_ascii_end(trim_ascii_start(s))
}
