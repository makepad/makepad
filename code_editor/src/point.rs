use {
    crate::{Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn apply_diff(self, diff: &Diff, mode: ApplyDiffMode) -> Point {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_point = Point::default();
        let mut distance_to_point = self - Point::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_point) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_point += length;
                        distance_to_point -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_point + distance_to_point;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_point == Length::default() {
                        break match mode {
                            ApplyDiffMode::InsertBefore => diffed_point + length,
                            ApplyDiffMode::InsertAfter => diffed_point,
                        };
                    } else {
                        diffed_point += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_point) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_point -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_point = Length::default();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_point + distance_to_point;
                }
            }
        }
    }
}

impl Add<Length> for Point {
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

impl AddAssign<Length> for Point {
    fn add_assign(&mut self, length: Length) {
        *self = *self + length;
    }
}

impl Sub for Point {
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApplyDiffMode {
    InsertBefore,
    InsertAfter,
}
