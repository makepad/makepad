use {
    crate::{
        position::Position,
        range::Range,
        size::Size
    },
    std::{
        collections::{btree_map::Entry, BTreeMap},
        slice::Iter,
    },
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct RangeSet {
    positions: Vec<Position>,
}

impl RangeSet {
    pub fn new() -> RangeSet {
        RangeSet::default()
    }
    
    pub fn contains_position(&self, position: Position) -> bool {
        match self.positions.binary_search(&position) {
            Ok(_) => false,
            Err(index) => index % 2 == 1,
        }
    }
    
    pub fn spans(&self) -> Spans {
        Spans {
            next_position_iter: self.positions.iter(),
            position: Position::origin(),
            is_included: false,
        }
    }
}

#[derive(Debug)]
pub struct Spans<'a> {
    next_position_iter: Iter<'a, Position>,
    position: Position,
    is_included: bool,
}

impl<'a> Iterator for Spans<'a> {
    type Item = Span;
    
    fn next(&mut self) -> Option<Self::Item> {
        let next_position = *self.next_position_iter.next() ?;
        let span = Span {
            len: next_position - self.position,
            is_included: self.is_included,
        };
        self.position = next_position;
        self.is_included = !self.is_included;
        Some(span)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Span {
    pub len: Size,
    pub is_included: bool,
}

#[derive(Debug, Default)]
pub struct Builder {
    deltas_by_position: BTreeMap<Position, i32>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }
    
    pub fn include(&mut self, range: Range) {
        match self.deltas_by_position.entry(range.start) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(1);
            }
        }
        match self.deltas_by_position.entry(range.end) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() -= 1;
                if *entry.get() == 0 {
                    entry.remove();
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(-1);
            }
        }
    }
    
    pub fn build(&self) -> RangeSet {
        let mut positions = Vec::new();
        let mut value = 0;
        for (position, delta) in &self.deltas_by_position {
            let next_value = value + delta;
            if (value == 0) != (next_value == 0) {
                positions.push(*position);
            }
            value = next_value;
        }
        RangeSet {positions}
    }
}
