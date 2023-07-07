use {
    crate::{diff::Strategy, Diff, Length},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line_index: usize,
    pub byte_index: usize,
}

impl Position {
    pub fn new(line_index: usize, byte_index: usize) -> Self {
        Self {
            line_index,
            byte_index,
        }
    }

    pub fn origin() -> Self {
        Self::default()
    }

    pub fn apply_diff(self, diff: &Diff, strategy: Strategy) -> Position {
        use {crate::diff::OperationInfo, std::cmp::Ordering};

        let mut diffed_position = Position::origin();
        let mut distance_to_position = self - Position::origin();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_position += length;
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break diffed_position + distance_to_position;
                    }
                },
                Some(OperationInfo::Insert(length)) => {
                    if distance_to_position == Length::zero() {
                        break match strategy {
                            Strategy::InsertBefore => diffed_position + length,
                            Strategy::InsertAfter => diffed_position,
                        };
                    } else {
                        diffed_position += length;
                        operation_info_slot = operation_infos.next();
                    }
                }
                Some(OperationInfo::Delete(length)) => match length.cmp(&distance_to_position) {
                    Ordering::Less | Ordering::Equal => {
                        distance_to_position -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        distance_to_position = Length::zero();
                        operation_info_slot = operation_infos.next();
                    }
                },
                None => {
                    break diffed_position + distance_to_position;
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, length: Length) -> Self::Output {
        if length.line_count == 0 {
            Self {
                line_index: self.line_index,
                byte_index: self.byte_index + length.byte_count,
            }
        } else {
            Self {
                line_index: self.line_index + length.line_count,
                byte_index: length.byte_count,
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
        if self.line_index == other.line_index {
            Length {
                line_count: 0,
                byte_count: self.byte_index - other.byte_index,
            }
        } else {
            Length {
                line_count: self.line_index - other.line_index,
                byte_count: self.byte_index,
            }
        }
    }
}
