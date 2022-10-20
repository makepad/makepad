use {
    crate::{Difference, Intersection, Iter, SymmetricDifference, Union},
    std::ops::Range,
};

/// An ordered set of non-overlapping, non-adjacent [`Range`]s.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RangeSet<T> {
    ranges: Vec<Range<T>>,
}

impl<T> RangeSet<T> {
    /// Creates a new, empty `RangeSet`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    /// Constructs a new, empty `RangeSet` with at least the given `capacity`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            ranges: Vec::with_capacity(capacity),
        }
    }

    /// Returns a slice of all [`Range`]s in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn as_slice(&self) -> &[Range<T>] {
        self.ranges.as_slice()
    }

    /// Returns `true` if `self` is empty.
    ///
    /// # Performance
    ///
    /// Rusn in O(1) time.
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }

    /// Returns the number of [`Range`]s in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Returns an iterator over the [`Range`]s in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter::new(&self.ranges)
    }

    /// Returns `true` if `self` contains the given `range`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn contains(&self, range: &Range<T>) -> bool
    where
        T: Ord,
    {
        use std::cmp::Ordering;

        match self.ranges.binary_search_by(|mid| {
            if mid.end < range.start {
                return Ordering::Less;
            }
            if mid.start > range.start {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => self.ranges[index].end >= range.end,
            Err(_) => false,
        }
    }

    /// Returns an iterator that yields the [`Range`]s in the difference of `self` and `other`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn difference<'a>(&'a self, other: &'a Self) -> Difference<'a, T>
    where
        T: Clone + Ord,
    {
        Difference::new(&self.ranges, &other.ranges)
    }

    /// Returns an iterator that yields the [`Range`]s in the intersection of `self` and `other`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn intersection<'a>(&'a self, other: &'a Self) -> Intersection<'a, T>
    where
        T: Clone + Ord,
    {
        Intersection::new(&self.ranges, &other.ranges)
    }

    /// Returns an iterator that yields the [`Range`]s in the symmetric difference of `self` and
    /// `other`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn symmetric_difference<'a>(&'a self, other: &'a Self) -> SymmetricDifference<'a, T>
    where
        T: Clone + Ord,
    {
        SymmetricDifference::new(&self.ranges, &other.ranges)
    }

    /// Returns an iterator that yields the [`Range`]s in the union of `self` and `other`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn union<'a>(&'a self, other: &'a Self) -> Union<'a, T>
    where
        T: Clone + Ord,
    {
        Union::new(&self.ranges, &other.ranges)
    }

    /// Adds the given `range` to `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log n) time.
    pub fn insert(&mut self, mut range: Range<T>)
    where
        T: Clone + Ord,
    {
        use std::{cmp::Ordering, iter};

        if range.is_empty() {
            return;
        }
        let start = match self.ranges.binary_search_by(|mid| {
            if mid.end < range.start {
                return Ordering::Less;
            }
            if mid.start > range.start {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                range.start = range.start.min(self.ranges[index].start.clone());
                index
            }
            Err(index) => index,
        };
        let end = match self.ranges.binary_search_by(|mid| {
            if mid.end < range.end {
                return Ordering::Less;
            }
            if mid.start > range.end {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => {
                range.end = range.end.max(self.ranges[index].end.clone());
                index + 1
            }
            Err(index) => index,
        };
        self.ranges.splice(start..end, iter::once(range));
    }

    /// Clears the set, removing all [`Range`]s.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn clear(&mut self) {
        self.ranges.clear();
    }
}

impl<T> Default for RangeSet<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<Range<T>> for RangeSet<T> {
    fn from(range: Range<T>) -> Self {
        Self {
            ranges: vec![range],
        }
    }
}

impl<T: Clone + Ord> Extend<Range<T>> for RangeSet<T> {
    /// Extends a `RangeSet` with the contents of an iterator.
    ///
    /// # Performance
    ///
    /// Runs in O(n * log(n)) time.
    fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = Range<T>>,
    {
        for range in iter.into_iter() {
            self.insert(range);
        }
    }
}

impl<T: Clone + Ord> FromIterator<Range<T>> for RangeSet<T> {
    /// Creates a `RangeSet` from an iterator.
    ///
    /// # Performance
    ///
    /// Runs in O(n * log(n)) time.
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = Range<T>>,
    {
        let mut range_set = RangeSet::new();
        range_set.extend(iter);
        range_set
    }
}

impl<'a, T> IntoIterator for &'a RangeSet<T> {
    type IntoIter = Iter<'a, T>;
    type Item = &'a Range<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
