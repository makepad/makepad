//! Comparison traits. Primitive impls are generated (num/prim_traits.rs); the std derives
//! (PartialEq, Eq, PartialOrd, Ord) are native in HotRust.

use crate::option::Option::{self, None, Some};

pub trait PartialEq<Rhs: ?Sized = Self> {
    fn eq(&self, other: &Rhs) -> bool;
    fn ne(&self, other: &Rhs) -> bool {
        !self.eq(other)
    }
}

pub trait Eq: PartialEq<Self> {}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(i8)]
pub enum Ordering {
    Less = -1,
    Equal = 0,
    Greater = 1,
}

pub use Ordering::{Equal, Greater, Less};

impl Ordering {
    pub const fn is_eq(self) -> bool {
        matches!(self, Equal)
    }
    pub const fn is_ne(self) -> bool {
        !matches!(self, Equal)
    }
    pub const fn is_lt(self) -> bool {
        matches!(self, Less)
    }
    pub const fn is_gt(self) -> bool {
        matches!(self, Greater)
    }
    pub const fn is_le(self) -> bool {
        !matches!(self, Greater)
    }
    pub const fn is_ge(self) -> bool {
        !matches!(self, Less)
    }
    pub const fn reverse(self) -> Ordering {
        match self {
            Less => Greater,
            Equal => Equal,
            Greater => Less,
        }
    }
    pub const fn then(self, other: Ordering) -> Ordering {
        match self {
            Equal => other,
            _ => self,
        }
    }
    pub fn then_with<F: FnOnce() -> Ordering>(self, f: F) -> Ordering {
        match self {
            Equal => f(),
            _ => self,
        }
    }
}

impl PartialOrd for Ordering {
    fn partial_cmp(&self, other: &Ordering) -> Option<Ordering> {
        (*self as i8).partial_cmp(&(*other as i8))
    }
}

impl Ord for Ordering {
    fn cmp(&self, other: &Ordering) -> Ordering {
        (*self as i8).cmp(&(*other as i8))
    }
}

pub trait PartialOrd<Rhs: ?Sized = Self>: PartialEq<Rhs> {
    fn partial_cmp(&self, other: &Rhs) -> Option<Ordering>;
    fn lt(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Less))
    }
    fn le(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Less | Equal))
    }
    fn gt(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Greater))
    }
    fn ge(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Greater | Equal))
    }
}

pub trait Ord: Eq + PartialOrd<Self> {
    fn cmp(&self, other: &Self) -> Ordering;
    fn max(self, other: Self) -> Self
    where
        Self: Sized,
    {
        if other.cmp(&self) == Less {
            self
        } else {
            other
        }
    }
    fn min(self, other: Self) -> Self
    where
        Self: Sized,
    {
        if other.cmp(&self) == Less {
            other
        } else {
            self
        }
    }
    fn clamp(self, min: Self, max: Self) -> Self
    where
        Self: Sized,
    {
        if min.cmp(&max) == Greater {
            panic!("assertion failed: min <= max");
        }
        if self.cmp(&min) == Less {
            min
        } else if self.cmp(&max) == Greater {
            max
        } else {
            self
        }
    }
}

pub fn min<T: Ord>(a: T, b: T) -> T {
    a.min(b)
}

pub fn max<T: Ord>(a: T, b: T) -> T {
    a.max(b)
}

pub fn min_by<T, F: FnOnce(&T, &T) -> Ordering>(a: T, b: T, compare: F) -> T {
    match compare(&a, &b) {
        Less | Equal => a,
        Greater => b,
    }
}

pub fn max_by<T, F: FnOnce(&T, &T) -> Ordering>(a: T, b: T, compare: F) -> T {
    match compare(&a, &b) {
        Less | Equal => b,
        Greater => a,
    }
}

pub fn min_by_key<T, K: Ord, F: FnMut(&T) -> K>(a: T, b: T, mut f: F) -> T {
    let ka = f(&a);
    let kb = f(&b);
    if kb < ka {
        b
    } else {
        a
    }
}

pub fn max_by_key<T, K: Ord, F: FnMut(&T) -> K>(a: T, b: T, mut f: F) -> T {
    let ka = f(&a);
    let kb = f(&b);
    if kb < ka {
        a
    } else {
        b
    }
}

/// Reverses the ordering of the wrapped value.
#[derive(PartialEq, Eq, Debug, Copy, Clone, Default, Hash)]
pub struct Reverse<T>(pub T);

impl<T: PartialOrd> PartialOrd for Reverse<T> {
    fn partial_cmp(&self, other: &Reverse<T>) -> Option<Ordering> {
        other.0.partial_cmp(&self.0)
    }
}

impl<T: Ord> Ord for Reverse<T> {
    fn cmp(&self, other: &Reverse<T>) -> Ordering {
        other.0.cmp(&self.0)
    }
}

// References compare by value.
impl<'a, 'b, A: ?Sized + PartialEq<B>, B: ?Sized> PartialEq<&'b B> for &'a A {
    fn eq(&self, other: &&'b B) -> bool {
        (**self).eq(*other)
    }
}
impl<'a, 'b, A: ?Sized + PartialEq<B>, B: ?Sized> PartialEq<&'b mut B> for &'a mut A {
    fn eq(&self, other: &&'b mut B) -> bool {
        (**self).eq(*other)
    }
}
impl<'a, A: ?Sized + Eq> Eq for &'a A {}
impl<'a, 'b, A: ?Sized + PartialOrd<B>, B: ?Sized> PartialOrd<&'b B> for &'a A {
    fn partial_cmp(&self, other: &&'b B) -> Option<Ordering> {
        (**self).partial_cmp(*other)
    }
}
impl<'a, A: ?Sized + Ord> Ord for &'a A {
    fn cmp(&self, other: &&'a A) -> Ordering {
        (**self).cmp(*other)
    }
}

impl PartialEq for () {
    fn eq(&self, _o: &()) -> bool {
        true
    }
}
impl Eq for () {}
impl PartialOrd for () {
    fn partial_cmp(&self, _o: &()) -> Option<Ordering> {
        Some(Equal)
    }
}
impl Ord for () {
    fn cmp(&self, _o: &()) -> Ordering {
        Equal
    }
}
