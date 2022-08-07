use crate::{ExtendFromInternalIterator, IntoInternalIterator};

/// A trait for conversion from an internal iterator.
///
/// This trait is commonly implemented for collections. It is useful when you have an internal
/// iterator but you need a collection.
pub trait FromInternalIterator<T> {
    /// Creates `Self` from an internal iterator.
    ///
    /// Note that `from_internal_iter` is almost never used directly. Instead, it is used by
    /// calling the `collect` method on `InternalIterator`.
    fn from_internal_iter<I>(internal_iter: I) -> Self
    where
        I: IntoInternalIterator<Item = T>;
}

impl<T> FromInternalIterator<T> for Vec<T> {
    fn from_internal_iter<I>(internal_iter: I) -> Self
    where
        I: IntoInternalIterator<Item = T>,
    {
        let mut vec = Vec::new();
        vec.extend_from_internal_iter(internal_iter);
        vec
    }
}
