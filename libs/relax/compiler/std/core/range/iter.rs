use super::{legacy, Range, RangeFrom, RangeInclusive};
use crate::intrinsics;
use crate::iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, IntoIterator, Iterator, Step};
use crate::mem;

/// rustc 1.96.0 (new_range_api): the by-value iterator over a `Range`.
#[derive(Debug, Clone)]
pub struct RangeIter<A>(pub(crate) legacy::Range<A>);

/// rustc 1.96.0 (new_range_api): iterates forward through the range.
impl<A: Step> Iterator for RangeIter<A> {
    type Item = A;

    fn next(&mut self) -> Option<A> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
    fn count(self) -> usize {
        self.0.size_hint().1.expect("count overflowed usize")
    }
    fn nth(&mut self, n: usize) -> Option<A> {
        self.0.nth(n)
    }
    fn last(self) -> Option<A> {
        self.0.last()
    }
    fn min(self) -> Option<A>
    where
        A: Ord,
    {
        Iterator::min(self.0)
    }
    fn max(self) -> Option<A>
    where
        A: Ord,
    {
        Iterator::max(self.0)
    }
    fn is_sorted(self) -> bool {
        true
    }
}

/// rustc 1.96.0 (new_range_api): iterates backward through the range.
impl<A: Step> DoubleEndedIterator for RangeIter<A> {
    fn next_back(&mut self) -> Option<A> {
        self.0.next_back()
    }
    fn nth_back(&mut self, n: usize) -> Option<A> {
        self.0.nth_back(n)
    }
}

/// rustc 1.96.0 (new_range_api): stays empty after exhaustion.
impl<A: Step> FusedIterator for RangeIter<A> {}

/// rustc 1.96.0 (new_range_api): iterates over the owned range.
impl<A: Step> IntoIterator for Range<A> {
    type Item = A;
    type IntoIter = RangeIter<A>;

    fn into_iter(self) -> Self::IntoIter {
        RangeIter(self.into())
    }
}

/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<usize> {}
/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<u8> {}
/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<u16> {}
/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<isize> {}
/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<i8> {}
/// rustc 1.96.0 (new_range_api): reports the exact remaining length.
impl ExactSizeIterator for RangeIter<i16> {}

/// rustc 1.95.0 (new_range_inclusive_api): the by-value iterator over a `RangeInclusive`.
#[derive(Debug, Clone)]
pub struct RangeInclusiveIter<A>(legacy::RangeInclusive<A>);

/// rustc 1.95.0 (new_range_inclusive_api): iterates forward through the range.
impl<A: Step> Iterator for RangeInclusiveIter<A> {
    type Item = A;

    fn next(&mut self) -> Option<A> {
        self.0.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
    fn count(self) -> usize {
        self.0.size_hint().1.expect("count overflowed usize")
    }
    fn nth(&mut self, n: usize) -> Option<A> {
        self.0.nth(n)
    }
    fn last(self) -> Option<A> {
        self.0.last()
    }
    fn min(self) -> Option<A>
    where
        A: Ord,
    {
        Iterator::min(self.0)
    }
    fn max(self) -> Option<A>
    where
        A: Ord,
    {
        Iterator::max(self.0)
    }
    fn is_sorted(self) -> bool {
        true
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): iterates backward through the range.
impl<A: Step> DoubleEndedIterator for RangeInclusiveIter<A> {
    fn next_back(&mut self) -> Option<A> {
        self.0.next_back()
    }
    fn nth_back(&mut self, n: usize) -> Option<A> {
        self.0.nth_back(n)
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): stays empty after exhaustion.
impl<A: Step> FusedIterator for RangeInclusiveIter<A> {}

/// rustc 1.95.0 (new_range_inclusive_api): iterates over the owned range.
impl<A: Step> IntoIterator for RangeInclusive<A> {
    type Item = A;
    type IntoIter = RangeInclusiveIter<A>;

    fn into_iter(self) -> Self::IntoIter {
        RangeInclusiveIter(self.into())
    }
}

/// rustc 1.95.0 (new_range_inclusive_api): reports the exact remaining length.
impl ExactSizeIterator for RangeInclusiveIter<u8> {}
/// rustc 1.95.0 (new_range_inclusive_api): reports the exact remaining length.
impl ExactSizeIterator for RangeInclusiveIter<i8> {}

/// rustc 1.96.0 (new_range_from_api): the by-value iterator over a `RangeFrom`.
#[derive(Debug, Clone)]
pub struct RangeFromIter<A> {
    start: A,
    /// Whether the maximum value of the iterator has yielded.
    /// Only used when overflow checks are enabled.
    exhausted: bool,
}

/// rustc 1.96.0 (new_range_from_api): iterates forward from the start, without an end.
impl<A: Step> Iterator for RangeFromIter<A> {
    type Item = A;

    #[rustc_inherit_overflow_checks]
    fn next(&mut self) -> Option<A> {
        if self.exhausted {
            // This should panic if overflow checks are enabled, since
            // `forward_checked` returned `None` in prior iteration.
            self.start = Step::forward(self.start.clone(), 1);

            // If we get here, if means this iterator was exhausted by a crate
            // with overflow-checks enabled, but now we're iterating in a crate with
            // overflow-checks disabled. Since we successfully incremented `self.start`
            // above (in many cases this will wrap around to MIN), we now unset
            // the flag so we don't repeat this process in the next iteration.
            //
            // This could also happen if `forward_checked` returned None but
            // (for whatever reason, not applicable to any std implementors)
            // `forward` doesn't panic when overflow-checks are enabled. In that
            // case, this is also the correct behavior.
            self.exhausted = false;
        }
        if intrinsics::overflow_checks() {
            let Some(n) = Step::forward_checked(self.start.clone(), 1) else {
                self.exhausted = true;
                return Some(self.start.clone());
            };
            return Some(mem::replace(&mut self.start, n));
        }

        let n = Step::forward(self.start.clone(), 1);
        Some(mem::replace(&mut self.start, n))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }

    #[rustc_inherit_overflow_checks]
    fn nth(&mut self, n: usize) -> Option<A> {
        // Typically `forward` will cause an overflow-check panic here,
        // but unset the exhausted flag to handle the uncommon cases.
        // See the comments in `next` for more details.
        if self.exhausted {
            self.start = Step::forward(self.start.clone(), 1);
            self.exhausted = false;
        }
        if intrinsics::overflow_checks() {
            let plus_n = Step::forward(self.start.clone(), n);
            if let Some(plus_n1) = Step::forward_checked(plus_n.clone(), 1) {
                self.start = plus_n1;
            } else {
                self.start = plus_n.clone();
                self.exhausted = true;
            }
            return Some(plus_n);
        }

        let plus_n = Step::forward(self.start.clone(), n);
        self.start = Step::forward(plus_n.clone(), 1);
        Some(plus_n)
    }
}

/// rustc 1.96.0 (new_range_from_api): never ends.
impl<A: Step> FusedIterator for RangeFromIter<A> {}

/// rustc 1.96.0 (new_range_from_api): iterates over the owned range.
impl<A: Step> IntoIterator for RangeFrom<A> {
    type Item = A;
    type IntoIter = RangeFromIter<A>;

    fn into_iter(self) -> Self::IntoIter {
        RangeFromIter { start: self.start, exhausted: false }
    }
}
