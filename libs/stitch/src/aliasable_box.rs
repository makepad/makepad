use std::{
    fmt,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    ptr::NonNull,
};

pub(crate) struct AliasableBox<T>
where
    T: ?Sized,
{
    ptr: NonNull<T>,
}

impl<T> AliasableBox<T>
where
    T: ?Sized,
{
    pub(crate) fn from_box(boxed: Box<T>) -> Self {
        Self {
            ptr: unsafe { NonNull::new_unchecked(Box::into_raw(boxed)) },
        }
    }

    pub(crate) fn as_raw(&self) -> NonNull<T> {
        self.ptr
    }
}

impl<T> fmt::Debug for AliasableBox<T>
where
    T: ?Sized + fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T> Deref for AliasableBox<T>
where
    T: ?Sized,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for AliasableBox<T>
where
    T: ?Sized,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for AliasableBox<T>
where
    T: ?Sized,
{
    fn drop(&mut self) {
        let this = ManuallyDrop::new(self);
        drop(unsafe { Box::from_raw(this.ptr.as_ptr()) });
    }
}
