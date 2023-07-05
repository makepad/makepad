use {
    crate::{Position, Range},
    std::slice,
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct RangeSet {
    ranges: Vec<Range>,
}

impl RangeSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ranges.len()
    }

    pub fn contains(&self, position: Position) -> bool {
        use std::cmp::Ordering;

        match self.ranges.binary_search_by(|range| {
            if range.end() < position {
                return Ordering::Less;
            }
            if range.start() > position {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => self.ranges[index].contains(position),
            Err(_) => false,
        }
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.ranges.iter(),
        }
    }
}

impl<const N: usize> From<[Range; N]> for RangeSet {
    fn from(array: [Range; N]) -> Self {
        array.into_iter().collect()
    }
}

impl From<Vec<Range>> for RangeSet {
    fn from(mut vec: Vec<Range>) -> Self {
        vec.sort_unstable_by_key(|range| range.start());
        let mut builder = Builder::new();
        for range in vec {
            builder.push(range);
        }
        builder.finish()
    }
}

impl FromIterator<Range> for RangeSet {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = Range>,
    {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

impl<'a> IntoIterator for &'a RangeSet {
    type Item = Range;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Builder {
    ranges: Vec<Range>,
}

impl Builder {
    pub fn new() -> Self {
        Self { ranges: Vec::new() }
    }

    pub fn push(&mut self, range: Range) {
        if let Some(last_range) = self.ranges.last_mut() {
            assert!(last_range.start() <= range.start());
            if let Some(merged_range) = last_range.try_merge(range) {
                *last_range = merged_range;
            }
        }
        self.ranges.push(range);
    }

    pub fn finish(self) -> RangeSet {
        RangeSet {
            ranges: self.ranges,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, Range>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Range;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().copied()
    }
}
