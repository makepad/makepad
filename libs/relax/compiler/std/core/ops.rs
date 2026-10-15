// ported from library/core/src/ops/function.rs @8d1a764
// ported from library/core/src/ops/async_function.rs @8d1a764
// ported from library/core/src/ops/unsize.rs @8d1a764
//! Operator traits, ranges, Fn traits, ControlFlow. Operators on primitives are built in;
//! their trait impls (for generic code) are generated in num/prim_traits.rs.

use crate::option::Option::{self, None, Some};
use crate::marker::{PointeeSized, Unsize};

#[cfg_attr(not(relax_check), lang = "dispatch_from_dyn")]
pub trait DispatchFromDyn<T>: Sized {}
impl<'a, T: PointeeSized + Unsize<U>, U: PointeeSized> DispatchFromDyn<&'a U> for &'a T {}
impl<'a, T: PointeeSized + Unsize<U>, U: PointeeSized> DispatchFromDyn<&'a mut U> for &'a mut T {}
impl<T: PointeeSized + Unsize<U>, U: PointeeSized> DispatchFromDyn<*const U> for *const T {}
impl<T: PointeeSized + Unsize<U>, U: PointeeSized> DispatchFromDyn<*mut U> for *mut T {}

#[cfg_attr(not(relax_check), lang = "drop")]
pub trait Drop {
    fn drop(&mut self);
}

#[cfg_attr(not(relax_check), lang = "deref")]
pub trait Deref: PointeeSized {
    #[cfg_attr(not(relax_check), lang = "deref_target")]
    type Target: ?Sized;
    fn deref(&self) -> &Self::Target;
}
#[cfg_attr(not(relax_check), lang = "deref_mut")]
pub trait DerefMut: Deref + PointeeSized {
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

#[cfg_attr(not(relax_check), lang = "index")]
pub trait Index<Idx: ?Sized> {
    type Output: ?Sized;
    #[track_caller]
    fn index(&self, index: Idx) -> &Self::Output;
}
#[cfg_attr(not(relax_check), lang = "index_mut")]
pub trait IndexMut<Idx: ?Sized>: Index<Idx> {
    #[track_caller]
    fn index_mut(&mut self, index: Idx) -> &mut Self::Output;
}

// Closures. `F: FnMut(A, B) -> R` is sugar for `F: FnMut<(A, B), Output = R>`; Relax
// implements these for closures, fn items and fn pointers itself.
#[cfg_attr(not(relax_check), lang = "fn_once")]
#[fundamental]
pub trait FnOnce<Args> {
    #[cfg_attr(not(relax_check), lang = "fn_once_output")]
    type Output;
    extern "rust-call" fn call_once(self, args: Args) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "fn_mut")]
#[fundamental]
pub trait FnMut<Args>: FnOnce<Args> {
    extern "rust-call" fn call_mut(&mut self, args: Args) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "fn")]
#[fundamental]
pub trait Fn<Args>: FnMut<Args> {
    extern "rust-call" fn call(&self, args: Args) -> Self::Output;
}

// ported from library/core/src/ops/async_function.rs:1-164 @8d1a764
use crate::future::Future;
use crate::marker::Tuple;

#[rustc_paren_sugar]
#[must_use = "async closures are lazy and do nothing unless called"]
#[cfg_attr(not(relax_check), lang = "async_fn")]
pub trait AsyncFn<Args: Tuple>: AsyncFnMut<Args> {
    extern "rust-call" fn async_call(&self, args: Args) -> Self::CallRefFuture<'_>;
}

#[rustc_paren_sugar]
#[must_use = "async closures are lazy and do nothing unless called"]
#[cfg_attr(not(relax_check), lang = "async_fn_mut")]
pub trait AsyncFnMut<Args: Tuple>: AsyncFnOnce<Args> {
    #[cfg_attr(not(relax_check), lang = "call_ref_future")]
    type CallRefFuture<'a>: Future<Output = Self::Output>
    where
        Self: 'a;

    extern "rust-call" fn async_call_mut(&mut self, args: Args) -> Self::CallRefFuture<'_>;
}

#[rustc_paren_sugar]
#[must_use = "async closures are lazy and do nothing unless called"]
#[cfg_attr(not(relax_check), lang = "async_fn_once")]
pub trait AsyncFnOnce<Args: Tuple> {
    #[cfg_attr(not(relax_check), lang = "call_once_future")]
    type CallOnceFuture: Future<Output = Self::Output>;

    #[cfg_attr(not(relax_check), lang = "async_fn_once_output")]
    type Output;

    extern "rust-call" fn async_call_once(self, args: Args) -> Self::CallOnceFuture;
}

mod impls {
    use super::{AsyncFn, AsyncFnMut, AsyncFnOnce};
    use crate::marker::Tuple;

    impl<A: Tuple, F: ?Sized> AsyncFn<A> for &F
    where
        F: AsyncFn<A>,
    {
        extern "rust-call" fn async_call(&self, args: A) -> Self::CallRefFuture<'_> {
            F::async_call(*self, args)
        }
    }

    impl<A: Tuple, F: ?Sized> AsyncFnMut<A> for &F
    where
        F: AsyncFn<A>,
    {
        type CallRefFuture<'a>
            = F::CallRefFuture<'a>
        where
            Self: 'a;

        extern "rust-call" fn async_call_mut(&mut self, args: A) -> Self::CallRefFuture<'_> {
            F::async_call(*self, args)
        }
    }

    impl<'a, A: Tuple, F: ?Sized> AsyncFnOnce<A> for &'a F
    where
        F: AsyncFn<A>,
    {
        type Output = F::Output;
        type CallOnceFuture = F::CallRefFuture<'a>;

        extern "rust-call" fn async_call_once(self, args: A) -> Self::CallOnceFuture {
            F::async_call(self, args)
        }
    }

    impl<A: Tuple, F: ?Sized> AsyncFnMut<A> for &mut F
    where
        F: AsyncFnMut<A>,
    {
        type CallRefFuture<'a>
            = F::CallRefFuture<'a>
        where
            Self: 'a;

        extern "rust-call" fn async_call_mut(&mut self, args: A) -> Self::CallRefFuture<'_> {
            F::async_call_mut(*self, args)
        }
    }

    impl<'a, A: Tuple, F: ?Sized> AsyncFnOnce<A> for &'a mut F
    where
        F: AsyncFnMut<A>,
    {
        type Output = F::Output;
        type CallOnceFuture = F::CallRefFuture<'a>;

        extern "rust-call" fn async_call_once(self, args: A) -> Self::CallOnceFuture {
            F::async_call_mut(self, args)
        }
    }
}

// library/core/src/ops/async_function.rs internal_implementation_detail: the solver's
// pending closure-kind check of `AsyncFn*` (its `Upvars` belongs to coroutine-closures,
// which Relax does not represent).
#[cfg_attr(not(relax_check), lang = "async_fn_kind_helper")]
pub trait AsyncFnKindHelper<GoalKind> {}

#[cfg_attr(not(relax_check), lang = "add")]
pub trait Add<Rhs = Self> {
    type Output;
    fn add(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "sub")]
pub trait Sub<Rhs = Self> {
    type Output;
    fn sub(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "mul")]
pub trait Mul<Rhs = Self> {
    type Output;
    fn mul(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "div")]
pub trait Div<Rhs = Self> {
    type Output;
    fn div(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "rem")]
pub trait Rem<Rhs = Self> {
    type Output;
    fn rem(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "neg")]
pub trait Neg {
    type Output;
    fn neg(self) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "not")]
pub trait Not {
    type Output;
    fn not(self) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "bitand")]
pub trait BitAnd<Rhs = Self> {
    type Output;
    fn bitand(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "bitor")]
pub trait BitOr<Rhs = Self> {
    type Output;
    fn bitor(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "bitxor")]
pub trait BitXor<Rhs = Self> {
    type Output;
    fn bitxor(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "shl")]
pub trait Shl<Rhs = Self> {
    type Output;
    fn shl(self, rhs: Rhs) -> Self::Output;
}
#[cfg_attr(not(relax_check), lang = "shr")]
pub trait Shr<Rhs = Self> {
    type Output;
    fn shr(self, rhs: Rhs) -> Self::Output;
}

#[cfg_attr(not(relax_check), lang = "add_assign")]
pub trait AddAssign<Rhs = Self> {
    fn add_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "sub_assign")]
pub trait SubAssign<Rhs = Self> {
    fn sub_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "mul_assign")]
pub trait MulAssign<Rhs = Self> {
    fn mul_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "div_assign")]
pub trait DivAssign<Rhs = Self> {
    fn div_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "rem_assign")]
pub trait RemAssign<Rhs = Self> {
    fn rem_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "bitand_assign")]
pub trait BitAndAssign<Rhs = Self> {
    fn bitand_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "bitor_assign")]
pub trait BitOrAssign<Rhs = Self> {
    fn bitor_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "bitxor_assign")]
pub trait BitXorAssign<Rhs = Self> {
    fn bitxor_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "shl_assign")]
pub trait ShlAssign<Rhs = Self> {
    fn shl_assign(&mut self, rhs: Rhs);
}
#[cfg_attr(not(relax_check), lang = "shr_assign")]
pub trait ShrAssign<Rhs = Self> {
    fn shr_assign(&mut self, rhs: Rhs);
}

// ---------------------------------------------------------------- ranges

#[derive(Clone, Default, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "Range")]
pub struct Range<Idx> {
    pub start: Idx,
    pub end: Idx,
}

#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "RangeFrom")]
pub struct RangeFrom<Idx> {
    pub start: Idx,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "RangeTo")]
pub struct RangeTo<Idx> {
    pub end: Idx,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "RangeToInclusive")]
pub struct RangeToInclusive<Idx> {
    pub end: Idx,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "RangeFull")]
pub struct RangeFull;

/// `start..=end`. `exhausted` is set once iteration has yielded `end` (as in real core).
#[derive(Clone, PartialEq, Eq, Hash)]
#[cfg_attr(not(relax_check), lang = "RangeInclusive")]
pub struct RangeInclusive<Idx> {
    pub(crate) start: Idx,
    pub(crate) end: Idx,
    pub(crate) exhausted: bool,
}

// Debug as real core: the bounds formatted with the caller's options, then the operator.
impl<Idx: crate::fmt::Debug> crate::fmt::Debug for Range<Idx> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..")?;
        self.end.fmt(f)
    }
}
impl<Idx: crate::fmt::Debug> crate::fmt::Debug for RangeFrom<Idx> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..")
    }
}
impl<Idx: crate::fmt::Debug> crate::fmt::Debug for RangeTo<Idx> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.write_str("..")?;
        self.end.fmt(f)
    }
}
impl<Idx: crate::fmt::Debug> crate::fmt::Debug for RangeToInclusive<Idx> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.write_str("..=")?;
        self.end.fmt(f)
    }
}
impl crate::fmt::Debug for RangeFull {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.write_str("..")
    }
}
impl<Idx: crate::fmt::Debug> crate::fmt::Debug for RangeInclusive<Idx> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        self.start.fmt(f)?;
        f.write_str("..=")?;
        self.end.fmt(f)?;
        if self.exhausted {
            f.write_str(" (exhausted)")?;
        }
        Ok(())
    }
}

impl<Idx> RangeInclusive<Idx> {
    #[cfg_attr(not(relax_check), lang = "range_inclusive_new")]
    #[rustc_promotable]
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

// Ranges of references bound a T (real core; deliberately no `T: ?Sized` except the tuple form):
// `v.drain(start..)` with `start: &usize`.
impl<'a, T: ?Sized + 'a> RangeBounds<T> for (Bound<&'a T>, Bound<&'a T>) {
    fn start_bound(&self) -> Bound<&T> {
        self.0
    }
    fn end_bound(&self) -> Bound<&T> {
        self.1
    }
}
impl<T> RangeBounds<T> for RangeFrom<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
}
impl<T> RangeBounds<T> for RangeTo<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Excluded(self.end)
    }
}
impl<T> RangeBounds<T> for Range<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Excluded(self.end)
    }
}
impl<T> RangeBounds<T> for RangeInclusive<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Included(self.start)
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Included(self.end)
    }
}
impl<T> RangeBounds<T> for RangeToInclusive<&T> {
    fn start_bound(&self) -> Bound<&T> {
        Bound::Unbounded
    }
    fn end_bound(&self) -> Bound<&T> {
        Bound::Included(self.end)
    }
}

pub enum ControlFlow<B, C = ()> {
    #[cfg_attr(not(relax_check), lang = "Continue")]
    Continue(C),
    #[cfg_attr(not(relax_check), lang = "Break")]
    Break(B),
}

/// The `?` protocol as real core spells it (core::ops::try_trait): used by Iterator::try_fold /
/// try_for_each and the `?` operator.
#[cfg_attr(not(relax_check), lang = "Try")]
pub trait Try: FromResidual<Self::Residual> {
    type Output;
    type Residual: Residual<Self::Output>;
    #[cfg_attr(not(relax_check), lang = "from_output")]
    fn from_output(output: Self::Output) -> Self;
    #[cfg_attr(not(relax_check), lang = "branch")]
    fn branch(self) -> ControlFlow<Self::Residual, Self::Output>;
}

pub trait FromResidual<R> {
    #[cfg_attr(not(relax_check), lang = "from_residual")]
    fn from_residual(residual: R) -> Self;
}

// ported from library/core/src/ops/try_trait.rs:364-368 @8d1a764.
// Const-trait conditions are cut; the residual/output equality is preserved.
pub trait Residual<O>: Sized {
    type TryType: Try<Output = O, Residual = Self>;
}

// ported from library/core/src/option.rs:2908-2910,
// result.rs:2202-2204, ops/control_flow.rs:138-140 @8d1a764.
impl<T> Residual<T> for crate::option::Option<crate::convert::Infallible> {
    type TryType = crate::option::Option<T>;
}
impl<T, E> Residual<T> for crate::result::Result<crate::convert::Infallible, E> {
    type TryType = crate::result::Result<T, E>;
}
impl<B, C> Residual<C> for ControlFlow<B, crate::convert::Infallible> {
    type TryType = ControlFlow<B, C>;
}

impl<T> Try for crate::option::Option<T> {
    type Output = T;
    type Residual = crate::option::Option<crate::convert::Infallible>;
    fn from_output(output: T) -> Self {
        crate::option::Option::Some(output)
    }
    fn branch(self) -> ControlFlow<crate::option::Option<crate::convert::Infallible>, T> {
        match self {
            crate::option::Option::Some(v) => ControlFlow::Continue(v),
            crate::option::Option::None => ControlFlow::Break(crate::option::Option::None),
        }
    }
}

impl<T> FromResidual<crate::option::Option<crate::convert::Infallible>> for crate::option::Option<T> {
    fn from_residual(_residual: crate::option::Option<crate::convert::Infallible>) -> Self {
        crate::option::Option::None
    }
}

impl<T, E> Try for crate::result::Result<T, E> {
    type Output = T;
    type Residual = crate::result::Result<crate::convert::Infallible, E>;
    fn from_output(output: T) -> Self {
        crate::result::Result::Ok(output)
    }
    fn branch(self) -> ControlFlow<crate::result::Result<crate::convert::Infallible, E>, T> {
        match self {
            crate::result::Result::Ok(v) => ControlFlow::Continue(v),
            crate::result::Result::Err(e) => ControlFlow::Break(crate::result::Result::Err(e)),
        }
    }
}

impl<T, E, F: crate::convert::From<E>> FromResidual<crate::result::Result<crate::convert::Infallible, E>> for crate::result::Result<T, F> {
    fn from_residual(residual: crate::result::Result<crate::convert::Infallible, E>) -> Self {
        match residual {
            crate::result::Result::Err(e) => crate::result::Result::Err(F::from(e)),
            crate::result::Result::Ok(never) => match never {},
        }
    }
}

impl<B, C> Try for ControlFlow<B, C> {
    type Output = C;
    type Residual = ControlFlow<B, crate::convert::Infallible>;
    fn from_output(output: C) -> Self {
        ControlFlow::Continue(output)
    }
    fn branch(self) -> ControlFlow<ControlFlow<B, crate::convert::Infallible>, C> {
        match self {
            ControlFlow::Continue(c) => ControlFlow::Continue(c),
            ControlFlow::Break(b) => ControlFlow::Break(ControlFlow::Break(b)),
        }
    }
}

impl<B, C> FromResidual<ControlFlow<B, crate::convert::Infallible>> for ControlFlow<B, C> {
    fn from_residual(residual: ControlFlow<B, crate::convert::Infallible>) -> Self {
        match residual {
            ControlFlow::Break(b) => ControlFlow::Break(b),
            ControlFlow::Continue(never) => match never {},
        }
    }
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

// library/core/src/ops/function.rs:258-314; selection evaluates F's nested predicate.
impl<A: crate::marker::Tuple, F: ?Sized + Fn<A>> Fn<A> for &F {
    extern "rust-call" fn call(&self, args: A) -> F::Output { (**self).call(args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + Fn<A>> FnMut<A> for &F {
    extern "rust-call" fn call_mut(&mut self, args: A) -> F::Output { (**self).call(args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + Fn<A>> FnOnce<A> for &F {
    type Output = F::Output;
    extern "rust-call" fn call_once(self, args: A) -> F::Output { (*self).call(args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + FnMut<A>> FnMut<A> for &mut F {
    extern "rust-call" fn call_mut(&mut self, args: A) -> F::Output { (*self).call_mut(args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + FnMut<A>> FnOnce<A> for &mut F {
    type Output = F::Output;
    extern "rust-call" fn call_once(self, args: A) -> F::Output { (*self).call_mut(args) }
}

// ported from library/core/src/ops/function.rs:315-353 @8d1a764
extern "C" {
    #[cfg_attr(not(relax_check), lang = "code")]
    pub type Code;
}

#[cfg_attr(not(relax_check), lang = "fn_ptr_trait")]
#[fundamental]
#[rustc_deny_explicit_impl]
#[rustc_dyn_incompatible_trait]
#[fundamental]
pub trait FnPtr: Copy {
    fn addr(self) -> usize { self.as_ptr().addr().get() }
    #[cfg_attr(not(relax_check), lang = "fn_ptr_as_ptr")]
    fn as_ptr(self) -> crate::ptr::NonNull<Code>;
    #[cfg_attr(not(relax_check), lang = "fn_ptr_from_ptr")]
    unsafe fn from_ptr(ptr: crate::ptr::NonNull<Code>) -> Self;
}
