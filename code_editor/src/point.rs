use {
    crate::Extent,
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}
