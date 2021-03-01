use {
    crate::core::Size,
    std::ops::{Add, AddAssign, Sub},
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
