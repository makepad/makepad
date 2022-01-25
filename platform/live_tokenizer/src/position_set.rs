use {
    crate::{
        position::Position,
        size::Size
    },
    std::{ops::Deref, slice::Iter},
};

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct PositionSet {
    positions: Vec<Position>,
}

impl PositionSet {
    pub fn new() -> PositionSet {
        PositionSet::default()
    }
    
    pub fn distances(&self) -> Distances {
        Distances {
            next_position_iter: self.positions.iter(),
            position: Position::origin(),
        }
    }
}

impl Deref for PositionSet {
    type Target = [Position];
    
    fn deref(&self) -> &Self::Target {
        &self.positions
    }
}

impl<'a> IntoIterator for &'a PositionSet {
    type Item = &'a Position;
    type IntoIter = Iter<'a, Position>;
    
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Default, Debug)]
pub struct Builder {
    positions: Vec<Position>,
}

impl Builder {
    pub fn new() -> Builder {
        Builder::default()
    }
    
    pub fn insert(&mut self, position: Position) {
        self.positions.push(position);
    }
    
    pub fn build(mut self) -> PositionSet {
        self.positions.sort();
        self.positions.dedup();
        PositionSet {
            positions: self.positions,
        }
    }
}

#[derive(Debug)]
pub struct Distances<'a> {
    next_position_iter: Iter<'a, Position>,
    position: Position,
}

impl<'a> Iterator for Distances<'a> {
    type Item = Size;
    
    fn next(&mut self) -> Option<Self::Item> {
        let next_position = *self.next_position_iter.next() ?;
        let distance = next_position - self.position;
        self.position = next_position;
        Some(distance)
    }
}
