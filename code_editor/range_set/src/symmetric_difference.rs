use std::{iter::Cloned, ops::Range, slice};

/// An iterator that yields the [`Range`]s in the symmetric difference of two [`RangeSet`]s.
///
/// This `struct` is created by the [`symmetric_difference`] method on [`RangeSet`].
///
/// [`RangeSet`]: crate::RangeSet
/// [`symmetric_difference`]: crate::RangeSet::symmetric_difference
#[derive(Clone, Debug)]
pub struct SymmetricDifference<'a, T: 'a> {
    range_0: Option<Range<T>>,
    range_1: Option<Range<T>>,
    ranges_0: Cloned<slice::Iter<'a, Range<T>>>,
    ranges_1: Cloned<slice::Iter<'a, Range<T>>>,
}

impl<'a, T: 'a> SymmetricDifference<'a, T> {
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

impl<'a, T: Clone + Ord + 'a> Iterator for SymmetricDifference<'a, T> {
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
                            break Some(range_0);
                        }
                        Ordering::Greater => {
                            let start_1 = range_1.start.clone();
                            self.range_0 = Some(start_1.clone()..range_0.end);
                            self.range_1 = Some(range_1);
                            break Some(range_0.start..start_1);
                        }
                    },
                    Ordering::Equal => match range_0.end.cmp(&range_1.end) {
                        Ordering::Less => {
                            self.range_0 = self.ranges_0.next();
                            self.range_1 = Some(range_0.end.clone()..range_1.end);
                            continue;
                        }
                        Ordering::Equal => {
                            self.range_0 = self.ranges_0.next();
                            self.range_1 = self.ranges_1.next();
                            continue;
                        }
                        Ordering::Greater => {
                            self.range_0 = Some(range_1.end.clone()..range_0.end);
                            self.range_1 = self.ranges_1.next();
                            continue;
                        }
                    },
                    Ordering::Greater => match range_0.start.cmp(&range_1.end) {
                        Ordering::Less => {
                            self.range_1 = Some(range_0.start.clone()..range_1.end);
                            let range = range_1.start..range_0.start.clone();
                            self.range_0 = Some(range_0);
                            break Some(range);
                        }
                        Ordering::Equal | Ordering::Greater => {
                            self.range_0 = Some(range_0);
                            self.range_1 = self.ranges_1.next();
                            break Some(range_1);
                        }
                    },
                },
                (Some(range_0), None) => {
                    self.range_0 = self.ranges_0.next();
                    break Some(range_0);
                }
                (None, Some(range_1)) => {
                    self.range_1 = self.ranges_1.next();
                    break Some(range_1);
                }
                _ => break None,
            }
        }
    }
}
