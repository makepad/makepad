//! Operator traits, ranges, Fn traits, ControlFlow. Operators on primitives are built in;
//! their trait impls (for generic code) are generated in num/prim_traits.rs.

use crate::option::Option::{self, None, Some};

pub trait Drop {
    fn drop(&mut self);
}

pub trait Deref {
    type Target: ?Sized;
    fn deref(&self) -> &Self::Target;
}
pub trait DerefMut: Deref {
    fn deref_mut(&mut self) -> &mut Self::Target;
}

impl<'a, T: ?Sized> Deref for &'a T {
    type Target = T;
    fn deref(&self) -> &T {
        *self
    }
}
impl<'a, T: ?Sized> Deref for &'a mut T {
    type Target = T;
    fn deref(&self) -> &T {
        &**self
    }
}
impl<'a, T: ?Sized> DerefMut for &'a mut T {
    fn deref_mut(&mut self) -> &mut T {
        &mut **self
    }
}

pub trait Index<Idx: ?Sized> {
    type Output: ?Sized;
    fn index(&self, index: Idx) -> &Self::Output;
}
pub trait IndexMut<Idx: ?Sized>: Index<Idx> {
    fn index_mut(&mut self, index: Idx) -> &mut Self::Output;
}

// Closures. `F: FnMut(A, B) -> R` is sugar for `F: FnMut<(A, B), Output = R>`; Rapid
// implements these for closures, fn items and fn pointers itself.
pub trait FnOnce<Args> {
    type Output;
    fn call_once(self, args: Args) -> Self::Output;
}
pub trait FnMut<Args>: FnOnce<Args> {
    fn call_mut(&mut self, args: Args) -> Self::Output;
}
pub trait Fn<Args>: FnMut<Args> {
    fn call(&self, args: Args) -> Self::Output;
}

pub trait Add<Rhs = Self> {
    type Output;
    fn add(self, rhs: Rhs) -> Self::Output;
}
pub trait Sub<Rhs = Self> {
    type Output;
    fn sub(self, rhs: Rhs) -> Self::Output;
}
pub trait Mul<Rhs = Self> {
    type Output;
    fn mul(self, rhs: Rhs) -> Self::Output;
}
pub trait Div<Rhs = Self> {
    type Output;
    fn div(self, rhs: Rhs) -> Self::Output;
}
pub trait Rem<Rhs = Self> {
    type Output;
    fn rem(self, rhs: Rhs) -> Self::Output;
}
pub trait Neg {
    type Output;
    fn neg(self) -> Self::Output;
}
pub trait Not {
    type Output;
    fn not(self) -> Self::Output;
}
pub trait BitAnd<Rhs = Self> {
    type Output;
    fn bitand(self, rhs: Rhs) -> Self::Output;
}
pub trait BitOr<Rhs = Self> {
    type Output;
    fn bitor(self, rhs: Rhs) -> Self::Output;
}
pub trait BitXor<Rhs = Self> {
    type Output;
    fn bitxor(self, rhs: Rhs) -> Self::Output;
}
pub trait Shl<Rhs = Self> {
    type Output;
    fn shl(self, rhs: Rhs) -> Self::Output;
}
pub trait Shr<Rhs = Self> {
    type Output;
    fn shr(self, rhs: Rhs) -> Self::Output;
}

pub trait AddAssign<Rhs = Self> {
    fn add_assign(&mut self, rhs: Rhs);
}
pub trait SubAssign<Rhs = Self> {
    fn sub_assign(&mut self, rhs: Rhs);
}
pub trait MulAssign<Rhs = Self> {
    fn mul_assign(&mut self, rhs: Rhs);
}
pub trait DivAssign<Rhs = Self> {
    fn div_assign(&mut self, rhs: Rhs);
}
pub trait RemAssign<Rhs = Self> {
    fn rem_assign(&mut self, rhs: Rhs);
}
pub trait BitAndAssign<Rhs = Self> {
    fn bitand_assign(&mut self, rhs: Rhs);
}
pub trait BitOrAssign<Rhs = Self> {
    fn bitor_assign(&mut self, rhs: Rhs);
}
pub trait BitXorAssign<Rhs = Self> {
    fn bitxor_assign(&mut self, rhs: Rhs);
}
pub trait ShlAssign<Rhs = Self> {
    fn shl_assign(&mut self, rhs: Rhs);
}
pub trait ShrAssign<Rhs = Self> {
    fn shr_assign(&mut self, rhs: Rhs);
}

// ---------------------------------------------------------------- ranges

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct Range<Idx> {
    pub start: Idx,
    pub end: Idx,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RangeFrom<Idx> {
    pub start: Idx,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeTo<Idx> {
    pub end: Idx,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RangeToInclusive<Idx> {
    pub end: Idx,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RangeFull;

/// `start..=end`. `exhausted` is set once iteration has yielded `end` (as in real core).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RangeInclusive<Idx> {
    pub(crate) start: Idx,
    pub(crate) end: Idx,
    pub(crate) exhausted: bool,
}

impl<Idx> RangeInclusive<Idx> {
    pub const fn new(start: Idx, end: Idx) -> RangeInclusive<Idx> {
        RangeInclusive { start, end, exhausted: false }
    }
    pub const fn start(&self) -> &Idx {
        &self.start
    }
    pub const fn end(&self) -> &Idx {
        &self.end
    }
    pub fn into_inner(self) -> (Idx, Idx) {
        (self.start, self.end)
    }
}

impl<Idx: PartialOrd<Idx>> Range<Idx> {
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        self.start <= *item && *item < self.end
    }
    pub fn is_empty(&self) -> bool {
        !(self.start < self.end)
    }
}

impl<Idx: PartialOrd<Idx>> RangeFrom<Idx> {
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        self.start <= *item
    }
}

impl<Idx: PartialOrd<Idx>> RangeTo<Idx> {
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        *item < self.end
    }
}

impl<Idx: PartialOrd<Idx>> RangeToInclusive<Idx> {
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        *item <= self.end
    }
}

impl<Idx: PartialOrd<Idx>> RangeInclusive<Idx> {
    pub fn contains<U: ?Sized + PartialOrd<Idx>>(&self, item: &U) -> bool
    where
        Idx: PartialOrd<U>,
    {
        self.start <= *item && *item <= self.end
    }
    pub fn is_empty(&self) -> bool {
        self.exhausted || !(self.start <= self.end)
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Bound<T> {
    Included(T),
    Excluded(T),
    Unbounded,
}

impl<T> Bound<T> {
    pub fn as_ref(&self) -> Bound<&T> {
        match self {
            Bound::Included(x) => Bound::Included(x),
            Bound::Excluded(x) => Bound::Excluded(x),
            Bound::Unbounded => Bound::Unbounded,
        }
    }
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Bound<U> {
        match self {
            Bound::Included(x) => Bound::Included(f(x)),
            Bound::Excluded(x) => Bound::Excluded(f(x)),
            Bound::Unbounded => Bound::Unbounded,
        }
    }
}

impl<T: Clone> Bound<&T> {
    pub fn cloned(self) -> Bound<T> {
        match self {
            Bound::Included(x) => Bound::Included(x.clone()),
            Bound::Excluded(x) => Bound::Excluded(x.clone()),
            Bound::Unbounded => Bound::Unbounded,
        }
    }
}

pub trait RangeBounds<T: ?Sized> {
    fn start_bound(&self) -> Bound<&T>;
    fn end_bound(&self) -> Bound<&T>;
    fn contains<U>(&self, item: &U) -> bool
    where
        T: PartialOrd<U>,
        U: ?Sized + PartialOrd<T>,
    {
        (match self.start_bound() {
            Bound::Included(s) => *s <= *item,
            Bound::Excluded(s) => *s < *item,
            Bound::Unbounded => true,
        }) && (match self.end_bound() {
            Bound::Included(e) => *item <= *e,
            Bound::Excluded(e) => *item < *e,
            Bound::Unbounded => true,
        })
    }
}

impl<T: ?Sized> RangeBounds<T> for RangeFull {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
}
impl<T> RangeBounds<T> for Range<T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Excluded(&self.end)
    }
}
impl<T> RangeBounds<T> for RangeFrom<T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
}
impl<T> RangeBounds<T> for RangeTo<T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Excluded(&self.end)
    }
}
impl<T> RangeBounds<T> for RangeToInclusive<T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Included(&self.end)
    }
}
impl<T> RangeBounds<T> for RangeInclusive<T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(&self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        if self.exhausted {
            Bound::Excluded(&self.end)
        } else {
            Bound::Included(&self.end)
        }
    }
}
impl<T> RangeBounds<T> for (Bound<T>, Bound<T>) {
    fn start_bound(&self) -> Bound<&T> {
        self.0.as_ref()
    }
    fn end_bound(&self) -> Bound<&T> {
        self.1.as_ref()
    }
}

pub enum ControlFlow<B, C = ()> {
    Continue(C),
    Break(B),
}

impl<B, C> ControlFlow<B, C> {
    pub fn is_break(&self) -> bool {
        matches!(self, ControlFlow::Break(_))
    }
    pub fn is_continue(&self) -> bool {
        matches!(self, ControlFlow::Continue(_))
    }
    pub fn break_value(self) -> Option<B> {
        match self {
            ControlFlow::Break(b) => Some(b),
            ControlFlow::Continue(_) => None,
        }
    }
}
