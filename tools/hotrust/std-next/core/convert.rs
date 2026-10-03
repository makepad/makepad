//! Conversion traits. Numeric From/TryFrom impls are generated (num/prim_traits.rs).
//! The blanket impls here are the fixed set the dialect allows in core (coord.md S4e).

use crate::result::Result::{self, Ok};

pub trait From<T>: Sized {
    fn from(value: T) -> Self;
}

pub trait Into<T>: Sized {
    fn into(self) -> T;
}

pub trait TryFrom<T>: Sized {
    type Error;
    fn try_from(value: T) -> Result<Self, Self::Error>;
}

pub trait TryInto<T>: Sized {
    type Error;
    fn try_into(self) -> Result<T, Self::Error>;
}

pub trait AsRef<T: ?Sized> {
    fn as_ref(&self) -> &T;
}

pub trait AsMut<T: ?Sized> {
    fn as_mut(&mut self) -> &mut T;
}

impl<T> From<T> for T {
    fn from(t: T) -> T {
        t
    }
}

impl<T, U: From<T>> Into<U> for T {
    fn into(self) -> U {
        U::from(self)
    }
}

impl<T, U: Into<T>> TryFrom<U> for T {
    type Error = Infallible;
    fn try_from(value: U) -> Result<T, Infallible> {
        Ok(U::into(value))
    }
}

impl<T, U: TryFrom<T>> TryInto<U> for T {
    type Error = U::Error;
    fn try_into(self) -> Result<U, U::Error> {
        U::try_from(self)
    }
}

impl<'a, T: ?Sized, U: ?Sized> AsRef<U> for &'a T
where
    T: AsRef<U>,
{
    fn as_ref(&self) -> &U {
        (**self).as_ref()
    }
}

impl<'a, T: ?Sized, U: ?Sized> AsRef<U> for &'a mut T
where
    T: AsRef<U>,
{
    fn as_ref(&self) -> &U {
        (**self).as_ref()
    }
}

impl<'a, T: ?Sized, U: ?Sized> AsMut<U> for &'a mut T
where
    T: AsMut<U>,
{
    fn as_mut(&mut self) -> &mut U {
        (**self).as_mut()
    }
}

impl<T> AsRef<[T]> for [T] {
    fn as_ref(&self) -> &[T] {
        self
    }
}

impl<T> AsMut<[T]> for [T] {
    fn as_mut(&mut self) -> &mut [T] {
        self
    }
}

impl AsRef<str> for str {
    fn as_ref(&self) -> &str {
        self
    }
}

impl AsRef<[u8]> for str {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// The error type for errors that can never happen.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Infallible {}

impl crate::fmt::Display for Infallible {
    fn fmt(&self, _f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        match *self {}
    }
}

pub const fn identity<T>(x: T) -> T {
    x
}
