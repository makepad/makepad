//! Pin (kept for the few makepad futures/task uses; Rapid does not move pinned values).

use crate::fmt;
use crate::ops::{Deref, DerefMut};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Pin<P> {
    pointer: P,
}

impl<P: Deref> Pin<P> {
    pub fn new(pointer: P) -> Pin<P>
    where
        P::Target: Unpin,
    {
        Pin { pointer }
    }
    pub unsafe fn new_unchecked(pointer: P) -> Pin<P> {
        Pin { pointer }
    }
    pub fn as_ref(&self) -> Pin<&P::Target> {
        Pin { pointer: &*self.pointer }
    }
    pub fn into_inner(pin: Pin<P>) -> P
    where
        P::Target: Unpin,
    {
        pin.pointer
    }
    pub unsafe fn into_inner_unchecked(pin: Pin<P>) -> P {
        pin.pointer
    }
}

impl<P: DerefMut> Pin<P> {
    pub fn as_mut(&mut self) -> Pin<&mut P::Target> {
        Pin { pointer: &mut *self.pointer }
    }
    pub fn set(&mut self, value: P::Target)
    where
        P::Target: Sized,
    {
        *self.pointer = value;
    }
}

impl<'a, T: ?Sized> Pin<&'a T> {
    pub fn get_ref(self) -> &'a T {
        self.pointer
    }
}

impl<'a, T: ?Sized> Pin<&'a mut T> {
    pub fn get_mut(self) -> &'a mut T
    where
        T: Unpin,
    {
        self.pointer
    }
    pub unsafe fn get_unchecked_mut(self) -> &'a mut T {
        self.pointer
    }
    pub unsafe fn map_unchecked_mut<U: ?Sized, F: FnOnce(&mut T) -> &mut U>(self, func: F) -> Pin<&'a mut U> {
        Pin { pointer: func(self.pointer) }
    }
}

impl<P: Deref> Deref for Pin<P> {
    type Target = P::Target;
    fn deref(&self) -> &P::Target {
        &*self.pointer
    }
}

impl<P: DerefMut> DerefMut for Pin<P>
where
    P::Target: Unpin,
{
    fn deref_mut(&mut self) -> &mut P::Target {
        &mut *self.pointer
    }
}

impl<P: fmt::Debug> fmt::Debug for Pin<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.pointer, f)
    }
}

impl<P: fmt::Display> fmt::Display for Pin<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.pointer, f)
    }
}

pub use crate::marker::Unpin;
