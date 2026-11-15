// ported from library/core/src/cmp.rs @8d1a764
//! Comparison traits. Primitive impls are generated (num/prim_traits.rs); the std derives
//! (PartialEq, Eq, PartialOrd, Ord) are native in Relax.

use crate::option::Option::{self, None, Some};

#[lang = "eq"]
pub trait PartialEq<Rhs: crate::marker::PointeeSized = Self>: crate::marker::PointeeSized {
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialeq_eq"]
    fn eq(&self, other: &Rhs) -> bool;
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialeq_ne"]
    fn ne(&self, other: &Rhs) -> bool {
        !self.eq(other)
    }
}

pub trait Eq: PartialEq<Self> + crate::marker::PointeeSized {
    #[doc(hidden)]
    fn assert_fields_are_eq(&self) {}
    #[doc(hidden)]
    #[deprecated(note = "implementation detail of `#[derive(Eq)]`")]
    #[rustc_diagnostic_item = "assert_receiver_is_total_eq"]
    fn assert_receiver_is_total_eq(&self) {}
}
// :395: field-bound assertions emitted by derive(Eq).
pub struct AssertParamIsEq<T: Eq + crate::marker::PointeeSized> { _field: crate::marker::PhantomData<T> }

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[repr(i8)]
#[lang = "Ordering"]
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
    pub const fn then_with<F: FnOnce() -> Ordering>(self, f: F) -> Ordering {
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

#[lang = "partial_ord"]
pub trait PartialOrd<Rhs: crate::marker::PointeeSized = Self>: PartialEq<Rhs> + crate::marker::PointeeSized {
    #[rustc_diagnostic_item = "cmp_partialord_cmp"]
    fn partial_cmp(&self, other: &Rhs) -> Option<Ordering>;
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialord_lt"]
    fn lt(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Less))
    }
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialord_le"]
    fn le(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Less | Equal))
    }
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialord_gt"]
    fn gt(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Greater))
    }
    #[must_use]
    #[rustc_diagnostic_item = "cmp_partialord_ge"]
    fn ge(&self, other: &Rhs) -> bool {
        matches!(self.partial_cmp(other), Some(Greater | Equal))
    }
}

pub trait Ord: Eq + PartialOrd<Self> + crate::marker::PointeeSized {
    #[rustc_diagnostic_item = "ord_cmp_method"]
    fn cmp(&self, other: &Self) -> Ordering;
    #[rustc_diagnostic_item = "cmp_ord_max"]
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
    #[rustc_diagnostic_item = "cmp_ord_min"]
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

// References compare by value. Every method forwards (core/src/cmp.rs `mod impls`): a type that
// overrides `lt`/`ne` keeps its own through a reference (an `Ord::cmp` written as `self < other`
// on references, whose `partial_cmp` is `cmp`, recursed until the stack overflowed when a
// reference's `lt` went through `partial_cmp`).
impl<'a, 'b, A: crate::marker::PointeeSized + PartialEq<B>, B: crate::marker::PointeeSized> PartialEq<&'b B> for &'a A {
    fn eq(&self, other: &&'b B) -> bool {
        PartialEq::eq(*self, *other)
    }
    fn ne(&self, other: &&'b B) -> bool {
        PartialEq::ne(*self, *other)
    }
}
impl<'a, 'b, A: crate::marker::PointeeSized + PartialEq<B>, B: crate::marker::PointeeSized> PartialEq<&'b mut B> for &'a mut A {
    fn eq(&self, other: &&'b mut B) -> bool {
        PartialEq::eq(*self, *other)
    }
    fn ne(&self, other: &&'b mut B) -> bool {
        PartialEq::ne(*self, *other)
    }
}
impl<'a, 'b, A: crate::marker::PointeeSized + PartialEq<B>, B: crate::marker::PointeeSized> PartialEq<&'b mut B> for &'a A {
    fn eq(&self, other: &&'b mut B) -> bool {
        PartialEq::eq(*self, *other)
    }
    fn ne(&self, other: &&'b mut B) -> bool {
        PartialEq::ne(*self, *other)
    }
}
impl<'a, 'b, A: crate::marker::PointeeSized + PartialEq<B>, B: crate::marker::PointeeSized> PartialEq<&'b B> for &'a mut A {
    fn eq(&self, other: &&'b B) -> bool {
        PartialEq::eq(*self, *other)
    }
    fn ne(&self, other: &&'b B) -> bool {
        PartialEq::ne(*self, *other)
    }
}
impl<'a, A: crate::marker::PointeeSized + Eq> Eq for &'a A {}
impl<'a, A: crate::marker::PointeeSized + Eq> Eq for &'a mut A {}
impl<'a, 'b, A: crate::marker::PointeeSized + PartialOrd<B>, B: crate::marker::PointeeSized> PartialOrd<&'b B> for &'a A {
    fn partial_cmp(&self, other: &&'b B) -> Option<Ordering> {
        PartialOrd::partial_cmp(*self, *other)
    }
    fn lt(&self, other: &&'b B) -> bool {
        PartialOrd::lt(*self, *other)
    }
    fn le(&self, other: &&'b B) -> bool {
        PartialOrd::le(*self, *other)
    }
    fn gt(&self, other: &&'b B) -> bool {
        PartialOrd::gt(*self, *other)
    }
    fn ge(&self, other: &&'b B) -> bool {
        PartialOrd::ge(*self, *other)
    }
}
impl<'a, 'b, A: crate::marker::PointeeSized + PartialOrd<B>, B: crate::marker::PointeeSized> PartialOrd<&'b mut B> for &'a mut A {
    fn partial_cmp(&self, other: &&'b mut B) -> Option<Ordering> {
        PartialOrd::partial_cmp(*self, *other)
    }
    fn lt(&self, other: &&'b mut B) -> bool {
        PartialOrd::lt(*self, *other)
    }
    fn le(&self, other: &&'b mut B) -> bool {
        PartialOrd::le(*self, *other)
    }
    fn gt(&self, other: &&'b mut B) -> bool {
        PartialOrd::gt(*self, *other)
    }
    fn ge(&self, other: &&'b mut B) -> bool {
        PartialOrd::ge(*self, *other)
    }
}
impl<'a, A: crate::marker::PointeeSized + Ord> Ord for &'a A {
    fn cmp(&self, other: &&'a A) -> Ordering {
        (**self).cmp(*other)
    }
}
impl<'a, A: crate::marker::PointeeSized + Ord> Ord for &'a mut A {
    fn cmp(&self, other: &&'a mut A) -> Ordering {
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
