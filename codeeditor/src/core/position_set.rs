use {
    crate::core::{
        delta::{Operation, OperationSizeOnly},
        Delta, Position, Size,
    },
    std::{cmp::Ordering, ops::Deref, slice::Iter},
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

    pub fn transform<'a, 'b>(&'a self, delta: &'b Delta) -> Transform<'a, 'b> {
        let mut distance_iter = self.distances();
        let mut operation_iter = delta.iter();
        let distance_slot = distance_iter.next();
        let operation_slot = operation_iter.next().map(|operation| operation.size_only());
        Transform {
            distance_iter,
            operation_iter,
            distance_slot,
            operation_slot,
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

#[derive(Debug)]
pub struct Distances<'a> {
    next_position_iter: Iter<'a, Position>,
    position: Position,
}

impl<'a> Iterator for Distances<'a> {
    type Item = Size;

    fn next(&mut self) -> Option<Self::Item> {
        let next_position = *self.next_position_iter.next()?;
        let distance = next_position - self.position;
        self.position = next_position;
        Some(distance)
    }
}

#[derive(Debug)]
pub struct Transform<'a, 'b> {
    distance_iter: Distances<'a>,
    operation_iter: Iter<'b, Operation>,
    distance_slot: Option<Size>,
    operation_slot: Option<OperationSizeOnly>,
    position: Position,
}

impl<'a, 'b> Iterator for Transform<'a, 'b> {
    type Item = Position;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match (self.distance_slot, self.operation_slot) {
                (Some(distance), Some(OperationSizeOnly::Retain(count))) => {
                    match distance.cmp(&count) {
                        Ordering::Less => {
                            self.distance_slot = self.distance_iter.next();
                            self.operation_slot = Some(OperationSizeOnly::Retain(count - distance));
                            self.position += distance;
                            break Some(self.position);
                        }
                        Ordering::Equal => {
                            self.distance_slot = Some(Size::zero());
                            self.operation_slot = self
                                .operation_iter
                                .next()
                                .map(|operation| operation.size_only());
                            self.position += distance;
                        }
                        Ordering::Greater => {
                            self.distance_slot = Some(distance - count);
                            self.operation_slot = self
                                .operation_iter
                                .next()
                                .map(|operation| operation.size_only());
                            self.position += count;
                        }
                    }
                }
                (Some(distance), Some(OperationSizeOnly::Insert(len))) => {
                    self.distance_slot = Some(distance);
                    self.operation_slot = self
                        .operation_iter
                        .next()
                        .map(|operation| operation.size_only());
                    self.position += len;
                }
                (Some(distance), Some(OperationSizeOnly::Delete(count))) => {
                    match distance.cmp(&count) {
                        Ordering::Less => {
                            self.distance_slot = self.distance_iter.next();
                            self.operation_slot = Some(OperationSizeOnly::Retain(count - distance));
                            break Some(self.position);
                        }
                        Ordering::Equal => {
                            self.distance_slot = self.distance_iter.next();
                            self.operation_slot = self
                                .operation_iter
                                .next()
                                .map(|operation| operation.size_only());
                            break Some(self.position);
                        }
                        Ordering::Greater => {
                            self.distance_slot = Some(distance - count);
                            self.operation_slot = self
                                .operation_iter
                                .next()
                                .map(|operation| operation.size_only());
                        }
                    }
                }
                (Some(distance), None) => {
                    self.distance_slot = self.distance_iter.next();
                    self.operation_slot = None;
                    self.position += distance;
                    break Some(self.position);
                }
                (None, _) => break None,
            }
        }
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
