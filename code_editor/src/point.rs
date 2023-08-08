use {
    crate::{
        change::{ChangeKind, Drift},
        Change, Extent,
    },
    std::{
        cmp::Ordering,
        ops::{Add, AddAssign, Sub},
    },
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

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref additional_text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + additional_text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => self + additional_text.extent(),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
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
