use std::ops::{Add, AddAssign, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Len {
    pub lines: usize,
    pub bytes: usize,
}

impl Add for Len {
    type Output = Len;

    fn add(self, other: Self) -> Self::Output {
        if other.lines == 0 {
            Self {
                lines: self.lines,
                bytes: self.bytes + other.bytes,
            }
        } else {
            Self {
                lines: self.lines + other.lines,
                bytes: other.bytes,
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
    type Output = Len;

    fn sub(self, other: Self) -> Self::Output {
        if self.lines == other.lines {
            Self {
                lines: 0,
                bytes: self.bytes - other.bytes,
            }
        } else {
            Self {
                lines: self.lines - other.lines,
                bytes: self.bytes,
            }
        }
    }
}

impl SubAssign for Len {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}
