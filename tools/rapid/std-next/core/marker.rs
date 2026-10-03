//! Marker traits and PhantomData. Copy/Send/Sync/Sized/Unpin are known to the compiler.

pub trait Sized {}
pub trait Copy: crate::clone::Clone {}
pub trait Send {}
pub trait Sync {}
pub trait Unpin {}

/// Zero-sized marker owning a `T` for variance/drop-check purposes.
pub struct PhantomData<T: ?Sized>;

impl<T: ?Sized> crate::clone::Clone for PhantomData<T> {
    fn clone(&self) -> PhantomData<T> {
        PhantomData
    }
}
impl<T: ?Sized> Copy for PhantomData<T> {}
impl<T: ?Sized> crate::default::Default for PhantomData<T> {
    fn default() -> PhantomData<T> {
        PhantomData
    }
}
impl<T: ?Sized> crate::cmp::PartialEq for PhantomData<T> {
    fn eq(&self, _o: &PhantomData<T>) -> bool {
        true
    }
}
impl<T: ?Sized> crate::cmp::Eq for PhantomData<T> {}
impl<T: ?Sized> crate::hash::Hash for PhantomData<T> {
    fn hash<H: crate::hash::Hasher>(&self, _state: &mut H) {}
}

pub struct PhantomPinned;
