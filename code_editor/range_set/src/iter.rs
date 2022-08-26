use std::{ops::Range, slice};

/// An iterator over the [`Range`]s in a [`RangeSet`].
///
/// This `struct` is created by the [`iter`] method on [`RangeSet`].
///
/// [`RangeSet`]: crate::RangeSet
/// [`iter`]: crate::RangeSet::iter
#[derive(Clone, Debug)]
pub struct Iter<'a, T> {
    iter: slice::Iter<'a, Range<T>>,
}

impl<'a, T> Iter<'a, T> {
    pub(crate) fn new(ranges: &'a [Range<T>]) -> Self {
        Self {
            iter: ranges.iter(),
        }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a Range<T>;

    /// Advances the iterator and returns a reference to the next [`Range`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
