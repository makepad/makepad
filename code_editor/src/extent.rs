use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
            Self {
                line_count: 0,
                byte_count: self.byte_count - other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count - other.line_count,
                byte_count: self.byte_count,
            }
        }
    }
}

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
