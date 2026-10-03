//! Borrow, ToOwned and Cow.

use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::ops::Deref;

pub trait Borrow<Borrowed: ?Sized> {
    fn borrow(&self) -> &Borrowed;
}

pub trait BorrowMut<Borrowed: ?Sized>: Borrow<Borrowed> {
    fn borrow_mut(&mut self) -> &mut Borrowed;
}

impl<T: ?Sized> Borrow<T> for T {
    fn borrow(&self) -> &T {
        self
    }
}
impl<T: ?Sized> BorrowMut<T> for T {
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}
impl<'a, T: ?Sized> Borrow<T> for &'a T {
    fn borrow(&self) -> &T {
        &**self
    }
}
impl<'a, T: ?Sized> Borrow<T> for &'a mut T {
    fn borrow(&self) -> &T {
        &**self
    }
}
impl<'a, T: ?Sized> BorrowMut<T> for &'a mut T {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}

pub trait ToOwned {
    type Owned: Borrow<Self>;
    fn to_owned(&self) -> Self::Owned;
    fn clone_into(&self, target: &mut Self::Owned) {
        *target = self.to_owned();
    }
}

impl<T: Clone> ToOwned for T {
    type Owned = T;
    fn to_owned(&self) -> T {
        self.clone()
    }
}

/// Clone-on-write smart pointer.
pub enum Cow<'a, B: ?Sized + ToOwned + 'a> {
    Borrowed(&'a B),
    Owned(<B as ToOwned>::Owned),
}

pub use Cow::{Borrowed, Owned};

impl<'a, B: ?Sized + ToOwned> Cow<'a, B> {
    pub fn is_borrowed(&self) -> bool {
        matches!(self, Borrowed(_))
    }
    pub fn is_owned(&self) -> bool {
        !self.is_borrowed()
    }
    pub fn to_mut(&mut self) -> &mut <B as ToOwned>::Owned {
        if let Borrowed(b) = *self {
            *self = Owned(b.to_owned());
        }
        match self {
            Borrowed(_) => unsafe { crate::hint::unreachable_unchecked() },
            Owned(o) => o,
        }
    }
    pub fn into_owned(self) -> <B as ToOwned>::Owned {
        match self {
            Borrowed(b) => b.to_owned(),
            Owned(o) => o,
        }
    }
}

impl<'a, B: ?Sized + ToOwned> Deref for Cow<'a, B>
where
    B::Owned: Borrow<B>,
{
    type Target = B;
    fn deref(&self) -> &B {
        match *self {
            Borrowed(b) => b,
            Owned(ref o) => o.borrow(),
        }
    }
}

impl<'a, B: ?Sized + ToOwned> Clone for Cow<'a, B> {
    fn clone(&self) -> Cow<'a, B> {
        match *self {
            Borrowed(b) => Borrowed(b),
            Owned(ref o) => {
                let b: &B = o.borrow();
                Owned(b.to_owned())
            }
        }
    }
}

impl<'a, B: ?Sized + ToOwned + fmt::Debug> fmt::Debug for Cow<'a, B>
where
    B::Owned: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Borrowed(ref b) => fmt::Debug::fmt(b, f),
            Owned(ref o) => fmt::Debug::fmt(o, f),
        }
    }
}

impl<'a, B: ?Sized + ToOwned + fmt::Display> fmt::Display for Cow<'a, B>
where
    B::Owned: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Borrowed(ref b) => fmt::Display::fmt(b, f),
            Owned(ref o) => fmt::Display::fmt(o, f),
        }
    }
}

impl<'a, B: ?Sized + ToOwned + PartialEq> PartialEq for Cow<'a, B> {
    fn eq(&self, other: &Cow<'a, B>) -> bool {
        **self == **other
    }
}
impl<'a, B: ?Sized + ToOwned + Eq> Eq for Cow<'a, B> {}
impl<'a, B: ?Sized + ToOwned + PartialOrd> PartialOrd for Cow<'a, B> {
    fn partial_cmp(&self, other: &Cow<'a, B>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}
impl<'a, B: ?Sized + ToOwned + Ord> Ord for Cow<'a, B> {
    fn cmp(&self, other: &Cow<'a, B>) -> Ordering {
        (**self).cmp(&**other)
    }
}
impl<'a, B: ?Sized + ToOwned + Hash> Hash for Cow<'a, B> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}
impl<'a, B: ?Sized + ToOwned> AsRef<B> for Cow<'a, B> {
    fn as_ref(&self) -> &B {
        self
    }
}
impl<'a, B: ?Sized + ToOwned> Default for Cow<'a, B>
where
    B::Owned: Default,
{
    fn default() -> Cow<'a, B> {
        Owned(<B::Owned as Default>::default())
    }
}
