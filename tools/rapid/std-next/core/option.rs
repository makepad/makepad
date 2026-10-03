//! Option.

use crate::ops::{Deref, DerefMut};
use crate::result::Result::{self, Err, Ok};

#[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Debug, Hash)]
pub enum Option<T> {
    None,
    Some(T),
}

pub use Option::{None, Some};

impl<T> Option<T> {
    pub const fn is_some(&self) -> bool {
        matches!(*self, Some(_))
    }
    pub fn is_some_and<F: FnOnce(T) -> bool>(self, f: F) -> bool {
        match self {
            Some(x) => f(x),
            None => false,
        }
    }
    pub const fn is_none(&self) -> bool {
        !self.is_some()
    }
    pub fn is_none_or<F: FnOnce(T) -> bool>(self, f: F) -> bool {
        match self {
            Some(x) => f(x),
            None => true,
        }
    }
    pub const fn as_ref(&self) -> Option<&T> {
        match *self {
            Some(ref x) => Some(x),
            None => None,
        }
    }
    pub fn as_mut(&mut self) -> Option<&mut T> {
        match *self {
            Some(ref mut x) => Some(x),
            None => None,
        }
    }
    pub fn as_slice(&self) -> &[T] {
        match self {
            Some(x) => crate::slice::from_ref(x),
            None => &[],
        }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        match self {
            Some(x) => crate::slice::from_mut(x),
            None => &mut [],
        }
    }
    #[track_caller]
    pub fn expect(self, msg: &str) -> T {
        match self {
            Some(x) => x,
            None => crate::panicking::panic_display(&msg),
        }
    }
    #[track_caller]
    pub fn unwrap(self) -> T {
        match self {
            Some(x) => x,
            None => crate::panicking::panic_str("called `Option::unwrap()` on a `None` value"),
        }
    }
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Some(x) => x,
            None => default,
        }
    }
    pub fn unwrap_or_else<F: FnOnce() -> T>(self, f: F) -> T {
        match self {
            Some(x) => x,
            None => f(),
        }
    }
    pub fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        match self {
            Some(x) => x,
            None => T::default(),
        }
    }
    #[track_caller]
    pub unsafe fn unwrap_unchecked(self) -> T {
        match self {
            Some(x) => x,
            None => crate::hint::unreachable_unchecked(),
        }
    }
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Option<U> {
        match self {
            Some(x) => Some(f(x)),
            None => None,
        }
    }
    pub fn inspect<F: FnOnce(&T)>(self, f: F) -> Option<T> {
        if let Some(ref x) = self {
            f(x);
        }
        self
    }
    pub fn map_or<U, F: FnOnce(T) -> U>(self, default: U, f: F) -> U {
        match self {
            Some(t) => f(t),
            None => default,
        }
    }
    pub fn map_or_else<U, D: FnOnce() -> U, F: FnOnce(T) -> U>(self, default: D, f: F) -> U {
        match self {
            Some(t) => f(t),
            None => default(),
        }
    }
    pub fn ok_or<E>(self, err: E) -> Result<T, E> {
        match self {
            Some(v) => Ok(v),
            None => Err(err),
        }
    }
    pub fn ok_or_else<E, F: FnOnce() -> E>(self, err: F) -> Result<T, E> {
        match self {
            Some(v) => Ok(v),
            None => Err(err()),
        }
    }
    pub fn as_deref(&self) -> Option<&T::Target>
    where
        T: Deref,
    {
        match self.as_ref() {
            Some(t) => Some(t.deref()),
            None => None,
        }
    }
    pub fn as_deref_mut(&mut self) -> Option<&mut T::Target>
    where
        T: DerefMut,
    {
        match self.as_mut() {
            Some(t) => Some(t.deref_mut()),
            None => None,
        }
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { inner: self.as_ref() }
    }
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut { inner: self.as_mut() }
    }
    pub fn and<U>(self, optb: Option<U>) -> Option<U> {
        match self {
            Some(_) => optb,
            None => None,
        }
    }
    pub fn and_then<U, F: FnOnce(T) -> Option<U>>(self, f: F) -> Option<U> {
        match self {
            Some(x) => f(x),
            None => None,
        }
    }
    pub fn filter<P: FnOnce(&T) -> bool>(self, predicate: P) -> Option<T> {
        if let Some(x) = self {
            if predicate(&x) {
                return Some(x);
            }
        }
        None
    }
    pub fn or(self, optb: Option<T>) -> Option<T> {
        match self {
            x @ Some(_) => x,
            None => optb,
        }
    }
    pub fn or_else<F: FnOnce() -> Option<T>>(self, f: F) -> Option<T> {
        match self {
            x @ Some(_) => x,
            None => f(),
        }
    }
    pub fn xor(self, optb: Option<T>) -> Option<T> {
        match (self, optb) {
            (a @ Some(_), None) => a,
            (None, b @ Some(_)) => b,
            _ => None,
        }
    }
    pub fn insert(&mut self, value: T) -> &mut T {
        *self = Some(value);
        match self {
            Some(v) => v,
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    pub fn get_or_insert(&mut self, value: T) -> &mut T {
        if let None = *self {
            *self = Some(value);
        }
        match self {
            Some(v) => v,
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    pub fn get_or_insert_default(&mut self) -> &mut T
    where
        T: Default,
    {
        if let None = *self {
            *self = Some(T::default());
        }
        match self {
            Some(v) => v,
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    pub fn get_or_insert_with<F: FnOnce() -> T>(&mut self, f: F) -> &mut T {
        if let None = *self {
            *self = Some(f());
        }
        match self {
            Some(v) => v,
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    pub fn take(&mut self) -> Option<T> {
        crate::mem::replace(self, None)
    }
    pub fn take_if<P: FnOnce(&mut T) -> bool>(&mut self, predicate: P) -> Option<T> {
        let hit = match self.as_mut() {
            Some(v) => predicate(v),
            None => false,
        };
        if hit {
            self.take()
        } else {
            None
        }
    }
    pub fn replace(&mut self, value: T) -> Option<T> {
        crate::mem::replace(self, Some(value))
    }
    pub fn zip<U>(self, other: Option<U>) -> Option<(T, U)> {
        match (self, other) {
            (Some(a), Some(b)) => Some((a, b)),
            _ => None,
        }
    }
}

impl<T, U> Option<(T, U)> {
    pub fn unzip(self) -> (Option<T>, Option<U>) {
        match self {
            Some((a, b)) => (Some(a), Some(b)),
            None => (None, None),
        }
    }
}

impl<'a, T: Copy> Option<&'a T> {
    pub fn copied(self) -> Option<T> {
        match self {
            Some(&v) => Some(v),
            None => None,
        }
    }
}

impl<'a, T: Clone> Option<&'a T> {
    pub fn cloned(self) -> Option<T> {
        match self {
            Some(t) => Some(t.clone()),
            None => None,
        }
    }
}

impl<'a, T: Copy> Option<&'a mut T> {
    pub fn copied(self) -> Option<T> {
        match self {
            Some(&mut t) => Some(t),
            None => None,
        }
    }
}

impl<'a, T: Clone> Option<&'a mut T> {
    pub fn cloned(self) -> Option<T> {
        match self {
            Some(t) => Some(t.clone()),
            None => None,
        }
    }
}

impl<T, E> Option<Result<T, E>> {
    pub fn transpose(self) -> Result<Option<T>, E> {
        match self {
            Some(Ok(x)) => Ok(Some(x)),
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }
}

impl<T> Option<Option<T>> {
    pub fn flatten(self) -> Option<T> {
        match self {
            Some(inner) => inner,
            None => None,
        }
    }
}

impl<T> Default for Option<T> {
    fn default() -> Option<T> {
        None
    }
}

impl<T> From<T> for Option<T> {
    fn from(val: T) -> Option<T> {
        Some(val)
    }
}

impl<'a, T> From<&'a Option<T>> for Option<&'a T> {
    fn from(o: &'a Option<T>) -> Option<&'a T> {
        o.as_ref()
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, T> {
    inner: Option<&'a T>,
}

impl<'a, T> Iter<'a, T> {
    pub(crate) fn from_option(inner: Option<&'a T>) -> Iter<'a, T> {
        Iter { inner }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        self.inner.take()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.inner.is_some() { 1 } else { 0 };
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        self.inner.take()
    }
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { inner: self.inner }
    }
}

pub struct IterMut<'a, T> {
    inner: Option<&'a mut T>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        self.inner.take()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.inner.is_some() { 1 } else { 0 };
        (n, Some(n))
    }
}

pub struct IntoIter<T> {
    inner: Option<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.inner.take()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.inner.is_some() { 1 } else { 0 };
        (n, Some(n))
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.inner.take()
    }
}

impl<T> IntoIterator for Option<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self }
    }
}

impl<'a, T> IntoIterator for &'a Option<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Option<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

/// `iter.collect::<Option<C>>()`: stops at the first None.
impl<A, V: FromIterator<A>> FromIterator<Option<A>> for Option<V> {
    fn from_iter<I: IntoIterator<Item = Option<A>>>(iter: I) -> Option<V> {
        let mut failed = false;
        let v: V = {
            let shunt = OptionShunt { iter: iter.into_iter(), failed: &mut failed };
            V::from_iter(shunt)
        };
        if failed {
            None
        } else {
            Some(v)
        }
    }
}

struct OptionShunt<'a, I> {
    iter: I,
    failed: &'a mut bool,
}

impl<'a, A, I: Iterator<Item = Option<A>>> Iterator for OptionShunt<'a, I> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        match self.iter.next() {
            Some(Some(v)) => Some(v),
            Some(None) => {
                *self.failed = true;
                None
            }
            None => None,
        }
    }
}
