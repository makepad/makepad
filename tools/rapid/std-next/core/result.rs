//! Result.

use crate::fmt;
use crate::ops::{Deref, DerefMut};
use crate::option::Option::{self, None, Some};

#[derive(Copy, Clone, PartialEq, PartialOrd, Eq, Ord, Debug, Hash)]
#[must_use]
pub enum Result<T, E> {
    Ok(T),
    Err(E),
}

pub use Result::{Err, Ok};

impl<T, E> Result<T, E> {
    pub const fn is_ok(&self) -> bool {
        matches!(*self, Ok(_))
    }
    pub fn is_ok_and<F: FnOnce(T) -> bool>(self, f: F) -> bool {
        match self {
            Ok(x) => f(x),
            Err(_) => false,
        }
    }
    pub const fn is_err(&self) -> bool {
        !self.is_ok()
    }
    pub fn is_err_and<F: FnOnce(E) -> bool>(self, f: F) -> bool {
        match self {
            Ok(_) => false,
            Err(e) => f(e),
        }
    }
    pub fn ok(self) -> Option<T> {
        match self {
            Ok(x) => Some(x),
            Err(_) => None,
        }
    }
    pub fn err(self) -> Option<E> {
        match self {
            Ok(_) => None,
            Err(e) => Some(e),
        }
    }
    pub const fn as_ref(&self) -> Result<&T, &E> {
        match *self {
            Ok(ref x) => Ok(x),
            Err(ref x) => Err(x),
        }
    }
    pub fn as_mut(&mut self) -> Result<&mut T, &mut E> {
        match *self {
            Ok(ref mut x) => Ok(x),
            Err(ref mut x) => Err(x),
        }
    }
    pub fn as_deref(&self) -> Result<&T::Target, &E>
    where
        T: Deref,
    {
        match self {
            Ok(t) => Ok(t.deref()),
            Err(e) => Err(e),
        }
    }
    pub fn as_deref_mut(&mut self) -> Result<&mut T::Target, &mut E>
    where
        T: DerefMut,
    {
        match self {
            Ok(t) => Ok(t.deref_mut()),
            Err(e) => Err(e),
        }
    }
    pub fn map<U, F: FnOnce(T) -> U>(self, op: F) -> Result<U, E> {
        match self {
            Ok(t) => Ok(op(t)),
            Err(e) => Err(e),
        }
    }
    pub fn map_or<U, F: FnOnce(T) -> U>(self, default: U, f: F) -> U {
        match self {
            Ok(t) => f(t),
            Err(_) => default,
        }
    }
    pub fn map_or_else<U, D: FnOnce(E) -> U, F: FnOnce(T) -> U>(self, default: D, f: F) -> U {
        match self {
            Ok(t) => f(t),
            Err(e) => default(e),
        }
    }
    pub fn map_err<F2, O: FnOnce(E) -> F2>(self, op: O) -> Result<T, F2> {
        match self {
            Ok(t) => Ok(t),
            Err(e) => Err(op(e)),
        }
    }
    pub fn inspect<F: FnOnce(&T)>(self, f: F) -> Result<T, E> {
        if let Ok(ref t) = self {
            f(t);
        }
        self
    }
    pub fn inspect_err<F: FnOnce(&E)>(self, f: F) -> Result<T, E> {
        if let Err(ref e) = self {
            f(e);
        }
        self
    }
    pub fn iter(&self) -> crate::option::Iter<'_, T> {
        crate::option::Iter::from_option(self.as_ref().ok())
    }
    #[track_caller]
    pub fn expect(self, msg: &str) -> T
    where
        E: fmt::Debug,
    {
        match self {
            Ok(t) => t,
            Err(e) => unwrap_failed(msg, &e),
        }
    }
    #[track_caller]
    pub fn unwrap(self) -> T
    where
        E: fmt::Debug,
    {
        match self {
            Ok(t) => t,
            Err(e) => unwrap_failed("called `Result::unwrap()` on an `Err` value", &e),
        }
    }
    pub fn unwrap_or_default(self) -> T
    where
        T: Default,
    {
        match self {
            Ok(x) => x,
            Err(_) => T::default(),
        }
    }
    #[track_caller]
    pub fn expect_err(self, msg: &str) -> E
    where
        T: fmt::Debug,
    {
        match self {
            Ok(t) => unwrap_failed(msg, &t),
            Err(e) => e,
        }
    }
    #[track_caller]
    pub fn unwrap_err(self) -> E
    where
        T: fmt::Debug,
    {
        match self {
            Ok(t) => unwrap_failed("called `Result::unwrap_err()` on an `Ok` value", &t),
            Err(e) => e,
        }
    }
    pub fn and<U>(self, res: Result<U, E>) -> Result<U, E> {
        match self {
            Ok(_) => res,
            Err(e) => Err(e),
        }
    }
    pub fn and_then<U, F: FnOnce(T) -> Result<U, E>>(self, op: F) -> Result<U, E> {
        match self {
            Ok(t) => op(t),
            Err(e) => Err(e),
        }
    }
    pub fn or<F2>(self, res: Result<T, F2>) -> Result<T, F2> {
        match self {
            Ok(v) => Ok(v),
            Err(_) => res,
        }
    }
    pub fn or_else<F2, O: FnOnce(E) -> Result<T, F2>>(self, op: O) -> Result<T, F2> {
        match self {
            Ok(t) => Ok(t),
            Err(e) => op(e),
        }
    }
    pub fn unwrap_or(self, default: T) -> T {
        match self {
            Ok(t) => t,
            Err(_) => default,
        }
    }
    pub fn unwrap_or_else<F: FnOnce(E) -> T>(self, op: F) -> T {
        match self {
            Ok(t) => t,
            Err(e) => op(e),
        }
    }
    #[track_caller]
    pub unsafe fn unwrap_unchecked(self) -> T {
        match self {
            Ok(t) => t,
            Err(_) => crate::hint::unreachable_unchecked(),
        }
    }
}

impl<T, E> Result<Option<T>, E> {
    pub fn transpose(self) -> Option<Result<T, E>> {
        match self {
            Ok(Some(x)) => Some(Ok(x)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

impl<'a, T: Copy, E> Result<&'a T, E> {
    pub fn copied(self) -> Result<T, E> {
        match self {
            Ok(&v) => Ok(v),
            Err(e) => Err(e),
        }
    }
}

impl<'a, T: Clone, E> Result<&'a T, E> {
    pub fn cloned(self) -> Result<T, E> {
        match self {
            Ok(t) => Ok(t.clone()),
            Err(e) => Err(e),
        }
    }
}

#[track_caller]
fn unwrap_failed(msg: &str, error: &dyn fmt::Debug) -> ! {
    crate::panicking::panic_fmt(format_args!("{msg}: {error:?}"))
}

pub struct IntoIter<T> {
    inner: Option<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.inner.take()
    }
}

impl<T, E> IntoIterator for Result<T, E> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self.ok() }
    }
}

/// `iter.collect::<Result<C, E>>()`: stops at the first error.
impl<A, E, V: FromIterator<A>> FromIterator<Result<A, E>> for Result<V, E> {
    fn from_iter<I: IntoIterator<Item = Result<A, E>>>(iter: I) -> Result<V, E> {
        let mut error: Option<E> = None;
        let v: V = {
            let shunt = ResultShunt { iter: iter.into_iter(), error: &mut error };
            V::from_iter(shunt)
        };
        match error {
            Some(e) => Err(e),
            None => Ok(v),
        }
    }
}

struct ResultShunt<'a, I, E> {
    iter: I,
    error: &'a mut Option<E>,
}

impl<'a, A, E, I: Iterator<Item = Result<A, E>>> Iterator for ResultShunt<'a, I, E> {
    type Item = A;
    fn next(&mut self) -> Option<A> {
        match self.iter.next() {
            Some(Ok(v)) => Some(v),
            Some(Err(e)) => {
                *self.error = Some(e);
                None
            }
            None => None,
        }
    }
}

impl<T, U, E> crate::iter::Sum<Result<U, E>> for Result<T, E>
where
    T: crate::iter::Sum<U>,
{
    fn sum<I: Iterator<Item = Result<U, E>>>(iter: I) -> Result<T, E> {
        let mut error: Option<E> = None;
        let v: T = {
            let shunt = ResultShunt { iter, error: &mut error };
            T::sum(shunt)
        };
        match error {
            Some(e) => Err(e),
            None => Ok(v),
        }
    }
}
