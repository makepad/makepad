use {
    super::{case_fold, range::Range},
    std::{cmp::Ordering, collections::BTreeMap, slice},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CharClass {
    bounds: Vec<u32>,
}

impl CharClass {
    pub fn any() -> Self {
        Self {
            bounds: vec![0, char::MAX as u32 + 1],
        }
    }

    pub fn contains(&self, c: char) -> bool {
        let c = c as u32;
        match self.bounds.binary_search_by(|&bound| {
            if bound < c {
                return Ordering::Less;
            }
            if bound > c {
                return Ordering::Greater;
            }
            Ordering::Equal
        }) {
            Ok(index) => index % 2 == 0,
            Err(index) => index % 2 == 1,
        }
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.bounds.iter(),
        }
    }
}

impl<'a> IntoIterator for &'a CharClass {
    type Item = Range<char>;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, u32>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Range<char>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(Range {
            start: char::from_u32(*self.iter.next()?).unwrap(),
            end: char::from_u32(*self.iter.next()? - 1).unwrap(),
        })
    }
}

#[derive(Debug)]
pub struct Builder {
    deltas: BTreeMap<u32, i32>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            deltas: BTreeMap::new(),
        }
    }

    pub fn add_ranges(&mut self, negated: bool, folded: bool, ranges: &[Range<char>]) {
        if negated {
            if folded {
                let mut builder = Builder::new();
                for &range in ranges {
                    builder.add_range(true, range);
                }
                let class = builder.build(true);
                for range in &class {
                    self.add_range_internal(Range::new(range.start as u32, range.end as u32));
                }
            } else {
                let mut add_range = |range: Range<u32>| {
                    if range.start <= 0xD7FF && range.end >= 0xE000 {
                        self.add_range_internal(Range::new(range.start, 0xD7FF));
                        self.add_range_internal(Range::new(0xE000, range.end));
                        return;
                    }
                    self.add_range_internal(Range::new(range.start, range.end))
                };
                if ranges.is_empty() {
                    return;
                }
                let first_range_start = ranges.first().unwrap().start as u32;
                if first_range_start > 0 {
                    add_range(Range::new(0, first_range_start - 1));
                }
                for window in ranges.windows(2) {
                    let previous_range_end = window[0].end as u32;
                    let next_range_start = window[1].start as u32;
                    assert!(previous_range_end + 1 < next_range_start);
                    add_range(Range::new(previous_range_end + 1, next_range_start - 1));
                }
                let last_range_end = ranges.last().unwrap().end as u32;
                if last_range_end < 0x10FFFF {
                    add_range(Range::new(last_range_end + 1, 0x10FFFF));
                }
            }
        } else {
            for &range in ranges {
                self.add_range(folded, range);
            }
        }
    }

    pub fn add_range(&mut self, folded: bool, range: Range<char>) {
        if folded {
            case_fold::case_fold_range(range, |folded_range| {
                self.add_range_internal(Range::new(
                    folded_range.start as u32,
                    folded_range.end as u32,
                ))
            })
        } else {
            self.add_range_internal(Range::new(range.start as u32, range.end as u32))
        }
    }

    pub fn add_char(&mut self, folded: bool, c: char) {
        self.add_range(folded, Range::new(c, c));
    }

    pub fn build(&mut self, negated: bool) -> CharClass {
        let mut bounds = Vec::new();
        if negated {
            if !self.deltas.contains_key(&0) {
                bounds.push(0);
            }
            let mut count = 0;
            for (&bound, &delta) in self.deltas.range(1..0xD800) {
                let next_count = count + delta;
                if (count != 0) != (next_count != 0) {
                    bounds.push(bound);
                }
                count = next_count;
            }
            if !self.deltas.contains_key(&0xD800) {
                bounds.push(0xD800);
            }
            if !self.deltas.contains_key(&0xE000) {
                bounds.push(0xE000);
            }
            let mut count = 0;
            for (&bound, &delta) in self.deltas.range(0xE001..0x110000) {
                let next_count = count + delta;
                if (count != 0) != (next_count != 0) {
                    bounds.push(bound);
                }
                count = next_count;
            }
            if !self.deltas.contains_key(&0x110000) {
                bounds.push(0x110000);
            }
        } else {
            let mut count = 0;
            for (&bound, &delta) in &self.deltas {
                let next_count = count + delta;
                if (count != 0) != (next_count != 0) {
                    bounds.push(bound);
                }
                count = next_count;
            }
        }
        self.deltas.clear();
        CharClass { bounds }
    }

    fn add_range_internal(&mut self, range: Range<u32>) {
        use std::collections::btree_map::Entry;

        match self.deltas.entry(range.start) {
            Entry::Occupied(mut entry) => {
                if *entry.get() == -1 {
                    entry.remove();
                } else {
                    *entry.get_mut() += 1;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(1);
            }
        }
        match self.deltas.entry(range.end + 1) {
            Entry::Occupied(mut entry) => {
                if *entry.get() == 1 {
                    entry.remove();
                } else {
                    *entry.get_mut() -= 1;
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(-1);
            }
        }
    }
}
