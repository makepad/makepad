use {crate::Length, std::ops::{Add, AddAssign, Sub}};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn new(line: usize, byte: usize) -> Self {
        Self { line, byte }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.byte_count,
            }
        } else {
            Self {
                line: self.line + length.line_count,
                byte: length.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Length {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Length {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}