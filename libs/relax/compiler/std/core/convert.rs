//! Conversion traits. Numeric From/TryFrom impls are generated (num/prim_traits.rs).
//! The blanket impls here are the fixed set the dialect allows in core (coord.md S4e).

use crate::result::Result::{self, Ok};

#[cfg_attr(not(relax_check), lang = "From")]
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

pub trait AsRef<T: crate::marker::PointeeSized>: crate::marker::PointeeSized {
    fn as_ref(&self) -> &T;
}

pub trait AsMut<T: crate::marker::PointeeSized>: crate::marker::PointeeSized {
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

impl<'a, T: crate::marker::PointeeSized, U: crate::marker::PointeeSized> AsRef<U> for &'a T
where
    T: AsRef<U>,
{
    fn as_ref(&self) -> &U {
        (**self).as_ref()
    }
}

impl<'a, T: crate::marker::PointeeSized, U: crate::marker::PointeeSized> AsRef<U> for &'a mut T
where
    T: AsRef<U>,
{
    fn as_ref(&self) -> &U {
        (**self).as_ref()
    }
}

impl<'a, T: crate::marker::PointeeSized, U: crate::marker::PointeeSized> AsMut<U> for &'a mut T
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

impl crate::error::Error for Infallible {}

pub const fn identity<T>(x: T) -> T {
    x
}

pub mod num {
    pub trait FloatToInt<Int>: Sized {
        unsafe fn to_int_unchecked(self) -> Int;
    }
    impl FloatToInt<u8> for f32 { unsafe fn to_int_unchecked(self) -> u8 { self as u8 } }
    impl FloatToInt<u16> for f32 { unsafe fn to_int_unchecked(self) -> u16 { self as u16 } }
    impl FloatToInt<u32> for f32 { unsafe fn to_int_unchecked(self) -> u32 { self as u32 } }
    impl FloatToInt<u64> for f32 { unsafe fn to_int_unchecked(self) -> u64 { self as u64 } }
    impl FloatToInt<u128> for f32 { unsafe fn to_int_unchecked(self) -> u128 { self as u128 } }
    impl FloatToInt<usize> for f32 { unsafe fn to_int_unchecked(self) -> usize { self as usize } }
    impl FloatToInt<i8> for f32 { unsafe fn to_int_unchecked(self) -> i8 { self as i8 } }
    impl FloatToInt<i16> for f32 { unsafe fn to_int_unchecked(self) -> i16 { self as i16 } }
    impl FloatToInt<i32> for f32 { unsafe fn to_int_unchecked(self) -> i32 { self as i32 } }
    impl FloatToInt<i64> for f32 { unsafe fn to_int_unchecked(self) -> i64 { self as i64 } }
    impl FloatToInt<i128> for f32 { unsafe fn to_int_unchecked(self) -> i128 { self as i128 } }
    impl FloatToInt<isize> for f32 { unsafe fn to_int_unchecked(self) -> isize { self as isize } }
    impl FloatToInt<u8> for f64 { unsafe fn to_int_unchecked(self) -> u8 { self as u8 } }
    impl FloatToInt<u16> for f64 { unsafe fn to_int_unchecked(self) -> u16 { self as u16 } }
    impl FloatToInt<u32> for f64 { unsafe fn to_int_unchecked(self) -> u32 { self as u32 } }
    impl FloatToInt<u64> for f64 { unsafe fn to_int_unchecked(self) -> u64 { self as u64 } }
    impl FloatToInt<u128> for f64 { unsafe fn to_int_unchecked(self) -> u128 { self as u128 } }
    impl FloatToInt<usize> for f64 { unsafe fn to_int_unchecked(self) -> usize { self as usize } }
    impl FloatToInt<i8> for f64 { unsafe fn to_int_unchecked(self) -> i8 { self as i8 } }
    impl FloatToInt<i16> for f64 { unsafe fn to_int_unchecked(self) -> i16 { self as i16 } }
    impl FloatToInt<i32> for f64 { unsafe fn to_int_unchecked(self) -> i32 { self as i32 } }
    impl FloatToInt<i64> for f64 { unsafe fn to_int_unchecked(self) -> i64 { self as i64 } }
    impl FloatToInt<i128> for f64 { unsafe fn to_int_unchecked(self) -> i128 { self as i128 } }
    impl FloatToInt<isize> for f64 { unsafe fn to_int_unchecked(self) -> isize { self as isize } }
}

pub use self::num::FloatToInt;
