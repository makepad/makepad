//! Range values with separate iterators.

use crate::fmt;
use crate::iter::Step;
use crate::ops::Bound::{self, Excluded, Included, Unbounded};
use crate::ops::RangeBounds;

mod iter;
/// rustc 1.98.0 (new_range_api_legacy): the original range types in `core::ops`.
pub mod legacy;

/// rustc 1.98.0 (new_range_api_exports): the unbounded and upper-exclusive range types.
pub use crate::ops::{RangeFull, RangeTo};
/// rustc 1.96.0 (new_range_api): the by-value iterator over a `Range`.
pub use self::iter::RangeIter;
/// rustc 1.95.0 (new_range_inclusive_api): the by-value iterator over a `RangeInclusive`.
pub use self::iter::RangeInclusiveIter;
/// rustc 1.96.0 (new_range_from_api): the by-value iterator over a `RangeFrom`.
pub use self::iter::RangeFromIter;

/// rustc 1.96.0 (new_range_api): a half-open range.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Range<Idx> {
    /// rustc 1.96.0 (new_range_api): the inclusive lower bound.
    pub start: Idx,
    /// rustc 1.96.0 (new_range_api): the exclusive upper bound.
    pub end: Idx,
}

/// rustc 1.96.0 (new_range_api): formats the bounds using range syntax.
impl<Idx: fmt::Debug> fmt::Debug for Range<Idx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..")?;
        self.end.fmt(f)
    }
}

impl<Idx: Step> Range<Idx> {
    /// rustc 1.96.0 (new_range_api): creates an iterator over the range's elements.
    pub fn iter(&self) -> RangeIter<Idx> {
        self.clone().into_iter()
    }
}

impl<Idx: PartialOrd> Range<Idx> {
    /// rustc 1.96.0 (new_range_api): returns whether the range contains the item.
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        <Self as RangeBounds<Idx>>::contains(self, item)
    }
    /// rustc 1.96.0 (new_range_api): returns whether the range contains no items.
    pub fn is_empty(&self) -> bool {
        !(self.start < self.end)
    }
}

/// rustc 1.96.0 (new_range_api): borrows the range endpoints.
impl<T> RangeBounds<T> for Range<T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Excluded(&self.end)
    }
}

/// rustc 1.96.0 (new_range_api): uses the referenced endpoints as bounds.
impl<T> RangeBounds<T> for Range<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Excluded(self.end)
    }
}

/// rustc 1.96.0 (new_range_api): converts to the legacy range.
impl<T> From<Range<T>> for legacy::Range<T> {
    fn from(value: Range<T>) -> Self {
        Self { start: value.start, end: value.end }
    }
}

/// rustc 1.96.0 (new_range_api): converts from the legacy range.
impl<T> From<legacy::Range<T>> for Range<T> {
    fn from(value: legacy::Range<T>) -> Self {
        Self { start: value.start, end: value.end }
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): a range including both endpoints.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeInclusive<Idx> {
    /// rustc 1.95.0 (new_range_inclusive_api): the inclusive lower bound.
    pub start: Idx,
    /// rustc 1.95.0 (new_range_inclusive_api): the inclusive upper bound.
    pub last: Idx,
}

/// rustc 1.95.0 (new_range_inclusive_api): formats the bounds using range syntax.
impl<Idx: fmt::Debug> fmt::Debug for RangeInclusive<Idx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..=")?;
        self.last.fmt(f)
    }
}

impl<Idx: Step> RangeInclusive<Idx> {
    /// rustc 1.95.0 (new_range_inclusive_api): creates an iterator over the range's elements.
    pub fn iter(&self) -> RangeInclusiveIter<Idx> {
        self.clone().into_iter()
    }
}

impl<Idx: PartialOrd> RangeInclusive<Idx> {
    /// rustc 1.95.0 (new_range_inclusive_api): returns whether the range contains the item.
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        <Self as RangeBounds<Idx>>::contains(self, item)
    }
    /// rustc 1.95.0 (new_range_inclusive_api): returns whether the range contains no items.
    pub fn is_empty(&self) -> bool {
        !(self.start <= self.last)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): borrows the range endpoints.
impl<T> RangeBounds<T> for RangeInclusive<T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Included(&self.last)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): uses the referenced endpoints as bounds.
impl<T> RangeBounds<T> for RangeInclusive<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Included(self.last)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): converts to the legacy range.
impl<T> From<RangeInclusive<T>> for legacy::RangeInclusive<T> {
    fn from(value: RangeInclusive<T>) -> Self {
        Self::new(value.start, value.last)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): converts from the legacy range.
impl<T> From<legacy::RangeInclusive<T>> for RangeInclusive<T> {
    fn from(value: legacy::RangeInclusive<T>) -> Self {
        assert!(!value.exhausted, "attempted to convert from an exhausted `legacy::RangeInclusive`");
        let (start, last) = value.into_inner();
        Self { start, last }
    }
}

/// rustc 1.96.0 (new_range_from_api): a range with an inclusive lower bound.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeFrom<Idx> {
    /// rustc 1.96.0 (new_range_from_api): the inclusive lower bound.
    pub start: Idx,
}

/// rustc 1.96.0 (new_range_from_api): formats the bounds using range syntax.
impl<Idx: fmt::Debug> fmt::Debug for RangeFrom<Idx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..")
    }
}

impl<Idx: Step> RangeFrom<Idx> {
    /// rustc 1.96.0 (new_range_from_api): creates an iterator over the range's elements.
    pub fn iter(&self) -> RangeFromIter<Idx> {
        self.clone().into_iter()
    }
}

impl<Idx: PartialOrd> RangeFrom<Idx> {
    /// rustc 1.96.0 (new_range_from_api): returns whether the range contains the item.
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        <Self as RangeBounds<Idx>>::contains(self, item)
    }
}

/// rustc 1.96.0 (new_range_from_api): borrows the range endpoints.
impl<T> RangeBounds<T> for RangeFrom<T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Unbounded
    }
}

/// rustc 1.96.0 (new_range_from_api): uses the referenced endpoints as bounds.
impl<T> RangeBounds<T> for RangeFrom<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Unbounded
    }
}

/// rustc 1.96.0 (new_range_from_api): converts to the legacy range.
impl<T> From<RangeFrom<T>> for legacy::RangeFrom<T> {
    fn from(value: RangeFrom<T>) -> Self {
        Self { start: value.start }
    }
}

/// rustc 1.96.0 (new_range_from_api): converts from the legacy range.
impl<T> From<legacy::RangeFrom<T>> for RangeFrom<T> {
    fn from(value: legacy::RangeFrom<T>) -> Self {
        Self { start: value.start }
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): a range with an inclusive upper bound.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeToInclusive<Idx> {
    /// rustc 1.96.0 (new_range_to_inclusive_api): the inclusive upper bound.
    pub last: Idx,
}

/// rustc 1.96.0 (new_range_to_inclusive_api): formats the bounds using range syntax.
impl<Idx: fmt::Debug> fmt::Debug for RangeToInclusive<Idx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("..=")?;
        self.last.fmt(f)
    }
}

impl<Idx: PartialOrd> RangeToInclusive<Idx> {
    /// rustc 1.96.0 (new_range_to_inclusive_api): returns whether the range contains the item.
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        <Self as RangeBounds<Idx>>::contains(self, item)
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): borrows the range endpoints.
impl<T> RangeBounds<T> for RangeToInclusive<T> {
    fn start_bound(&self) -> Bound<&T> {
        Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Included(&self.last)
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): uses the referenced endpoints as bounds.
impl<T> RangeBounds<T> for RangeToInclusive<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Included(self.last)
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): converts to the legacy range.
impl<T> From<RangeToInclusive<T>> for legacy::RangeToInclusive<T> {
    fn from(value: RangeToInclusive<T>) -> Self {
        Self { end: value.last }
    }
}

/// rustc 1.96.0 (new_range_to_inclusive_api): converts from the legacy range.
impl<T> From<legacy::RangeToInclusive<T>> for RangeToInclusive<T> {
    fn from(value: legacy::RangeToInclusive<T>) -> Self {
        Self { last: value.end }
    }
}
