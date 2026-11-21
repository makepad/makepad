//! Relax's trait impls on str.

use super::{cmp_str, eq_str, StrIndex};
use crate::cmp::Ordering;
use crate::option::Option::{self, Some};
use crate::slice::SliceIndex;

impl PartialEq for str {
    fn eq(&self, other: &str) -> bool {
        eq_str(self, other)
    }
}

impl Eq for str {}

impl PartialOrd for str {
    fn partial_cmp(&self, other: &str) -> Option<Ordering> {
        Some(cmp_str(self, other))
    }
}

impl Ord for str {
    fn cmp(&self, other: &str) -> Ordering {
        cmp_str(self, other)
    }
}

impl<I: StrIndex> crate::ops::Index<I> for str {
    type Output = str;
    #[track_caller]
    fn index(&self, index: I) -> &str {
        index.index_str(self)
    }
}

impl<I: StrIndex> crate::ops::IndexMut<I> for str {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut str {
        index.index_str_mut(self)
    }
}

// String methods and the indexing operators take StrIndex. Accept every string
// SliceIndex here so a generic SliceIndex<str, Output = str> bound works too.
impl<I: SliceIndex<str, Output = str>> StrIndex for I {
    fn get_str(self, slice: &str) -> Option<&str> {
        SliceIndex::get(self, slice)
    }
    fn get_str_mut(self, slice: &mut str) -> Option<&mut str> {
        SliceIndex::get_mut(self, slice)
    }
    #[track_caller]
    fn index_str(self, slice: &str) -> &str {
        SliceIndex::index(self, slice)
    }
    #[track_caller]
    fn index_str_mut(self, slice: &mut str) -> &mut str {
        SliceIndex::index_mut(self, slice)
    }
}

/// rustc 1.96.0 (new_range_api): indexes a string at UTF-8 boundaries with a `Range`.
impl SliceIndex<str> for crate::range::Range<usize> {
    type Output = str;
    fn get(self, slice: &str) -> Option<&str> {
        crate::ops::Range::from(self).get_str(slice)
    }
    fn get_mut(self, slice: &mut str) -> Option<&mut str> {
        crate::ops::Range::from(self).get_str_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const str) -> *const str {
        unsafe { crate::ops::Range::from(self).get_unchecked(slice as *const [u8]) as *const str }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut str) -> *mut str {
        unsafe { crate::ops::Range::from(self).get_unchecked_mut(slice as *mut [u8]) as *mut str }
    }
    #[track_caller]
    fn index(self, slice: &str) -> &str {
        crate::ops::Range::from(self).index_str(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut str) -> &mut str {
        crate::ops::Range::from(self).index_str_mut(slice)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): indexes a string at UTF-8 boundaries with a `RangeInclusive`.
impl SliceIndex<str> for crate::range::RangeInclusive<usize> {
    type Output = str;
    fn get(self, slice: &str) -> Option<&str> {
        crate::ops::RangeInclusive::from(self).get_str(slice)
    }
    fn get_mut(self, slice: &mut str) -> Option<&mut str> {
        crate::ops::RangeInclusive::from(self).get_str_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const str) -> *const str {
        unsafe { crate::ops::RangeInclusive::from(self).get_unchecked(slice as *const [u8]) as *const str }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut str) -> *mut str {
        unsafe { crate::ops::RangeInclusive::from(self).get_unchecked_mut(slice as *mut [u8]) as *mut str }
    }
    #[track_caller]
    fn index(self, slice: &str) -> &str {
        crate::ops::RangeInclusive::from(self).index_str(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut str) -> &mut str {
        crate::ops::RangeInclusive::from(self).index_str_mut(slice)
    }
}

/// rustc 1.96.0 (new_range_from_api): indexes a string at UTF-8 boundaries with a `RangeFrom`.
impl SliceIndex<str> for crate::range::RangeFrom<usize> {
    type Output = str;
    fn get(self, slice: &str) -> Option<&str> {
        crate::ops::RangeFrom::from(self).get_str(slice)
    }
    fn get_mut(self, slice: &mut str) -> Option<&mut str> {
        crate::ops::RangeFrom::from(self).get_str_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const str) -> *const str {
        unsafe { crate::ops::RangeFrom::from(self).get_unchecked(slice as *const [u8]) as *const str }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut str) -> *mut str {
        unsafe { crate::ops::RangeFrom::from(self).get_unchecked_mut(slice as *mut [u8]) as *mut str }
    }
    #[track_caller]
    fn index(self, slice: &str) -> &str {
        crate::ops::RangeFrom::from(self).index_str(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut str) -> &mut str {
        crate::ops::RangeFrom::from(self).index_str_mut(slice)
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): indexes a string at UTF-8 boundaries with a `RangeToInclusive`.
impl SliceIndex<str> for crate::range::RangeToInclusive<usize> {
    type Output = str;
    fn get(self, slice: &str) -> Option<&str> {
        (0..=self.last).get_str(slice)
    }
    fn get_mut(self, slice: &mut str) -> Option<&mut str> {
        (0..=self.last).get_str_mut(slice)
    }
    unsafe fn get_unchecked(self, slice: *const str) -> *const str {
        unsafe { (0..=self.last).get_unchecked(slice as *const [u8]) as *const str }
    }
    unsafe fn get_unchecked_mut(self, slice: *mut str) -> *mut str {
        unsafe { (0..=self.last).get_unchecked_mut(slice as *mut [u8]) as *mut str }
    }
    #[track_caller]
    fn index(self, slice: &str) -> &str {
        (0..=self.last).index_str(slice)
    }
    #[track_caller]
    fn index_mut(self, slice: &mut str) -> &mut str {
        (0..=self.last).index_str_mut(slice)
    }
}

impl<'a> Default for &'a mut str {
    fn default() -> &'a mut str {
        unsafe { super::from_utf8_unchecked_mut(&mut []) }
    }
}
