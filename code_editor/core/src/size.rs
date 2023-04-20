use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Size {
    pub line: usize,
    pub byte: usize,
}

impl Add for Size {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        if other.line == 0 {
            Self {
                line: self.line,
                byte: self.byte + other.byte,
            }
        } else {
            Self {
                line: self.line + other.line,
                byte: other.byte,
            }
        }
    }
}

impl AddAssign<Size> for Size {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Size {
    type Output = Size;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Self {
                line: 0,
                byte: other.byte - self.byte,
            }
        } else {
            Self {
                line: other.line - self.line,
                byte: other.byte,
            }
        }
    }
}

impl SubAssign<Size> for Size {
    fn sub_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}
