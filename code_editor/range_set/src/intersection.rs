use std::{iter::Cloned, ops::Range, slice};

/// An iterator that yields the [`Range`]s in the intersection of two [`RangeSet`]s.
///
/// This `struct` is created by the [`difference`] method on [`RangeSet`].
///
/// [`RangeSet`]: crate::RangeSet
/// [`difference`]: crate::RangeSet::difference
#[derive(Clone, Debug)]
pub struct Intersection<'a, T: 'a> {
    range_0: Option<Range<T>>,
    range_1: Option<Range<T>>,
    ranges_0: Cloned<slice::Iter<'a, Range<T>>>,
    ranges_1: Cloned<slice::Iter<'a, Range<T>>>,
}

impl<'a, T: 'a> Intersection<'a, T> {
    pub(crate) fn new(ranges_0: &'a [Range<T>], ranges_1: &'a [Range<T>]) -> Self
    where
        T: Clone,
    {
        let mut ranges_0 = ranges_0.iter().cloned();
        let mut ranges_1 = ranges_1.iter().cloned();
        Self {
            range_0: ranges_0.next(),
            range_1: ranges_1.next(),
            ranges_0,
            ranges_1,
        }
    }
}

impl<'a, T: Clone + Ord + 'a> Iterator for Intersection<'a, T> {
    type Item = Range<T>;

    /// Advances the iterator and returns the next [`Range`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(m + n) time.
    fn next(&mut self) -> Option<Self::Item> {
        use std::cmp::Ordering;

        loop {
            match (self.range_0.take(), self.range_1.take()) {
                (Some(range_0), Some(range_1)) => match range_0.start.cmp(&range_1.start) {
                    Ordering::Less => match range_0.end.cmp(&range_1.start) {
                        Ordering::Less | Ordering::Equal => {
                            self.range_0 = self.ranges_0.next();
                            self.range_1 = Some(range_1);
                            continue;
                        }
                        Ordering::Greater => {
                            self.range_0 = Some(range_1.start.clone()..range_0.end);
                            self.range_1 = Some(range_1);
                            continue;
                        }
                    },
                    Ordering::Equal => match range_0.end.cmp(&range_1.end) {
                        Ordering::Less => {
                            self.range_0 = self.ranges_0.next();
                            self.range_1 = Some(range_0.end.clone()..range_1.end);
                            break Some(range_0);
                        }
                        Ordering::Equal => {
                            self.range_0 = self.ranges_0.next();
                            self.range_1 = self.ranges_1.next();
                            break Some(range_0);
                        }
                        Ordering::Greater => {
                            self.range_0 = Some(range_1.end.clone()..range_0.end);
                            self.range_1 = self.ranges_1.next();
                            break Some(range_1);
                        }
                    },
                    Ordering::Greater => match range_0.start.cmp(&range_1.end) {
                        Ordering::Less => {
                            self.range_1 = Some(range_0.start.clone()..range_1.end);
                            self.range_0 = Some(range_0);
                            continue;
                        }
                        Ordering::Equal | Ordering::Greater => {
                            self.range_0 = Some(range_0);
                            self.range_1 = self.ranges_1.next();
                            continue;
                        }
                    },
                },
                _ => break None,
            }
        }
    }
}
