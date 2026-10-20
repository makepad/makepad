//! Pin (kept for futures/task uses; Relax does not move pinned values).

use crate::fmt;
use crate::ops::{Deref, DerefMut, DispatchFromDyn};

// library/core/src/pin.rs:1827-1832,1899,1918,1937,1971 @8d1a764: a pinned pointer
// dispatches as its pointer does (`self: Pin<&mut Self>` methods are dyn-callable).
impl<Ptr, U> DispatchFromDyn<Pin<U>> for Pin<Ptr>
where
    Ptr: DispatchFromDyn<U> + PinSafePointer,
    U: PinSafePointer,
{
}
impl<Ptr: crate::ops::CoerceUnsized<U>, U> crate::ops::CoerceUnsized<Pin<U>> for Pin<Ptr> {}
pub unsafe trait PinSafePointer: Deref + Sized {}
unsafe impl<'a, T: ?Sized> PinSafePointer for &'a T {}
unsafe impl<'a, T: ?Sized> PinSafePointer for &'a mut T {}
unsafe impl<P: PinSafePointer> PinSafePointer for Pin<P> {}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
#[fundamental]
#[cfg_attr(not(relax_check), lang = "pin")]
pub struct Pin<P> {
    pointer: P,
}

/// `pin!(value)` (core/src/pin.rs): pins `value` in place on the stack -- a `Pin<&mut T>` to
/// a temporary that lives as long as the binding it initializes (temporary lifetime extension
/// through the struct literal, as rustc's `super let`).
#[macro_export]
macro_rules! pin {
    ($value:expr $(,)?) => {
        $crate::pin::Pin::<&mut _> { pointer: &mut { $value } }
    };
}

impl<P: Deref> Pin<P> {
    pub const fn new(pointer: P) -> Pin<P>
    where
        P::Target: Unpin,
    {
        Pin { pointer }
    }
    #[cfg_attr(not(relax_check), lang = "new_unchecked")]
    pub const unsafe fn new_unchecked(pointer: P) -> Pin<P> {
        Pin { pointer }
    }
    pub const fn as_ref(&self) -> Pin<&P::Target> {
        Pin { pointer: &*self.pointer }
    }
    pub const fn into_inner(pin: Pin<P>) -> P
    where
        P::Target: Unpin,
    {
        pin.pointer
    }
    pub const unsafe fn into_inner_unchecked(pin: Pin<P>) -> P {
        pin.pointer
    }
}

impl<P: DerefMut> Pin<P> {
    pub const fn as_mut(&mut self) -> Pin<&mut P::Target> {
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
    pub const fn get_ref(self) -> &'a T {
        self.pointer
    }
}

impl<'a, T: ?Sized> Pin<&'a mut T> {
    pub const fn get_mut(self) -> &'a mut T
    where
        T: Unpin,
    {
        self.pointer
    }
    pub const unsafe fn get_unchecked_mut(self) -> &'a mut T {
        self.pointer
    }
    pub unsafe fn map_unchecked_mut<U: ?Sized, F: FnOnce(&mut T) -> &mut U>(self, func: F) -> Pin<&'a mut U> {
        Pin { pointer: func(self.pointer) }
    }
}

impl<P: crate::ops::LegacyReceiver> crate::ops::LegacyReceiver for Pin<P> {}
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
