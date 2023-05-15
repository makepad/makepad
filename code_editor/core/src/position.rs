use {
    super::{DeltaLen, Size},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line: usize,
    pub byte: usize,
}

impl Position {
    pub fn apply_delta(self, delta_len: DeltaLen) -> Self {
        if self < delta_len.range.start() {
            self
        } else {
            delta_len.range.start()
                + delta_len.replace_with_len
                + (self.max(delta_len.range.end()) - delta_len.range.end())
        }
    }
}

impl Add<Size> for Position {
    type Output = Self;

    fn add(self, size: Size) -> Self::Output {
        if size.line == 0 {
            Self {
                line: self.line,
                byte: self.byte + size.byte,
            }
        } else {
            Self {
                line: self.line + size.line,
                byte: size.byte,
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

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Size {
                line: 0,
                byte: other.byte - self.byte,
            }
        } else {
            Size {
                line: other.line - self.line,
                byte: other.byte,
            }
        }
    }
}
