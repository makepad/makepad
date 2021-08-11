use {
    serde::{Deserialize, Serialize},
    std::ops::{Add, AddAssign, Sub, SubAssign},
};

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct Size {
    pub line: usize,
    pub column: usize,
}

impl Size {
    pub fn zero() -> Size {
        Size::default()
    }

    pub fn is_zero(self) -> bool {
        self.line == 0 && self.column == 0
    }
}

impl Add for Size {
    type Output = Size;

    fn add(self, other: Size) -> Size {
        if other.line == 0 {
            Size {
                line: self.line,
                column: self.column + other.column,
            }
        } else {
            Size {
                line: self.line + other.line,
                column: other.column,
            }
        }
    }
}

impl AddAssign for Size {
    fn add_assign(&mut self, other: Size) {
        *self = *self + other;
    }
}

impl Sub for Size {
    type Output = Size;

    fn sub(self, other: Size) -> Size {
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

impl SubAssign for Size {
    fn sub_assign(&mut self, other: Size) {
        *self = *self - other;
    }
}
