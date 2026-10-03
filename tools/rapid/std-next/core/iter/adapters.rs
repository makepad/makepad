//! Iterator adapters (general path).

use super::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, IntoIterator, Iterator};

// ---------------------------------------------------------------- Map

#[derive(Clone)]
pub struct Map<I, F> {
    iter: I,
    f: F,
}

impl<I, F> Map<I, F> {
    pub(crate) fn new(iter: I, f: F) -> Map<I, F> {
        Map { iter, f }
    }
}

impl<B, I: Iterator, F: FnMut(I::Item) -> B> Iterator for Map<I, F> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        match self.iter.next() {
            Some(x) => Some((self.f)(x)),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<B, I: DoubleEndedIterator, F: FnMut(I::Item) -> B> DoubleEndedIterator for Map<I, F> {
    fn next_back(&mut self) -> Option<B> {
        match self.iter.next_back() {
            Some(x) => Some((self.f)(x)),
            None => None,
        }
    }
}

impl<B, I: ExactSizeIterator, F: FnMut(I::Item) -> B> ExactSizeIterator for Map<I, F> {}
impl<B, I: FusedIterator, F: FnMut(I::Item) -> B> FusedIterator for Map<I, F> {}

// ---------------------------------------------------------------- Filter

#[derive(Clone)]
pub struct Filter<I, P> {
    iter: I,
    predicate: P,
}

impl<I, P> Filter<I, P> {
    pub(crate) fn new(iter: I, predicate: P) -> Filter<I, P> {
        Filter { iter, predicate }
    }
}

impl<I: Iterator, P: FnMut(&I::Item) -> bool> Iterator for Filter<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        while let Some(x) = self.iter.next() {
            if (self.predicate)(&x) {
                return Some(x);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

impl<I: DoubleEndedIterator, P: FnMut(&I::Item) -> bool> DoubleEndedIterator for Filter<I, P> {
    fn next_back(&mut self) -> Option<I::Item> {
        while let Some(x) = self.iter.next_back() {
            if (self.predicate)(&x) {
                return Some(x);
            }
        }
        None
    }
}

impl<I: FusedIterator, P: FnMut(&I::Item) -> bool> FusedIterator for Filter<I, P> {}

// ---------------------------------------------------------------- FilterMap

#[derive(Clone)]
pub struct FilterMap<I, F> {
    iter: I,
    f: F,
}

impl<I, F> FilterMap<I, F> {
    pub(crate) fn new(iter: I, f: F) -> FilterMap<I, F> {
        FilterMap { iter, f }
    }
}

impl<B, I: Iterator, F: FnMut(I::Item) -> Option<B>> Iterator for FilterMap<I, F> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        while let Some(x) = self.iter.next() {
            if let Some(y) = (self.f)(x) {
                return Some(y);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

impl<B, I: DoubleEndedIterator, F: FnMut(I::Item) -> Option<B>> DoubleEndedIterator for FilterMap<I, F> {
    fn next_back(&mut self) -> Option<B> {
        while let Some(x) = self.iter.next_back() {
            if let Some(y) = (self.f)(x) {
                return Some(y);
            }
        }
        None
    }
}

// ---------------------------------------------------------------- Enumerate

#[derive(Clone, Debug)]
pub struct Enumerate<I> {
    iter: I,
    count: usize,
}

impl<I> Enumerate<I> {
    pub(crate) fn new(iter: I) -> Enumerate<I> {
        Enumerate { iter, count: 0 }
    }
}

impl<I: Iterator> Iterator for Enumerate<I> {
    type Item = (usize, I::Item);
    fn next(&mut self) -> Option<(usize, I::Item)> {
        let a = self.iter.next()?;
        let i = self.count;
        self.count += 1;
        Some((i, a))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
    fn nth(&mut self, n: usize) -> Option<(usize, I::Item)> {
        let a = self.iter.nth(n)?;
        let i = self.count + n;
        self.count = i + 1;
        Some((i, a))
    }
}

impl<I: ExactSizeIterator + DoubleEndedIterator> DoubleEndedIterator for Enumerate<I> {
    fn next_back(&mut self) -> Option<(usize, I::Item)> {
        let a = self.iter.next_back()?;
        let len = self.iter.len();
        Some((self.count + len, a))
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for Enumerate<I> {}
impl<I: FusedIterator> FusedIterator for Enumerate<I> {}

// ---------------------------------------------------------------- Zip

#[derive(Clone, Debug)]
pub struct Zip<A, B> {
    a: A,
    b: B,
}

impl<A, B> Zip<A, B> {
    pub(crate) fn new(a: A, b: B) -> Zip<A, B> {
        Zip { a, b }
    }
}

impl<A: Iterator, B: Iterator> Iterator for Zip<A, B> {
    type Item = (A::Item, B::Item);
    fn next(&mut self) -> Option<(A::Item, B::Item)> {
        let x = self.a.next()?;
        let y = self.b.next()?;
        Some((x, y))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (al, au) = self.a.size_hint();
        let (bl, bu) = self.b.size_hint();
        let lower = if al < bl { al } else { bl };
        let upper = match (au, bu) {
            (Some(x), Some(y)) => Some(if x < y { x } else { y }),
            (Some(x), None) => Some(x),
            (None, Some(y)) => Some(y),
            (None, None) => None,
        };
        (lower, upper)
    }
}

impl<A, B> DoubleEndedIterator for Zip<A, B>
where
    A: DoubleEndedIterator + ExactSizeIterator,
    B: DoubleEndedIterator + ExactSizeIterator,
{
    fn next_back(&mut self) -> Option<(A::Item, B::Item)> {
        let mut la = self.a.len();
        let mut lb = self.b.len();
        while la > lb {
            self.a.next_back();
            la -= 1;
        }
        while lb > la {
            self.b.next_back();
            lb -= 1;
        }
        match (self.a.next_back(), self.b.next_back()) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
    }
}

impl<A: ExactSizeIterator, B: ExactSizeIterator> ExactSizeIterator for Zip<A, B> {}
impl<A: FusedIterator, B: FusedIterator> FusedIterator for Zip<A, B> {}

// ---------------------------------------------------------------- Rev

#[derive(Clone, Debug)]
pub struct Rev<I> {
    iter: I,
}

impl<I> Rev<I> {
    pub(crate) fn new(iter: I) -> Rev<I> {
        Rev { iter }
    }
}

impl<I: DoubleEndedIterator> Iterator for Rev<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        self.iter.next_back()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
    fn nth(&mut self, n: usize) -> Option<I::Item> {
        self.iter.nth_back(n)
    }
}

impl<I: DoubleEndedIterator> DoubleEndedIterator for Rev<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        self.iter.next()
    }
    fn nth_back(&mut self, n: usize) -> Option<I::Item> {
        self.iter.nth(n)
    }
}

impl<I: DoubleEndedIterator + ExactSizeIterator> ExactSizeIterator for Rev<I> {}
impl<I: DoubleEndedIterator + FusedIterator> FusedIterator for Rev<I> {}

// ---------------------------------------------------------------- Take / Skip

#[derive(Clone, Debug)]
pub struct Take<I> {
    iter: I,
    n: usize,
}

impl<I> Take<I> {
    pub(crate) fn new(iter: I, n: usize) -> Take<I> {
        Take { iter, n }
    }
}

impl<I: Iterator> Iterator for Take<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.n != 0 {
            self.n -= 1;
            self.iter.next()
        } else {
            None
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.n == 0 {
            return (0, Some(0));
        }
        let (lower, upper) = self.iter.size_hint();
        let lower = if lower < self.n { lower } else { self.n };
        let upper = match upper {
            Some(x) if x < self.n => Some(x),
            _ => Some(self.n),
        };
        (lower, upper)
    }
    fn nth(&mut self, n: usize) -> Option<I::Item> {
        if self.n > n {
            self.n -= n + 1;
            self.iter.nth(n)
        } else {
            if self.n > 0 {
                self.iter.nth(self.n - 1);
                self.n = 0;
            }
            None
        }
    }
}

impl<I: DoubleEndedIterator + ExactSizeIterator> DoubleEndedIterator for Take<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        if self.n == 0 {
            return None;
        }
        let n = self.n;
        self.n -= 1;
        let skip = self.iter.len().saturating_sub(n);
        self.iter.nth_back(skip)
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for Take<I> {}
impl<I: FusedIterator> FusedIterator for Take<I> {}

#[derive(Clone, Debug)]
pub struct Skip<I> {
    iter: I,
    n: usize,
}

impl<I> Skip<I> {
    pub(crate) fn new(iter: I, n: usize) -> Skip<I> {
        Skip { iter, n }
    }
}

impl<I: Iterator> Iterator for Skip<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.n > 0 {
            let n = self.n;
            self.n = 0;
            self.iter.nth(n)
        } else {
            self.iter.next()
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.iter.size_hint();
        let lower = lower.saturating_sub(self.n);
        let upper = match upper {
            Some(x) => Some(x.saturating_sub(self.n)),
            None => None,
        };
        (lower, upper)
    }
}

impl<I: DoubleEndedIterator + ExactSizeIterator> DoubleEndedIterator for Skip<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        if self.len() > 0 {
            self.iter.next_back()
        } else {
            None
        }
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for Skip<I> {}
impl<I: FusedIterator> FusedIterator for Skip<I> {}

// ---------------------------------------------------------------- StepBy

#[derive(Clone, Debug)]
pub struct StepBy<I> {
    iter: I,
    step_minus_one: usize,
    first_take: bool,
}

impl<I> StepBy<I> {
    pub(crate) fn new(iter: I, step: usize) -> StepBy<I> {
        if step == 0 {
            crate::panicking::panic_str("assertion failed: step != 0");
        }
        StepBy { iter, step_minus_one: step - 1, first_take: true }
    }
}

impl<I: Iterator> Iterator for StepBy<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        let step = if self.first_take { 0 } else { self.step_minus_one };
        self.first_take = false;
        self.iter.nth(step)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let step = self.step_minus_one + 1;
        let (lower, upper) = self.iter.size_hint();
        if self.first_take {
            let f = |n: usize| if n == 0 { 0 } else { 1 + (n - 1) / step };
            (f(lower), upper.map(f))
        } else {
            let f = |n: usize| n / step;
            (f(lower), upper.map(f))
        }
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for StepBy<I> {}

// ---------------------------------------------------------------- Chain

#[derive(Clone, Debug)]
pub struct Chain<A, B> {
    a: Option<A>,
    b: Option<B>,
}

impl<A, B> Chain<A, B> {
    pub(crate) fn new(a: A, b: B) -> Chain<A, B> {
        Chain { a: Some(a), b: Some(b) }
    }
}

impl<A: Iterator, B: Iterator<Item = A::Item>> Iterator for Chain<A, B> {
    type Item = A::Item;
    fn next(&mut self) -> Option<A::Item> {
        if let Some(a) = &mut self.a {
            match a.next() {
                None => self.a = None,
                item => return item,
            }
        }
        match &mut self.b {
            Some(b) => b.next(),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (al, au) = match &self.a {
            Some(a) => a.size_hint(),
            None => (0, Some(0)),
        };
        let (bl, bu) = match &self.b {
            Some(b) => b.size_hint(),
            None => (0, Some(0)),
        };
        let lower = al.saturating_add(bl);
        let upper = match (au, bu) {
            (Some(x), Some(y)) => x.checked_add(y),
            _ => None,
        };
        (lower, upper)
    }
}

impl<A: DoubleEndedIterator, B: DoubleEndedIterator<Item = A::Item>> DoubleEndedIterator for Chain<A, B> {
    fn next_back(&mut self) -> Option<A::Item> {
        if let Some(b) = &mut self.b {
            match b.next_back() {
                None => self.b = None,
                item => return item,
            }
        }
        match &mut self.a {
            Some(a) => a.next_back(),
            None => None,
        }
    }
}

impl<A: FusedIterator, B: FusedIterator<Item = A::Item>> FusedIterator for Chain<A, B> {}

// ---------------------------------------------------------------- Cloned / Copied

#[derive(Clone, Debug)]
pub struct Cloned<I> {
    it: I,
}

impl<I> Cloned<I> {
    pub(crate) fn new(it: I) -> Cloned<I> {
        Cloned { it }
    }
}

impl<'a, T: 'a + Clone, I: Iterator<Item = &'a T>> Iterator for Cloned<I> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.it.next().cloned()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
}

impl<'a, T: 'a + Clone, I: DoubleEndedIterator<Item = &'a T>> DoubleEndedIterator for Cloned<I> {
    fn next_back(&mut self) -> Option<T> {
        self.it.next_back().cloned()
    }
}

impl<'a, T: 'a + Clone, I: ExactSizeIterator<Item = &'a T>> ExactSizeIterator for Cloned<I> {}

#[derive(Clone, Debug)]
pub struct Copied<I> {
    it: I,
}

impl<I> Copied<I> {
    pub(crate) fn new(it: I) -> Copied<I> {
        Copied { it }
    }
}

impl<'a, T: 'a + Copy, I: Iterator<Item = &'a T>> Iterator for Copied<I> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.it.next().copied()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.it.size_hint()
    }
    fn nth(&mut self, n: usize) -> Option<T> {
        self.it.nth(n).copied()
    }
}

impl<'a, T: 'a + Copy, I: DoubleEndedIterator<Item = &'a T>> DoubleEndedIterator for Copied<I> {
    fn next_back(&mut self) -> Option<T> {
        self.it.next_back().copied()
    }
}

impl<'a, T: 'a + Copy, I: ExactSizeIterator<Item = &'a T>> ExactSizeIterator for Copied<I> {}

// ---------------------------------------------------------------- Peekable

#[derive(Clone, Debug)]
pub struct Peekable<I: Iterator> {
    iter: I,
    /// Some(None) = peeked the end
    peeked: Option<Option<I::Item>>,
}

impl<I: Iterator> Peekable<I> {
    pub(crate) fn new(iter: I) -> Peekable<I> {
        Peekable { iter, peeked: None }
    }
    pub fn peek(&mut self) -> Option<&I::Item> {
        if self.peeked.is_none() {
            self.peeked = Some(self.iter.next());
        }
        match &self.peeked {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }
    pub fn peek_mut(&mut self) -> Option<&mut I::Item> {
        if self.peeked.is_none() {
            self.peeked = Some(self.iter.next());
        }
        match &mut self.peeked {
            Some(Some(v)) => Some(v),
            _ => None,
        }
    }
    pub fn next_if<F: FnOnce(&I::Item) -> bool>(&mut self, func: F) -> Option<I::Item> {
        let next = match self.peeked.take() {
            Some(v) => v,
            None => self.iter.next(),
        };
        match next {
            Some(matched) => {
                if func(&matched) {
                    Some(matched)
                } else {
                    self.peeked = Some(Some(matched));
                    None
                }
            }
            None => {
                self.peeked = Some(None);
                None
            }
        }
    }
    pub fn next_if_eq<T: ?Sized>(&mut self, expected: &T) -> Option<I::Item>
    where
        I::Item: PartialEq<T>,
    {
        self.next_if(|next| next == expected)
    }
}

impl<I: Iterator> Iterator for Peekable<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        match self.peeked.take() {
            Some(v) => v,
            None => self.iter.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let peek_len = match self.peeked {
            Some(None) => return (0, Some(0)),
            Some(Some(_)) => 1,
            None => 0,
        };
        let (lo, hi) = self.iter.size_hint();
        let lo = lo.saturating_add(peek_len);
        let hi = match hi {
            Some(x) => x.checked_add(peek_len),
            None => None,
        };
        (lo, hi)
    }
    fn count(self) -> usize {
        match self.peeked {
            Some(None) => 0,
            Some(Some(_)) => 1 + self.iter.count(),
            None => self.iter.count(),
        }
    }
    fn last(self) -> Option<I::Item> {
        let peek_opt = match self.peeked {
            Some(None) => return None,
            Some(v) => v,
            None => None,
        };
        match self.iter.last() {
            Some(x) => Some(x),
            None => peek_opt,
        }
    }
}

impl<I: DoubleEndedIterator> DoubleEndedIterator for Peekable<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        match &mut self.peeked {
            Some(None) => None,
            Some(Some(_)) => match self.iter.next_back() {
                Some(x) => Some(x),
                None => match self.peeked.take() {
                    Some(v) => v,
                    None => None,
                },
            },
            None => self.iter.next_back(),
        }
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for Peekable<I> {}
impl<I: FusedIterator> FusedIterator for Peekable<I> {}

// ---------------------------------------------------------------- SkipWhile / TakeWhile / MapWhile

#[derive(Clone)]
pub struct SkipWhile<I, P> {
    iter: I,
    flag: bool,
    predicate: P,
}

impl<I, P> SkipWhile<I, P> {
    pub(crate) fn new(iter: I, predicate: P) -> SkipWhile<I, P> {
        SkipWhile { iter, flag: false, predicate }
    }
}

impl<I: Iterator, P: FnMut(&I::Item) -> bool> Iterator for SkipWhile<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        while let Some(x) = self.iter.next() {
            if self.flag || !(self.predicate)(&x) {
                self.flag = true;
                return Some(x);
            }
        }
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

#[derive(Clone)]
pub struct TakeWhile<I, P> {
    iter: I,
    flag: bool,
    predicate: P,
}

impl<I, P> TakeWhile<I, P> {
    pub(crate) fn new(iter: I, predicate: P) -> TakeWhile<I, P> {
        TakeWhile { iter, flag: false, predicate }
    }
}

impl<I: Iterator, P: FnMut(&I::Item) -> bool> Iterator for TakeWhile<I, P> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        if self.flag {
            return None;
        }
        let x = self.iter.next()?;
        if (self.predicate)(&x) {
            Some(x)
        } else {
            self.flag = true;
            None
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.flag {
            (0, Some(0))
        } else {
            let (_, upper) = self.iter.size_hint();
            (0, upper)
        }
    }
}

#[derive(Clone)]
pub struct MapWhile<I, P> {
    iter: I,
    predicate: P,
}

impl<I, P> MapWhile<I, P> {
    pub(crate) fn new(iter: I, predicate: P) -> MapWhile<I, P> {
        MapWhile { iter, predicate }
    }
}

impl<B, I: Iterator, P: FnMut(I::Item) -> Option<B>> Iterator for MapWhile<I, P> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        let x = self.iter.next()?;
        (self.predicate)(x)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

// ---------------------------------------------------------------- Scan

#[derive(Clone)]
pub struct Scan<I, St, F> {
    iter: I,
    f: F,
    state: St,
}

impl<I, St, F> Scan<I, St, F> {
    pub(crate) fn new(iter: I, state: St, f: F) -> Scan<I, St, F> {
        Scan { iter, f, state }
    }
}

impl<B, I: Iterator, St, F: FnMut(&mut St, I::Item) -> Option<B>> Iterator for Scan<I, St, F> {
    type Item = B;
    fn next(&mut self) -> Option<B> {
        let a = self.iter.next()?;
        (self.f)(&mut self.state, a)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

// ---------------------------------------------------------------- FlatMap / Flatten

pub struct FlatMap<I, U: IntoIterator, F> {
    iter: Fuse<Map<I, F>>,
    front: Option<U::IntoIter>,
    back: Option<U::IntoIter>,
}

impl<I: Iterator, U: IntoIterator, F: FnMut(I::Item) -> U> FlatMap<I, U, F> {
    pub(crate) fn new(iter: I, f: F) -> FlatMap<I, U, F> {
        FlatMap { iter: Fuse::new(Map::new(iter, f)), front: None, back: None }
    }
}

impl<I: Clone, U: IntoIterator, F: Clone> Clone for FlatMap<I, U, F>
where
    U::IntoIter: Clone,
{
    fn clone(&self) -> FlatMap<I, U, F> {
        FlatMap { iter: self.iter.clone(), front: self.front.clone(), back: self.back.clone() }
    }
}

impl<I: Iterator, U: IntoIterator, F: FnMut(I::Item) -> U> Iterator for FlatMap<I, U, F> {
    type Item = U::Item;
    fn next(&mut self) -> Option<U::Item> {
        loop {
            if let Some(inner) = &mut self.front {
                match inner.next() {
                    None => self.front = None,
                    item => return item,
                }
            }
            match self.iter.next() {
                Some(next) => self.front = Some(next.into_iter()),
                None => {
                    return match &mut self.back {
                        Some(b) => {
                            let r = b.next();
                            if r.is_none() {
                                self.back = None;
                            }
                            r
                        }
                        None => None,
                    }
                }
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (flo, fhi) = match &self.front {
            Some(f) => f.size_hint(),
            None => (0, Some(0)),
        };
        let (blo, bhi) = match &self.back {
            Some(b) => b.size_hint(),
            None => (0, Some(0)),
        };
        let lo = flo.saturating_add(blo);
        match (self.iter.size_hint(), fhi, bhi) {
            ((0, Some(0)), Some(a), Some(b)) => (lo, a.checked_add(b)),
            _ => (lo, None),
        }
    }
}

impl<I: DoubleEndedIterator, U: IntoIterator, F: FnMut(I::Item) -> U> DoubleEndedIterator for FlatMap<I, U, F>
where
    U::IntoIter: DoubleEndedIterator,
{
    fn next_back(&mut self) -> Option<U::Item> {
        loop {
            if let Some(inner) = &mut self.back {
                match inner.next_back() {
                    None => self.back = None,
                    item => return item,
                }
            }
            match self.iter.next_back() {
                Some(next) => self.back = Some(next.into_iter()),
                None => {
                    return match &mut self.front {
                        Some(f) => {
                            let r = f.next_back();
                            if r.is_none() {
                                self.front = None;
                            }
                            r
                        }
                        None => None,
                    }
                }
            }
        }
    }
}

impl<I: Iterator, U: IntoIterator, F: FnMut(I::Item) -> U> FusedIterator for FlatMap<I, U, F> {}

pub struct Flatten<I: Iterator>
where
    I::Item: IntoIterator,
{
    iter: Fuse<I>,
    front: Option<<I::Item as IntoIterator>::IntoIter>,
    back: Option<<I::Item as IntoIterator>::IntoIter>,
}

impl<I: Iterator> Flatten<I>
where
    I::Item: IntoIterator,
{
    pub(crate) fn new(iter: I) -> Flatten<I> {
        Flatten { iter: Fuse::new(iter), front: None, back: None }
    }
}

impl<I: Iterator + Clone> Clone for Flatten<I>
where
    I::Item: IntoIterator,
    <I::Item as IntoIterator>::IntoIter: Clone,
{
    fn clone(&self) -> Flatten<I> {
        Flatten { iter: self.iter.clone(), front: self.front.clone(), back: self.back.clone() }
    }
}

impl<I: Iterator> Iterator for Flatten<I>
where
    I::Item: IntoIterator,
{
    type Item = <I::Item as IntoIterator>::Item;
    fn next(&mut self) -> Option<<I::Item as IntoIterator>::Item> {
        loop {
            if let Some(inner) = &mut self.front {
                match inner.next() {
                    None => self.front = None,
                    item => return item,
                }
            }
            match self.iter.next() {
                Some(next) => self.front = Some(next.into_iter()),
                None => {
                    return match &mut self.back {
                        Some(b) => {
                            let r = b.next();
                            if r.is_none() {
                                self.back = None;
                            }
                            r
                        }
                        None => None,
                    }
                }
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (flo, fhi) = match &self.front {
            Some(f) => f.size_hint(),
            None => (0, Some(0)),
        };
        let (blo, bhi) = match &self.back {
            Some(b) => b.size_hint(),
            None => (0, Some(0)),
        };
        let lo = flo.saturating_add(blo);
        match (self.iter.size_hint(), fhi, bhi) {
            ((0, Some(0)), Some(a), Some(b)) => (lo, a.checked_add(b)),
            _ => (lo, None),
        }
    }
}

impl<I: DoubleEndedIterator> DoubleEndedIterator for Flatten<I>
where
    I::Item: IntoIterator,
    <I::Item as IntoIterator>::IntoIter: DoubleEndedIterator,
{
    fn next_back(&mut self) -> Option<<I::Item as IntoIterator>::Item> {
        loop {
            if let Some(inner) = &mut self.back {
                match inner.next_back() {
                    None => self.back = None,
                    item => return item,
                }
            }
            match self.iter.next_back() {
                Some(next) => self.back = Some(next.into_iter()),
                None => {
                    return match &mut self.front {
                        Some(f) => {
                            let r = f.next_back();
                            if r.is_none() {
                                self.front = None;
                            }
                            r
                        }
                        None => None,
                    }
                }
            }
        }
    }
}

impl<I: Iterator> FusedIterator for Flatten<I> where I::Item: IntoIterator {}

// ---------------------------------------------------------------- Fuse / Inspect / Cycle

#[derive(Clone, Debug)]
pub struct Fuse<I> {
    iter: Option<I>,
}

impl<I> Fuse<I> {
    pub(crate) fn new(iter: I) -> Fuse<I> {
        Fuse { iter: Some(iter) }
    }
}

impl<I: Iterator> Iterator for Fuse<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        match &mut self.iter {
            Some(it) => match it.next() {
                None => {
                    self.iter = None;
                    None
                }
                item => item,
            },
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.iter {
            Some(it) => it.size_hint(),
            None => (0, Some(0)),
        }
    }
}

impl<I: DoubleEndedIterator> DoubleEndedIterator for Fuse<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        match &mut self.iter {
            Some(it) => match it.next_back() {
                None => {
                    self.iter = None;
                    None
                }
                item => item,
            },
            None => None,
        }
    }
}

impl<I: ExactSizeIterator> ExactSizeIterator for Fuse<I> {}
impl<I: Iterator> FusedIterator for Fuse<I> {}

#[derive(Clone)]
pub struct Inspect<I, F> {
    iter: I,
    f: F,
}

impl<I, F> Inspect<I, F> {
    pub(crate) fn new(iter: I, f: F) -> Inspect<I, F> {
        Inspect { iter, f }
    }
}

impl<I: Iterator, F: FnMut(&I::Item)> Iterator for Inspect<I, F> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        let x = self.iter.next()?;
        (self.f)(&x);
        Some(x)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<I: DoubleEndedIterator, F: FnMut(&I::Item)> DoubleEndedIterator for Inspect<I, F> {
    fn next_back(&mut self) -> Option<I::Item> {
        let x = self.iter.next_back()?;
        (self.f)(&x);
        Some(x)
    }
}

#[derive(Clone, Debug)]
pub struct Cycle<I> {
    orig: I,
    iter: I,
}

impl<I: Clone> Cycle<I> {
    pub(crate) fn new(iter: I) -> Cycle<I> {
        Cycle { orig: iter.clone(), iter }
    }
}

impl<I: Clone + Iterator> Iterator for Cycle<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        match self.iter.next() {
            None => {
                self.iter = self.orig.clone();
                self.iter.next()
            }
            y => y,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self.orig.size_hint() {
            (0, Some(0)) => (0, Some(0)),
            (0, _) => (0, None),
            _ => (usize::MAX, None),
        }
    }
}
