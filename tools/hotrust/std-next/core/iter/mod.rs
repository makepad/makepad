//! Iterators: the general (non-fused) path. HotRust fuses std adapter chains over slices,
//! ranges, Vec, str and HashMap into loops at lowering time (R5); these definitions are what
//! user-defined iterators and everything else go through, with real core's semantics.

mod adapters;
mod range;
mod sources;

pub use adapters::{
    Chain, Cloned, Copied, Cycle, Enumerate, Filter, FilterMap, FlatMap, Flatten, Fuse, Inspect, Map, MapWhile,
    Peekable, Rev, Scan, Skip, SkipWhile, StepBy, Take, TakeWhile, Zip,
};
pub use range::Step;
pub use sources::{
    empty, from_fn, once, once_with, repeat, repeat_n, repeat_with, successors, zip, Empty, FromFn, Once, OnceWith,
    Repeat, RepeatN, RepeatWith, Successors,
};

use crate::cmp::Ordering;
use crate::ops::{Add, Mul};

pub trait Iterator {
    type Item;

    fn next(&mut self) -> Option<Self::Item>;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, None)
    }

    fn count(self) -> usize
    where
        Self: Sized,
    {
        let mut n = 0;
        let mut s = self;
        while let Some(_) = s.next() {
            n += 1;
        }
        n
    }

    fn last(self) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut last = None;
        let mut s = self;
        while let Some(x) = s.next() {
            last = Some(x);
        }
        last
    }

    fn advance_by(&mut self, n: usize) -> Result<(), usize> {
        let mut i = 0;
        while i < n {
            if self.next().is_none() {
                return Err(n - i);
            }
            i += 1;
        }
        Ok(())
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        if self.advance_by(n).is_err() {
            return None;
        }
        self.next()
    }

    fn step_by(self, step: usize) -> StepBy<Self>
    where
        Self: Sized,
    {
        StepBy::new(self, step)
    }

    fn chain<U: IntoIterator<Item = Self::Item>>(self, other: U) -> Chain<Self, U::IntoIter>
    where
        Self: Sized,
    {
        Chain::new(self, other.into_iter())
    }

    fn zip<U: IntoIterator>(self, other: U) -> Zip<Self, U::IntoIter>
    where
        Self: Sized,
    {
        Zip::new(self, other.into_iter())
    }

    fn map<B, F: FnMut(Self::Item) -> B>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
    {
        Map::new(self, f)
    }

    fn for_each<F: FnMut(Self::Item)>(self, f: F)
    where
        Self: Sized,
    {
        let mut f = f;
        let mut s = self;
        while let Some(x) = s.next() {
            f(x);
        }
    }

    fn filter<P: FnMut(&Self::Item) -> bool>(self, predicate: P) -> Filter<Self, P>
    where
        Self: Sized,
    {
        Filter::new(self, predicate)
    }

    fn filter_map<B, F: FnMut(Self::Item) -> Option<B>>(self, f: F) -> FilterMap<Self, F>
    where
        Self: Sized,
    {
        FilterMap::new(self, f)
    }

    fn enumerate(self) -> Enumerate<Self>
    where
        Self: Sized,
    {
        Enumerate::new(self)
    }

    fn peekable(self) -> Peekable<Self>
    where
        Self: Sized,
    {
        Peekable::new(self)
    }

    fn skip_while<P: FnMut(&Self::Item) -> bool>(self, predicate: P) -> SkipWhile<Self, P>
    where
        Self: Sized,
    {
        SkipWhile::new(self, predicate)
    }

    fn take_while<P: FnMut(&Self::Item) -> bool>(self, predicate: P) -> TakeWhile<Self, P>
    where
        Self: Sized,
    {
        TakeWhile::new(self, predicate)
    }

    fn map_while<B, P: FnMut(Self::Item) -> Option<B>>(self, predicate: P) -> MapWhile<Self, P>
    where
        Self: Sized,
    {
        MapWhile::new(self, predicate)
    }

    fn skip(self, n: usize) -> Skip<Self>
    where
        Self: Sized,
    {
        Skip::new(self, n)
    }

    fn take(self, n: usize) -> Take<Self>
    where
        Self: Sized,
    {
        Take::new(self, n)
    }

    fn scan<St, B, F: FnMut(&mut St, Self::Item) -> Option<B>>(self, initial_state: St, f: F) -> Scan<Self, St, F>
    where
        Self: Sized,
    {
        Scan::new(self, initial_state, f)
    }

    fn flat_map<U: IntoIterator, F: FnMut(Self::Item) -> U>(self, f: F) -> FlatMap<Self, U, F>
    where
        Self: Sized,
    {
        FlatMap::new(self, f)
    }

    fn flatten(self) -> Flatten<Self>
    where
        Self: Sized,
        Self::Item: IntoIterator,
    {
        Flatten::new(self)
    }

    fn fuse(self) -> Fuse<Self>
    where
        Self: Sized,
    {
        Fuse::new(self)
    }

    fn inspect<F: FnMut(&Self::Item)>(self, f: F) -> Inspect<Self, F>
    where
        Self: Sized,
    {
        Inspect::new(self, f)
    }

    fn by_ref(&mut self) -> &mut Self
    where
        Self: Sized,
    {
        self
    }

    fn collect<B: FromIterator<Self::Item>>(self) -> B
    where
        Self: Sized,
    {
        B::from_iter(self)
    }

    fn partition<B: Default + Extend<Self::Item>, F: FnMut(&Self::Item) -> bool>(self, f: F) -> (B, B)
    where
        Self: Sized,
    {
        let mut f = f;
        let mut left: B = B::default();
        let mut right: B = B::default();
        let mut s = self;
        while let Some(x) = s.next() {
            if f(&x) {
                left.extend_one(x);
            } else {
                right.extend_one(x);
            }
        }
        (left, right)
    }

    fn fold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, f: F) -> B
    where
        Self: Sized,
    {
        let mut f = f;
        let mut acc = init;
        let mut s = self;
        while let Some(x) = s.next() {
            acc = f(acc, x);
        }
        acc
    }

    fn reduce<F: FnMut(Self::Item, Self::Item) -> Self::Item>(self, f: F) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut s = self;
        let first = s.next()?;
        Some(s.fold(first, f))
    }

    fn all<F: FnMut(Self::Item) -> bool>(&mut self, f: F) -> bool
    where
        Self: Sized,
    {
        let mut f = f;
        while let Some(x) = self.next() {
            if !f(x) {
                return false;
            }
        }
        true
    }

    fn any<F: FnMut(Self::Item) -> bool>(&mut self, f: F) -> bool
    where
        Self: Sized,
    {
        let mut f = f;
        while let Some(x) = self.next() {
            if f(x) {
                return true;
            }
        }
        false
    }

    fn find<P: FnMut(&Self::Item) -> bool>(&mut self, predicate: P) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut predicate = predicate;
        while let Some(x) = self.next() {
            if predicate(&x) {
                return Some(x);
            }
        }
        None
    }

    fn find_map<B, F: FnMut(Self::Item) -> Option<B>>(&mut self, f: F) -> Option<B>
    where
        Self: Sized,
    {
        let mut f = f;
        while let Some(x) = self.next() {
            if let Some(b) = f(x) {
                return Some(b);
            }
        }
        None
    }

    fn position<P: FnMut(Self::Item) -> bool>(&mut self, predicate: P) -> Option<usize>
    where
        Self: Sized,
    {
        let mut predicate = predicate;
        let mut i = 0;
        while let Some(x) = self.next() {
            if predicate(x) {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    fn rposition<P: FnMut(Self::Item) -> bool>(&mut self, predicate: P) -> Option<usize>
    where
        Self: Sized + ExactSizeIterator + DoubleEndedIterator,
    {
        let mut predicate = predicate;
        let mut i = self.len();
        while let Some(x) = self.next_back() {
            i -= 1;
            if predicate(x) {
                return Some(i);
            }
        }
        None
    }

    fn max(self) -> Option<Self::Item>
    where
        Self: Sized,
        Self::Item: Ord,
    {
        self.max_by(Ord::cmp)
    }

    fn min(self) -> Option<Self::Item>
    where
        Self: Sized,
        Self::Item: Ord,
    {
        self.min_by(Ord::cmp)
    }

    fn max_by_key<B: Ord, F: FnMut(&Self::Item) -> B>(self, f: F) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut f = f;
        let mut s = self;
        let mut best = s.next()?;
        let mut best_key = f(&best);
        while let Some(x) = s.next() {
            let k = f(&x);
            // ties keep the last element, as real core
            if k >= best_key {
                best = x;
                best_key = k;
            }
        }
        Some(best)
    }

    fn max_by<F: FnMut(&Self::Item, &Self::Item) -> Ordering>(self, compare: F) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut compare = compare;
        let mut s = self;
        let mut best = s.next()?;
        while let Some(x) = s.next() {
            if compare(&best, &x) != Ordering::Greater {
                best = x;
            }
        }
        Some(best)
    }

    fn min_by_key<B: Ord, F: FnMut(&Self::Item) -> B>(self, f: F) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut f = f;
        let mut s = self;
        let mut best = s.next()?;
        let mut best_key = f(&best);
        while let Some(x) = s.next() {
            let k = f(&x);
            // ties keep the first element
            if k < best_key {
                best = x;
                best_key = k;
            }
        }
        Some(best)
    }

    fn min_by<F: FnMut(&Self::Item, &Self::Item) -> Ordering>(self, compare: F) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut compare = compare;
        let mut s = self;
        let mut best = s.next()?;
        while let Some(x) = s.next() {
            if compare(&best, &x) == Ordering::Greater {
                best = x;
            }
        }
        Some(best)
    }

    fn rev(self) -> Rev<Self>
    where
        Self: Sized + DoubleEndedIterator,
    {
        Rev::new(self)
    }

    fn unzip<A, B, FromA: Default + Extend<A>, FromB: Default + Extend<B>>(self) -> (FromA, FromB)
    where
        Self: Sized + Iterator<Item = (A, B)>,
    {
        let mut a: FromA = FromA::default();
        let mut b: FromB = FromB::default();
        let mut s = self;
        while let Some((x, y)) = s.next() {
            a.extend_one(x);
            b.extend_one(y);
        }
        (a, b)
    }

    fn copied<'a, T: 'a + Copy>(self) -> Copied<Self>
    where
        Self: Sized + Iterator<Item = &'a T>,
    {
        Copied::new(self)
    }

    fn cloned<'a, T: 'a + Clone>(self) -> Cloned<Self>
    where
        Self: Sized + Iterator<Item = &'a T>,
    {
        Cloned::new(self)
    }

    fn cycle(self) -> Cycle<Self>
    where
        Self: Sized + Clone,
    {
        Cycle::new(self)
    }

    fn sum<S: Sum<Self::Item>>(self) -> S
    where
        Self: Sized,
    {
        S::sum(self)
    }

    fn product<P: Product<Self::Item>>(self) -> P
    where
        Self: Sized,
    {
        P::product(self)
    }

    fn cmp<I: IntoIterator<Item = Self::Item>>(self, other: I) -> Ordering
    where
        Self::Item: Ord,
        Self: Sized,
    {
        let mut a = self;
        let mut b = other.into_iter();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Ordering::Equal,
                (None, Some(_)) => return Ordering::Less,
                (Some(_), None) => return Ordering::Greater,
                (Some(x), Some(y)) => match x.cmp(&y) {
                    Ordering::Equal => {}
                    o => return o,
                },
            }
        }
    }

    fn partial_cmp<I: IntoIterator>(self, other: I) -> Option<Ordering>
    where
        Self::Item: PartialOrd<I::Item>,
        Self: Sized,
    {
        let mut a = self;
        let mut b = other.into_iter();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return Some(Ordering::Equal),
                (None, Some(_)) => return Some(Ordering::Less),
                (Some(_), None) => return Some(Ordering::Greater),
                (Some(x), Some(y)) => match x.partial_cmp(&y) {
                    Some(Ordering::Equal) => {}
                    o => return o,
                },
            }
        }
    }

    fn eq<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialEq<I::Item>,
        Self: Sized,
    {
        let mut a = self;
        let mut b = other.into_iter();
        loop {
            match (a.next(), b.next()) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if x != y {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    fn ne<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialEq<I::Item>,
        Self: Sized,
    {
        !self.eq(other)
    }

    fn lt<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialOrd<I::Item>,
        Self: Sized,
    {
        self.partial_cmp(other) == Some(Ordering::Less)
    }

    fn le<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialOrd<I::Item>,
        Self: Sized,
    {
        matches!(self.partial_cmp(other), Some(Ordering::Less | Ordering::Equal))
    }

    fn gt<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialOrd<I::Item>,
        Self: Sized,
    {
        self.partial_cmp(other) == Some(Ordering::Greater)
    }

    fn ge<I: IntoIterator>(self, other: I) -> bool
    where
        Self::Item: PartialOrd<I::Item>,
        Self: Sized,
    {
        matches!(self.partial_cmp(other), Some(Ordering::Greater | Ordering::Equal))
    }

    fn is_sorted(self) -> bool
    where
        Self: Sized,
        Self::Item: PartialOrd,
    {
        self.is_sorted_by(|a, b| a <= b)
    }

    fn is_sorted_by<F: FnMut(&Self::Item, &Self::Item) -> bool>(self, compare: F) -> bool
    where
        Self: Sized,
    {
        let mut compare = compare;
        let mut s = self;
        let mut last = match s.next() {
            Some(e) => e,
            None => return true,
        };
        while let Some(x) = s.next() {
            if !compare(&last, &x) {
                return false;
            }
            last = x;
        }
        true
    }

    fn is_sorted_by_key<K: PartialOrd, F: FnMut(Self::Item) -> K>(self, f: F) -> bool
    where
        Self: Sized,
    {
        self.map(f).is_sorted()
    }
}

impl<'a, I: Iterator + ?Sized> Iterator for &'a mut I {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        (**self).next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}

impl<'a, I: DoubleEndedIterator + ?Sized> DoubleEndedIterator for &'a mut I {
    fn next_back(&mut self) -> Option<I::Item> {
        (**self).next_back()
    }
}

impl<'a, I: ExactSizeIterator + ?Sized> ExactSizeIterator for &'a mut I {}

pub trait DoubleEndedIterator: Iterator {
    fn next_back(&mut self) -> Option<Self::Item>;

    fn advance_back_by(&mut self, n: usize) -> Result<(), usize> {
        let mut i = 0;
        while i < n {
            if self.next_back().is_none() {
                return Err(n - i);
            }
            i += 1;
        }
        Ok(())
    }

    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        if self.advance_back_by(n).is_err() {
            return None;
        }
        self.next_back()
    }

    fn rfold<B, F: FnMut(B, Self::Item) -> B>(self, init: B, f: F) -> B
    where
        Self: Sized,
    {
        let mut f = f;
        let mut acc = init;
        let mut s = self;
        while let Some(x) = s.next_back() {
            acc = f(acc, x);
        }
        acc
    }

    fn rfind<P: FnMut(&Self::Item) -> bool>(&mut self, predicate: P) -> Option<Self::Item>
    where
        Self: Sized,
    {
        let mut predicate = predicate;
        while let Some(x) = self.next_back() {
            if predicate(&x) {
                return Some(x);
            }
        }
        None
    }
}

pub trait ExactSizeIterator: Iterator {
    fn len(&self) -> usize {
        let (lower, _upper) = self.size_hint();
        lower
    }
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub trait FusedIterator: Iterator {}

pub unsafe trait TrustedLen: Iterator {}

pub trait IntoIterator {
    type Item;
    type IntoIter: Iterator<Item = Self::Item>;
    fn into_iter(self) -> Self::IntoIter;
}

impl<I: Iterator> IntoIterator for I {
    type Item = I::Item;
    type IntoIter = I;
    fn into_iter(self) -> I {
        self
    }
}

pub trait FromIterator<A>: Sized {
    fn from_iter<T: IntoIterator<Item = A>>(iter: T) -> Self;
}

pub trait Extend<A> {
    fn extend<T: IntoIterator<Item = A>>(&mut self, iter: T);
    fn extend_one(&mut self, item: A) {
        self.extend(Some(item));
    }
    fn extend_reserve(&mut self, additional: usize) {
        let _ = additional;
    }
}

impl Extend<()> for () {
    fn extend<T: IntoIterator<Item = ()>>(&mut self, iter: T) {
        iter.into_iter().for_each(drop)
    }
}

impl FromIterator<()> for () {
    fn from_iter<T: IntoIterator<Item = ()>>(iter: T) {
        iter.into_iter().for_each(drop)
    }
}

impl<A, B, EA: Extend<A>, EB: Extend<B>> Extend<(A, B)> for (EA, EB) {
    fn extend<T: IntoIterator<Item = (A, B)>>(&mut self, iter: T) {
        for (a, b) in iter {
            self.0.extend_one(a);
            self.1.extend_one(b);
        }
    }
}

pub trait Sum<A = Self>: Sized {
    fn sum<I: Iterator<Item = A>>(iter: I) -> Self;
}

pub trait Product<A = Self>: Sized {
    fn product<I: Iterator<Item = A>>(iter: I) -> Self;
}

/// Shared bodies for the generated Sum/Product impls of numeric types.
pub(crate) fn sum_from<T: Add<Output = T>, I: Iterator<Item = T>>(zero: T, iter: I) -> T {
    iter.fold(zero, |a, b| a + b)
}

pub(crate) fn product_from<T: Mul<Output = T>, I: Iterator<Item = T>>(one: T, iter: I) -> T {
    iter.fold(one, |a, b| a * b)
}
