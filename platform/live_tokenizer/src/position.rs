use {
    makepad_micro_serde::*,
    crate::{
        delta::{Delta, OperationSpan},
        size::Size,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
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
        let new_position = loop {
            match operation_span_slot {
                Some(OperationSpan::Retain(count)) => match count.cmp(&distance) {
                    Ordering::Less => {
                        position += count;
                        distance -= count;
                        operation_span_slot = operation_span_iter.next();
                    }
                    Ordering::Equal | Ordering::Greater => {
                        break position + distance;
                    }
                },
                Some(OperationSpan::Insert(count)) => {
                    position += count;
                    operation_span_slot = operation_span_iter.next();
                }
                Some(OperationSpan::Delete(count)) => match count.cmp(&distance) {
                    Ordering::Less => {
                        distance -= count;
                        operation_span_slot = operation_span_iter.next();
                    }
                    Ordering::Equal | Ordering::Greater => {
                        break position;
                    }
                },
                None => {
                    break position + distance;
                }
            }
        };
        new_position
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
/*
impl From<TextPos> for Position {
    fn from(text_pos: TextPos) -> Position {
        Position{
            line:text_pos.line as usize,
            column:text_pos.column as usize
        }
    }
}*/
