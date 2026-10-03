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
            let n = Step::forward(self.start.clone(), 1);
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
}

impl<A: Step> DoubleEndedIterator for Range<A> {
    fn next_back(&mut self) -> Option<A> {
        if self.start < self.end {
            self.end = Step::backward(self.end.clone(), 1);
            Some(self.end.clone())
        } else {
            None
        }
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
impl<A: Step> FusedIterator for Range<A> {}

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
            let n = Step::forward(self.start.clone(), 1);
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
            let n = Step::backward(self.end.clone(), 1);
            crate::mem::replace(&mut self.end, n)
        } else {
            self.exhausted = true;
            self.end.clone()
        })
    }
}

impl<A: Step> ExactSizeIterator for RangeInclusive<A> {}
impl<A: Step> FusedIterator for RangeInclusive<A> {}
