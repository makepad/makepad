//! Iterator sources: once, repeat, empty, from_fn, successors, zip.

use super::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, IntoIterator, Iterator, Zip};
use crate::marker::PhantomData;

pub struct Empty<T> {
    _m: PhantomData<T>,
}

pub const fn empty<T>() -> Empty<T> {
    Empty { _m: PhantomData }
}

impl<T> Iterator for Empty<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        None
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(0))
    }
}
impl<T> DoubleEndedIterator for Empty<T> {
    fn next_back(&mut self) -> Option<T> {
        None
    }
}
impl<T> ExactSizeIterator for Empty<T> {}
impl<T> FusedIterator for Empty<T> {}
impl<T> Clone for Empty<T> {
    fn clone(&self) -> Empty<T> {
        Empty { _m: PhantomData }
    }
}
impl<T> Default for Empty<T> {
    fn default() -> Empty<T> {
        Empty { _m: PhantomData }
    }
}

#[derive(Clone, Debug)]
pub struct Once<T> {
    inner: Option<T>,
}

pub fn once<T>(value: T) -> Once<T> {
    Once { inner: Some(value) }
}

impl<T> Iterator for Once<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.inner.take()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.inner.is_some() { 1 } else { 0 };
        (n, Some(n))
    }
}
impl<T> DoubleEndedIterator for Once<T> {
    fn next_back(&mut self) -> Option<T> {
        self.inner.take()
    }
}
impl<T> ExactSizeIterator for Once<T> {}
impl<T> FusedIterator for Once<T> {}

pub struct OnceWith<F> {
    make: Option<F>,
}

pub fn once_with<A, F: FnOnce() -> A>(make: F) -> OnceWith<F> {
    OnceWith { make: Some(make) }
}

impl<A, F: FnOnce() -> A> Iterator for OnceWith<F> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        match self.make.take() {
            Some(f) => Some(f()),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.make.is_some() { 1 } else { 0 };
        (n, Some(n))
    }
}

#[derive(Clone, Debug)]
pub struct Repeat<A> {
    element: A,
}

pub fn repeat<T: Clone>(elt: T) -> Repeat<T> {
    Repeat { element: elt }
}

impl<A: Clone> Iterator for Repeat<A> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        Some(self.element.clone())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
    fn nth(&mut self, _n: usize) -> Option<A> {
        Some(self.element.clone())
    }
}
impl<A: Clone> DoubleEndedIterator for Repeat<A> {
    fn next_back(&mut self) -> Option<A> {
        Some(self.element.clone())
    }
}
impl<A: Clone> FusedIterator for Repeat<A> {}

#[derive(Clone, Debug)]
pub struct RepeatN<A> {
    element: Option<A>,
    count: usize,
}

pub fn repeat_n<T: Clone>(element: T, count: usize) -> RepeatN<T> {
    RepeatN { element: if count == 0 { None } else { Some(element) }, count }
}

impl<A: Clone> Iterator for RepeatN<A> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        if self.count == 0 {
            return None;
        }
        self.count -= 1;
        if self.count == 0 {
            self.element.take()
        } else {
            self.element.clone()
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}
impl<A: Clone> ExactSizeIterator for RepeatN<A> {}

pub struct RepeatWith<F> {
    f: F,
}

pub fn repeat_with<A, F: FnMut() -> A>(repeater: F) -> RepeatWith<F> {
    RepeatWith { f: repeater }
}

impl<A, F: FnMut() -> A> Iterator for RepeatWith<F> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        Some((self.f)())
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (usize::MAX, None)
    }
}

#[derive(Clone)]
pub struct FromFn<F> {
    f: F,
}

pub fn from_fn<T, F: FnMut() -> Option<T>>(f: F) -> FromFn<F> {
    FromFn { f }
}

impl<T, F: FnMut() -> Option<T>> Iterator for FromFn<F> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        (self.f)()
    }
}

#[derive(Clone)]
pub struct Successors<T, F> {
    next: Option<T>,
    succ: F,
}

pub fn successors<T, F: FnMut(&T) -> Option<T>>(first: Option<T>, succ: F) -> Successors<T, F> {
    Successors { next: first, succ }
}

impl<T, F: FnMut(&T) -> Option<T>> Iterator for Successors<T, F> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        let item = self.next.take()?;
        self.next = (self.succ)(&item);
        Some(item)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.next.is_some() {
            (1, None)
        } else {
            (0, Some(0))
        }
    }
}
impl<T, F: FnMut(&T) -> Option<T>> FusedIterator for Successors<T, F> {}

pub fn zip<A: IntoIterator, B: IntoIterator>(a: A, b: B) -> Zip<A::IntoIter, B::IntoIter> {
    Zip::new(a.into_iter(), b.into_iter())
}
