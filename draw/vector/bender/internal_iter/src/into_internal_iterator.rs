use crate::InternalIterator;

pub trait IntoInternalIterator {
    type Item;

    type IntoInternalIter: InternalIterator<Item = Self::Item>;

    fn into_internal_iter(self) -> Self::IntoInternalIter;
}

impl<I: InternalIterator> IntoInternalIterator for I {
    type Item = <Self as InternalIterator>::Item;

    type IntoInternalIter = Self;

    fn into_internal_iter(self) -> Self::IntoInternalIter {
        self
    }
}
