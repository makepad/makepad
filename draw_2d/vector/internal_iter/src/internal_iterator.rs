use crate::FromInternalIterator;

/// A trait for internal iterators. An internal iterator differs from a normal iterator in that its
/// iteration is controlled internally by the iterator itself, instead of externally by the calling
/// code. This means that instead of returning a single item on each call to `next`, internal
/// iterators call a closure for each item on a single call to `for_each`. This allows internal
/// operators to be implemented recursively, something that is not possible with normal iterators.
pub trait InternalIterator {
    type Item;

    /// Calls `f` with each item of `self`.
    ///
    /// If `f` returns `false`, iteration is aborted. If iteration was aborted, this function
    /// returns `false`.
    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(Self::Item) -> bool;

    /// Transforms `self` into a collection.
    fn collect<F>(self) -> F
    where
        Self: Sized,
        F: FromInternalIterator<Self::Item>,
    {
        FromInternalIterator::from_internal_iter(self)
    }

    /// Returns an internal iterator that applies `f` to each item of `self`.
    fn map<R, F>(self, f: F) -> Map<Self, F>
    where
        Self: Sized,
        F: FnMut(Self::Item) -> R,
    {
        Map {
            internal_iter: self,
            f,
        }
    }
}

impl<I> InternalIterator for I
where
    I: Iterator,
{
    type Item = I::Item;

    fn for_each<F>(self, f: &mut F) -> bool
    where
        F: FnMut(Self::Item) -> bool,
    {
        for item in self {
            if !f(item) {
                return false;
            }
        }
        true
    }
}

/// An internal iterator that applies `f` to each item of `self`.
#[derive(Clone, Debug)]
pub struct Map<I, F> {
    internal_iter: I,
    f: F,
}

impl<R, I, F> InternalIterator for Map<I, F>
where
    I: InternalIterator,
    F: FnMut(I::Item) -> R,
{
    type Item = R;

    fn for_each<G>(mut self, g: &mut G) -> bool
    where
        G: FnMut(Self::Item) -> bool,
    {
        self.internal_iter.for_each({
            let f = &mut self.f;
            &mut move |item| g((f)(item))
        })
    }
}
