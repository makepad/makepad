//! Range iteration through `Step` (implemented for the integer types in num/prim_traits.rs
//! and for char here).

use super::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, Iterator};
use crate::ops::{Range, RangeFrom, RangeInclusive};

pub trait Step: Clone + PartialOrd + Sized {
    /// (lower, upper) bounds on the number of successor steps from start to end;
    /// (0, None) if end < start never happens (callers check start < end first).
    fn steps_between(start: &Self, end: &Self) -> (usize, Option<usize>);
    fn forward_checked(start: Self, count: usize) -> Option<Self>;
    fn backward_checked(start: Self, count: usize) -> Option<Self>;
    fn forward(start: Self, count: usize) -> Self {
        match Step::forward_checked(start, count) {
            Some(v) => v,
            None => crate::panicking::panic_str("overflow in `Step::forward`"),
        }
    }
    fn backward(start: Self, count: usize) -> Self {
        match Step::backward_checked(start, count) {
            Some(v) => v,
            None => crate::panicking::panic_str("overflow in `Step::backward`"),
        }
    }
    /// `start + count` when the caller knows it stays in range (rustc: `Step::forward_unchecked`,
    /// used by `Range::next` after `start < end`): no overflow test in every range loop's step
    unsafe fn forward_unchecked(start: Self, count: usize) -> Self {
        Step::forward(start, count)
    }
    /// `start - count` when the caller knows it stays in range (rustc: `Step::backward_unchecked`)
    unsafe fn backward_unchecked(start: Self, count: usize) -> Self {
        Step::backward(start, count)
    }
}

impl Step for char {
    fn steps_between(start: &char, end: &char) -> (usize, Option<usize>) {
        let s = *start as u32;
        let e = *end as u32;
        if s <= e {
            let mut count = e - s;
            if s < 0xD800 && 0xE000 <= e {
                count -= 0x800;
            }
            (count as usize, Some(count as usize))
        } else {
            (0, None)
        }
    }
    fn forward_checked(start: char, count: usize) -> Option<char> {
        let s = start as u32;
        let mut r = (s as usize).checked_add(count)? as u64;
        if s < 0xD800 && 0xD800 <= r {
            r = r.checked_add(0x800)?;
        }
        if r > 0x10FFFF {
            return None;
        }
        crate::char::from_u32(r as u32)
    }
    fn backward_checked(start: char, count: usize) -> Option<char> {
        let s = start as u32;
        let mut r = (s as usize).checked_sub(count)?;
        if s >= 0xE000 && r < 0xE000 {
            r = r.checked_sub(0x800)?;
        }
        crate::char::from_u32(r as u32)
    }
}

impl<A: Step> Iterator for Range<A> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        if self.start < self.end {
            // SAFETY: start < end, so start + 1 <= end does not overflow (rustc's Range::next)
            let n = unsafe { Step::forward_unchecked(self.start.clone(), 1) };
            Some(crate::mem::replace(&mut self.start, n))
        } else {
            None
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.start < self.end {
            Step::steps_between(&self.start, &self.end)
        } else {
            (0, Some(0))
        }
    }
    // core range.rs: start moves past at most the items left, counted once
    fn advance_by(&mut self, n: usize) -> Result<(), usize> {
        let available = if self.start < self.end {
            let steps = Step::steps_between(&self.start, &self.end);
            steps.1.unwrap_or(steps.0)
        } else {
            0
        };
        let taken = if available < n { available } else { n };
        self.start = Step::forward_checked(self.start.clone(), taken).expect("`Step` invariants not upheld");
        if taken < n { Err(n - taken) } else { Ok(()) }
    }
    fn nth(&mut self, n: usize) -> Option<A> {
        if let Some(plus_n) = Step::forward_checked(self.start.clone(), n) {
            if plus_n < self.end {
                self.start = Step::forward(plus_n.clone(), 1);
                return Some(plus_n);
            }
        }
        self.start = self.end.clone();
        None
    }
    fn last(self) -> Option<A> {
        let mut s = self;
        s.next_back()
    }
    fn min(self) -> Option<A>
    where
        A: Ord,
    {
        let mut s = self;
        s.next()
    }
    fn max(self) -> Option<A>
    where
        A: Ord,
    {
        let mut s = self;
        s.next_back()
    }
    #[doc(hidden)]
    #[inline]
    unsafe fn __iterator_get_unchecked(&mut self, idx: usize) -> A {
        // SAFETY: TrustedRandomAccess callers pass an index below the range's length; the
        // ranges that are TrustedRandomAccess are of Copy integers, so even a repeated read of
        // one index would be sound
        unsafe { Step::forward_unchecked(self.start.clone(), idx) }
    }
}

impl<A: Step> DoubleEndedIterator for Range<A> {
    fn next_back(&mut self) -> Option<A> {
        if self.start < self.end {
            // SAFETY: start < end, so end - 1 >= start does not overflow
            self.end = unsafe { Step::backward_unchecked(self.end.clone(), 1) };
            Some(self.end.clone())
        } else {
            None
        }
    }
    // core range.rs: end moves back past at most the items left
    fn advance_back_by(&mut self, n: usize) -> Result<(), usize> {
        let available = if self.start < self.end {
            let steps = Step::steps_between(&self.start, &self.end);
            steps.1.unwrap_or(steps.0)
        } else {
            0
        };
        let taken = if available < n { available } else { n };
        self.end = Step::backward_checked(self.end.clone(), taken).expect("`Step` invariants not upheld");
        if taken < n { Err(n - taken) } else { Ok(()) }
    }
    fn nth_back(&mut self, n: usize) -> Option<A> {
        if let Some(minus_n) = Step::backward_checked(self.end.clone(), n) {
            if minus_n > self.start {
                self.end = Step::backward(minus_n, 1);
                return Some(self.end.clone());
            }
        }
        self.end = self.start.clone();
        None
    }
}

impl<A: Step> ExactSizeIterator for Range<A> {}
// real core: TrustedLen for Range<A: TrustedStep> (Step here is only the integers and char: TrustedStep's set)
unsafe impl<A: Step> super::TrustedLen for Range<A> {}
impl<A: Step> FusedIterator for Range<A> {}

// real core: the integer ranges whose length fits in usize are TrustedRandomAccess
macro_rules! range_trusted_random_access {
    ($($t:ty)*) => ($(
        unsafe impl super::TrustedRandomAccess for Range<$t> {}
        unsafe impl super::TrustedRandomAccessNoCoerce for Range<$t> {
            const MAY_HAVE_SIDE_EFFECT: bool = false;
        }
    )*)
}
range_trusted_random_access! { usize u8 u16 isize i8 i16 }
#[cfg(target_pointer_width = "32")]
range_trusted_random_access! { u32 i32 }
#[cfg(target_pointer_width = "64")]
range_trusted_random_access! { u32 i32 u64 i64 }

impl<A: Step> Iterator for RangeFrom<A> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        let n = Step::forward(self.start.clone(), 1);
        Some(crate::mem::replace(&mut self.start, n))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
    fn nth(&mut self, n: usize) -> Option<A> {
        let plus_n = Step::forward(self.start.clone(), n);
        self.start = Step::forward(plus_n.clone(), 1);
        Some(plus_n)
    }
}

impl<A: Step> FusedIterator for RangeFrom<A> {}

impl<A: Step> Iterator for RangeInclusive<A> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        if self.exhausted || !(self.start <= self.end) {
            return None;
        }
        let is_iterating = self.start < self.end;
        Some(if is_iterating {
            // SAFETY: start < end
            let n = unsafe { Step::forward_unchecked(self.start.clone(), 1) };
            crate::mem::replace(&mut self.start, n)
        } else {
            self.exhausted = true;
            self.start.clone()
        })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.exhausted || !(self.start <= self.end) {
            return (0, Some(0));
        }
        let (lo, hi) = Step::steps_between(&self.start, &self.end);
        (lo.saturating_add(1), match hi {
            Some(h) => h.checked_add(1),
            None => None,
        })
    }
    fn nth(&mut self, n: usize) -> Option<A> {
        if self.exhausted || !(self.start <= self.end) {
            return None;
        }
        if let Some(plus_n) = Step::forward_checked(self.start.clone(), n) {
            if plus_n < self.end {
                self.start = Step::forward(plus_n.clone(), 1);
                return Some(plus_n);
            }
            if plus_n == self.end {
                self.start = plus_n.clone();
                self.exhausted = true;
                return Some(plus_n);
            }
        }
        self.start = self.end.clone();
        self.exhausted = true;
        None
    }
    fn last(self) -> Option<A> {
        let mut s = self;
        s.next_back()
    }
    fn min(self) -> Option<A>
    where
        A: Ord,
    {
        let mut s = self;
        s.next()
    }
    fn max(self) -> Option<A>
    where
        A: Ord,
    {
        let mut s = self;
        s.next_back()
    }
}

impl<A: Step> DoubleEndedIterator for RangeInclusive<A> {
    fn next_back(&mut self) -> Option<A> {
        if self.exhausted || !(self.start <= self.end) {
            return None;
        }
        let is_iterating = self.start < self.end;
        Some(if is_iterating {
            // SAFETY: start < end
            let n = unsafe { Step::backward_unchecked(self.end.clone(), 1) };
            crate::mem::replace(&mut self.end, n)
        } else {
            self.exhausted = true;
            self.end.clone()
        })
    }
}

impl<A: Step> ExactSizeIterator for RangeInclusive<A> {}
unsafe impl<A: Step> super::TrustedLen for RangeInclusive<A> {}
impl<A: Step> FusedIterator for RangeInclusive<A> {}
