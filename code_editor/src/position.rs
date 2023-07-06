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

        let mut position = Position::default();
        let mut remaining_length = self - Position::default();
        let mut operation_infos = diff.iter().map(|operation| operation.info());
        let mut operation_info_slot = operation_infos.next();
        loop {
            match operation_info_slot {
                Some(OperationInfo::Retain(length)) => match length.cmp(&remaining_length) {
                    Ordering::Less | Ordering::Equal => {
                        position += length;
                        remaining_length -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break position + remaining_length;
                    }
                },
                Some(OperationInfo::Insert(length)) => match strategy {
                    Strategy::InsertBefore => {
                        break position + length;
                    }
                    Strategy::InsertAfter => {
                        operation_info_slot = operation_infos.next();
                    }
                },
                Some(OperationInfo::Delete(length)) => match length.cmp(&remaining_length) {
                    Ordering::Less | Ordering::Equal => {
                        remaining_length -= length;
                        operation_info_slot = operation_infos.next();
                    }
                    Ordering::Greater => {
                        break position;
                    }
                },
                None => {
                    break position + remaining_length;
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
