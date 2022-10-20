use {crate::Range, makepad_range_set::RangeSet};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct CharClass {
    range_set: RangeSet<u32>,
}

impl CharClass {
    pub(crate) fn new() -> Self {
        CharClass::default()
    }

    pub(crate) fn any() -> Self {
        Self {
            range_set: (0..char::MAX as u32 + 1).into(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.range_set.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.range_set.len()
    }

    pub(crate) fn contains(&self, ch: char) -> bool {
        self.range_set.contains(&(ch as u32..ch as u32 + 1))
    }

    pub(crate) fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.range_set.iter(),
        }
    }

    pub(crate) fn difference(&self, other: &Self, output: &mut Self) {
        output
            .range_set
            .extend(self.range_set.difference(&other.range_set));
    }

    pub(crate) fn intersection(&self, other: &Self, output: &mut Self) {
        output
            .range_set
            .extend(self.range_set.intersection(&other.range_set));
    }

    pub(crate) fn symmetric_difference(&self, other: &Self, output: &mut Self) {
        output
            .range_set
            .extend(self.range_set.symmetric_difference(&other.range_set));
    }

    pub(crate) fn union(&self, other: &Self, output: &mut Self) {
        output
            .range_set
            .extend(self.range_set.union(&other.range_set));
    }

    pub(crate) fn insert(&mut self, range: Range<char>) {
        self.range_set
            .insert(range.start as u32..range.end as u32 + 1);
    }

    pub(crate) fn clear(&mut self) {
        self.range_set.clear()
    }
}

impl<'a> IntoIterator for &'a CharClass {
    type IntoIter = Iter<'a>;
    type Item = Range<char>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Iter<'a> {
    iter: makepad_range_set::Iter<'a, u32>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Range<char>;

    fn next(&mut self) -> Option<Self::Item> {
        let range = self.iter.next()?;
        Some(Range::new(
            char::from_u32(range.start).unwrap(),
            char::from_u32(range.end - 1).unwrap(),
        ))
    }
}
