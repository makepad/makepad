use {
    crate::{
        delta::{Delta, OperationSpan},
        size::Size,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    pub fn origin() -> Position {
        Position::default()
    }

    pub fn is_origin(self) -> bool {
        self.line == 0 && self.column == 0
    }

    pub fn apply_delta(&mut self, delta: &Delta) -> Position {
        let mut position = Position::origin();
        let mut distance = *self - Position::origin();
        let mut operation_span_iter = delta.iter().map(|operation| operation.span());
        let mut operation_span_slot = operation_span_iter.next();
        loop {
            match operation_span_slot {
                Some(OperationSpan::Retain(count)) => match distance.cmp(&count) {
                    Ordering::Less => {
                        break position + distance;
                    }
                    Ordering::Equal => {
                        position += distance;
                        distance = Size::zero();
                        operation_span_slot = operation_span_iter.next();
                    }
                    Ordering::Greater => {
                        position += count;
                        distance -= count;
                        operation_span_slot = operation_span_iter.next();
                    }
                },
                Some(OperationSpan::Insert(count)) => {
                    position += count;
                    operation_span_slot = operation_span_iter.next();
                }
                Some(OperationSpan::Delete(count)) => match distance.cmp(&count) {
                    Ordering::Less => {
                        break position + distance;
                    }
                    Ordering::Equal => {
                        break position;
                    }
                    Ordering::Greater => {
                        distance -= count;
                        operation_span_slot = operation_span_iter.next();
                    }
                },
                None => {
                    break position + distance;
                }
            }
        }
    }
}

impl Add<Size> for Position {
    type Output = Position;

    fn add(self, size: Size) -> Position {
        if size.line == 0 {
            Position {
                line: self.line,
                column: self.column + size.column,
            }
        } else {
            Position {
                line: self.line + size.line,
                column: size.column,
            }
        }
    }
}

impl AddAssign<Size> for Position {
    fn add_assign(&mut self, size: Size) {
        *self = *self + size;
    }
}

impl Sub for Position {
    type Output = Size;

    fn sub(self, other: Position) -> Size {
        if self.line == other.line {
            Size {
                line: 0,
                column: self.column - other.column,
            }
        } else {
            Size {
                line: self.line - other.line,
                column: self.column,
            }
        }
    }
}
