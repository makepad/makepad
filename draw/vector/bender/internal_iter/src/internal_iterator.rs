use crate::FromInternalIterator;

pub trait InternalIterator {
    type Item;

    fn for_each(self, f: &mut impl FnMut(Self::Item) -> bool) -> bool;

    fn collect<F: FromInternalIterator<Self::Item>>(self) -> F
    where
        Self: Sized,
    {
        FromInternalIterator::from_internal_iter(self)
    }
}

impl<I: Iterator> InternalIterator for I {
    type Item = I::Item;

    fn for_each(self, f: &mut impl FnMut(Self::Item) -> bool) -> bool {
        for item in self {
            if !f(item) {
                return false;
            }
        }
        true
    }
}
