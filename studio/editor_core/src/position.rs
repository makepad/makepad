use {
    crate::makepad_micro_serde::*,
    crate::{
        delta::{Delta, OperationSpan},
        size::Size,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
};

/// A type for representing a position in a text.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

impl Position {
    /// Returns the position of the start of a text.
    /// 
    /// ```
    /// use makepad_editor_core::Position;
    /// 
    /// let position = Position::origin();
    /// ```
    pub fn origin() -> Position {
        Position::default()
    }

    /// Returns `true` if this is the position of the start of a text.
    /// 
    /// ```
    /// use makepad_editor_core::Position;
    /// 
    /// let position = Position::origin();
    /// assert!(position.is_origin());
    /// let position = Position { line: 1, column: 1};
    /// assert!(!position.is_origin());
    /// ```
    pub fn is_origin(self) -> bool {
        self.line == 0 && self.column == 0
    }

    /// Applies the given delta to this position.
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
 
    /// Returns the sum of this position and the given size. The result represents the position
    /// obtained by moving this position forward by the amount of text corresponding to the given
    /// size.
    fn add(self, other: Size) -> Position {
        if other.line == 0 {
            Position {
                line: self.line,
                column: self.column + other.column as usize,
            }
        } else {
            Position {
                line: self.line + other.line as usize,
                column: other.column as usize,
            }
        }
    }
}

impl AddAssign<Size> for Position {
    /// Adds the given size to this position. This moves this position forward by the amount of text
    /// corresponding to the given size.
    fn add_assign(&mut self, size: Size) {
        *self = *self + size;
    }
}

impl Sub for Position {
    type Output = Size;

    /// Returns the difference between this position and the given position. The result represents
    /// the amount of the text between this position and the given position.
    fn sub(self, other: Position) -> Size {
        if self.line == other.line {
            Size {
                line: 0,
                column: self.column as u32 - other.column as u32,
            }
        } else {
            Size {
                line: self.line as u32 - other.line as u32,
                column: self.column as u32,
            }
        }
    }
}