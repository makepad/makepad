//! SliceIndex: indexing slices by usize and ranges, with core's panic messages.
//! (str implements SliceIndex<str> for ranges in str/.)

use crate::intrinsics_mem as im;
use crate::ops::{Bound, Index, IndexMut, Range, RangeFrom, RangeFull, RangeInclusive, RangeTo, RangeToInclusive};
use crate::panicking::{panic_bounds, slice_end_index_len_fail, slice_index_order_fail, slice_start_index_len_fail};

pub trait SliceIndex<T: ?Sized> {
    type Output: ?Sized;
    fn get(self, slice: &T) -> Option<&Self::Output>;
    fn get_mut(self, slice: &mut T) -> Option<&mut Self::Output>;
    unsafe fn get_unchecked(self, slice: *const T) -> *const Self::Output;
    unsafe fn get_unchecked_mut(self, slice: *mut T) -> *mut Self::Output;
    #[track_caller]
    fn index(self, slice: &T) -> &Self::Output;
    #[track_caller]
    fn index_mut(self, slice: &mut T) -> &mut Self::Output;
}

impl<T, I: SliceIndex<[T]>> Index<I> for [T] {
    type Output = I::Output;
    #[track_caller]
    fn index(&self, index: I) -> &I::Output {
        index.index(self)
    }
}

impl<T, I: SliceIndex<[T]>> IndexMut<I> for [T] {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut I::Output {
        index.index_mut(self)
    }
}

impl<T, I: SliceIndex<[T]>, const N: usize> Index<I> for [T; N] {
    type Output = I::Output;
    #[track_caller]
    fn index(&self, index: I) -> &I::Output {
        index.index(&self[..])
    }
}

impl<T, I: SliceIndex<[T]>, const N: usize> IndexMut<I> for [T; N] {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut I::Output {
        index.index_mut(&mut self[..])
    }
}

fn base<T>(s: *const [T]) -> *const T {
    s as *const T
}

fn base_mut<T>(s: *mut [T]) -> *mut T {
    s as *mut T
}

unsafe fn sub<'a, T>(s: *const [T], start: usize, len: usize) -> *const [T] {
    im::slice_from_raw(im::ptr_add(base(s), start), len) as *const [T]
}

unsafe fn sub_mut<'a, T>(s: *mut [T], start: usize, len: usize) -> *mut [T] {
    im::slice_from_raw_mut(im::ptr_add_mut(base_mut(s), start), len) as *mut [T]
}

impl<T> SliceIndex<[T]> for usize {
    type Output = T;
    fn get(self, slice: &[T]) -> Option<&T> {
        if self < slice.len() {
            Some(unsafe { &*im::ptr_add(base(slice), self) })
        } else {
            None
        }
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut T> {
        if self < slice.len() {
            Some(unsafe { &mut *im::ptr_add_mut(base_mut(slice), self) })
        } else {
            None
        }
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const T {
        im::ptr_add(base(slice), self)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut T {
        im::ptr_add_mut(base_mut(slice), self)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &T {
        let len = slice.len();
        if self >= len {
            panic_bounds(self, len);
        }
        unsafe { &*im::ptr_add(base(slice), self) }
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut T {
        let len = slice.len();
        if self >= len {
            panic_bounds(self, len);
        }
        unsafe { &mut *im::ptr_add_mut(base_mut(slice), self) }
    }
}

impl<T> SliceIndex<[T]> for Range<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        if self.start > self.end || self.end > slice.len() {
            None
        } else {
            Some(unsafe { &*sub(slice, self.start, self.end - self.start) })
        }
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        if self.start > self.end || self.end > slice.len() {
            None
        } else {
            Some(unsafe { &mut *sub_mut(slice, self.start, self.end - self.start) })
        }
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        sub(slice, self.start, self.end - self.start)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        sub_mut(slice, self.start, self.end - self.start)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        if self.start > self.end {
            slice_index_order_fail(self.start, self.end);
        } else if self.end > slice.len() {
            slice_end_index_len_fail(self.end, slice.len());
        }
        unsafe { &*sub(slice, self.start, self.end - self.start) }
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        if self.start > self.end {
            slice_index_order_fail(self.start, self.end);
        } else if self.end > slice.len() {
            slice_end_index_len_fail(self.end, slice.len());
        }
        unsafe { &mut *sub_mut(slice, self.start, self.end - self.start) }
    }
}

impl<T> SliceIndex<[T]> for RangeTo<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        (0..self.end).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        (0..self.end).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        (0..self.end).get_unchecked(slice)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        (0..self.end).get_unchecked_mut(slice)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        (0..self.end).index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        (0..self.end).index_mut(slice)
    }
}

impl<T> SliceIndex<[T]> for RangeFrom<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        (self.start..slice.len()).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        let len = slice.len();
        (self.start..len).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        let len = (*slice).len();
        (self.start..len).get_unchecked(slice)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        let len = (*slice).len();
        (self.start..len).get_unchecked_mut(slice)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        if self.start > slice.len() {
            slice_start_index_len_fail(self.start, slice.len());
        }
        unsafe { &*sub(slice, self.start, slice.len() - self.start) }
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        let len = slice.len();
        if self.start > len {
            slice_start_index_len_fail(self.start, len);
        }
        unsafe { &mut *sub_mut(slice, self.start, len - self.start) }
    }
}

impl<T> SliceIndex<[T]> for RangeFull {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        Some(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        Some(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        slice
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        slice
    }
    fn index(self, slice: &[T]) -> &[T] {
        slice
    }
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        slice
    }
}

/// rustc's `RangeInclusive::into_slice_range` as (start, exclusive end); the caller checks the end
/// is below the length.
fn incl_parts(r: &RangeInclusive<usize>) -> (usize, usize) {
    // If we're not exhausted, we want to simply slice `start..end + 1`.
    // If we are exhausted, then slicing with `end + 1..end + 1` gives us an
    // empty range that is still subject to bounds-checks for that endpoint.
    let exclusive_end = r.end + 1;
    let start = if r.exhausted { exclusive_end } else { r.start };
    (start, exclusive_end)
}

/// rustc 1.26.0 (inclusive_range): indexes the slice by the inclusive range, as rustc 1.100's
/// impl (an end at or past the length is reported as it was given).
impl<T> SliceIndex<[T]> for RangeInclusive<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        if *self.end() >= slice.len() {
            None
        } else {
            let (s, e) = incl_parts(&self);
            (s..e).get(slice)
        }
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        if *self.end() >= slice.len() {
            None
        } else {
            let (s, e) = incl_parts(&self);
            (s..e).get_mut(slice)
        }
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        let (s, e) = incl_parts(&self);
        (s..e).get_unchecked(slice)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        let (s, e) = incl_parts(&self);
        (s..e).get_unchecked_mut(slice)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        let RangeInclusive { mut start, mut end, exhausted } = self;
        let len = slice.len();
        if end < len {
            end = end + 1;
            start = if exhausted { end } else { start };
            if usize::checked_sub(end, start).is_some() {
                // `self` is checked to be valid and in bounds above.
                return unsafe { &*(start..end).get_unchecked(slice) };
            }
        }
        slice_index_fail(start, end, slice.len())
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        let RangeInclusive { mut start, mut end, exhausted } = self;
        let len = slice.len();
        if end < len {
            end = end + 1;
            start = if exhausted { end } else { start };
            if usize::checked_sub(end, start).is_some() {
                // `self` is checked to be valid and in bounds above.
                return unsafe { &mut *(start..end).get_unchecked_mut(slice) };
            }
        }
        slice_index_fail(start, end, slice.len())
    }
}

impl<T> SliceIndex<[T]> for RangeToInclusive<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        (0..=self.end).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        (0..=self.end).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        (0..=self.end).get_unchecked(slice)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        (0..=self.end).get_unchecked_mut(slice)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        (0..=self.end).index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        (0..=self.end).index_mut(slice)
    }
}

/// rustc 1.96.0 (new_range_api): indexes a slice with a `Range`.
impl<T> SliceIndex<[T]> for crate::range::Range<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        Range::from(self).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        Range::from(self).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        unsafe { Range::from(self).get_unchecked(slice) }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        unsafe { Range::from(self).get_unchecked_mut(slice) }
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        let len = slice.len();
        match Range::from(self).get(slice) {
            Some(part) => part,
            None => slice_index_fail(self.start, self.end, len),
        }
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        let len = slice.len();
        match Range::from(self).get_mut(slice) {
            Some(part) => part,
            None => slice_index_fail(self.start, self.end, len),
        }
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): indexes a slice with a `RangeInclusive`.
impl<T> SliceIndex<[T]> for crate::range::RangeInclusive<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        RangeInclusive::from(self).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        RangeInclusive::from(self).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        unsafe { RangeInclusive::from(self).get_unchecked(slice) }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        unsafe { RangeInclusive::from(self).get_unchecked_mut(slice) }
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        RangeInclusive::from(self).index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        RangeInclusive::from(self).index_mut(slice)
    }
}

/// rustc 1.96.0 (new_range_from_api): indexes a slice with a `RangeFrom`.
impl<T> SliceIndex<[T]> for crate::range::RangeFrom<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        RangeFrom::from(self).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        RangeFrom::from(self).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        unsafe { RangeFrom::from(self).get_unchecked(slice) }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        unsafe { RangeFrom::from(self).get_unchecked_mut(slice) }
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        RangeFrom::from(self).index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        RangeFrom::from(self).index_mut(slice)
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): indexes a slice with a `RangeToInclusive`.
impl<T> SliceIndex<[T]> for crate::range::RangeToInclusive<usize> {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        (0..=self.last).get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        (0..=self.last).get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        unsafe { (0..=self.last).get_unchecked(slice) }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        unsafe { (0..=self.last).get_unchecked_mut(slice) }
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        crate::range::RangeInclusive { start: 0, last: self.last }.index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        crate::range::RangeInclusive { start: 0, last: self.last }.index_mut(slice)
    }
}

/// The range a bound pair names in a slice of `len`, or the (start, end) its failure reports
/// (core slice/index.rs try_into_slice_range): the end first, then the start against it.
pub(crate) fn try_into_slice_range(len: usize, (start, end): (Bound<usize>, Bound<usize>)) -> Result<Range<usize>, (usize, usize)> {
    let end = match end {
        Bound::Included(end) if end >= len => return Err((0, end)),
        // (`end < len`: no overflow)
        Bound::Included(end) => end + 1,
        Bound::Excluded(end) if end > len => return Err((0, end)),
        Bound::Excluded(end) => end,
        Bound::Unbounded => len,
    };
    let start = match start {
        Bound::Excluded(start) if start >= end => return Err((start, end)),
        // (`start < end`: no overflow)
        Bound::Excluded(start) => start + 1,
        Bound::Included(start) if start > end => return Err((start, end)),
        Bound::Included(start) => start,
        Bound::Unbounded => 0,
    };
    Ok(start..end)
}

/// `try_into_slice_range`'s range, or its failure's panic (core into_slice_range).
#[track_caller]
pub(crate) fn into_slice_range(len: usize, bounds: (Bound<usize>, Bound<usize>)) -> Range<usize> {
    match try_into_slice_range(len, bounds) {
        Ok(range) => range,
        Err((start, end)) => slice_index_fail(start, end, len),
    }
}

/// core slice/index.rs slice_index_fail: a start past the length, else an end past it, else the
/// order, else the end (one the length reaches: `(Unbounded, Included(len))`, or an excluded
/// start at the end: `(Excluded(3), Excluded(3))`), as rustc 1.99's messages are.
#[track_caller]
fn slice_index_fail(start: usize, end: usize, len: usize) -> ! {
    if start > len {
        slice_start_index_len_fail(start, len)
    }
    if end > len {
        slice_end_index_len_fail(end, len)
    }
    if start > end {
        slice_index_order_fail(start, end)
    }
    slice_end_index_len_fail(end, len)
}

fn bounds_to_range(b: (Bound<usize>, Bound<usize>), len: usize) -> Range<usize> {
    let (s, e) = crate::slice::range(&b, len);
    s..e
}

impl<T> SliceIndex<[T]> for (Bound<usize>, Bound<usize>) {
    type Output = [T];
    fn get(self, slice: &[T]) -> Option<&[T]> {
        try_into_slice_range(slice.len(), self).ok()?.get(slice)
    }
    fn get_mut(self, slice: &mut [T]) -> Option<&mut [T]> {
        try_into_slice_range(slice.len(), self).ok()?.get_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const [T]) -> *const [T] {
        bounds_to_range(self, (*slice).len()).get_unchecked(slice)
    }
    unsafe fn get_unchecked_mut(self, slice: *mut [T]) -> *mut [T] {
        bounds_to_range(self, (*slice).len()).get_unchecked_mut(slice)
    }
    #[track_caller]
    fn index(self, slice: &[T]) -> &[T] {
        into_slice_range(slice.len(), self).index(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut [T]) -> &mut [T] {
        let len = slice.len();
        into_slice_range(len, self).index_mut(slice)
    }
}
