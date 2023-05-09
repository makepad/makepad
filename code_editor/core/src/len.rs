use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Len {
    pub line: usize,
    pub byte: usize,
}

impl Add for Len {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if other.line == 0 {
            Self {
                line: self.line,
                byte: self.line + other.line,
            }
        } else {
            Self {
                line: self.line + other.line,
                byte: other.line,
            }
        }
    }
}

impl AddAssign for Len {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Len {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        if self.line - other.line == 0 {
            Self {
                line: 0,
                byte: self.byte - other.byte,
            }
        } else {
            Self {
                line: self.line - other.line,
                byte: self.byte,
            }
        }
    }
}

impl SubAssign for Len {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
