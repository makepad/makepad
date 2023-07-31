use {
    crate::{Diff, Len},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}

impl Pos {
    pub fn apply_diff(self, diff: &Diff, mode: ApplyDiffMode) -> Pos {
        use {crate::diff::OpInfo, std::cmp::Ordering};

        let mut diffed_pos = Pos::default();
        let mut offset_to_pos = self - Pos::default();
        let mut op_infos = diff.iter().map(|op| op.info());
        let mut op_info_op = op_infos.next();
        loop {
            match op_info_op {
                Some(OpInfo::Retain(length)) => match length.cmp(&offset_to_pos) {
                    Ordering::Less | Ordering::Equal => {
                        diffed_pos += length;
                        offset_to_pos -= length;
                        op_info_op = op_infos.next();
                    }
                    Ordering::Greater => break diffed_pos + offset_to_pos,
                },
                Some(OpInfo::Insert(length)) => {
                    if offset_to_pos == Len::default() {
                        break match mode {
                            ApplyDiffMode::InsertBefore => diffed_pos + length,
                            ApplyDiffMode::InsertAfter => diffed_pos,
                        };
                    } else {
                        diffed_pos += length;
                        op_info_op = op_infos.next();
                    }
                }
                Some(OpInfo::Delete(length)) => match length.cmp(&offset_to_pos) {
                    Ordering::Less | Ordering::Equal => {
                        offset_to_pos -= length;
                        op_info_op = op_infos.next();
                    }
                    Ordering::Greater => {
                        offset_to_pos = Len::default();
                        op_info_op = op_infos.next();
                    }
                },
                None => break diffed_pos + offset_to_pos,
            }
        }
    }
}

impl Add<Len> for Pos {
    type Output = Self;

    fn add(self, length: Len) -> Self::Output {
        if length.lines == 0 {
            Self {
                line: self.line,
                byte: self.byte + length.bytes,
            }
        } else {
            Self {
                line: self.line + length.lines,
                byte: length.bytes,
            }
        }
    }
}

impl AddAssign<Len> for Pos {
    fn add_assign(&mut self, length: Len) {
        *self = *self + length;
    }
}

impl Sub for Pos {
    type Output = Len;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Len {
                lines: 0,
                bytes: self.byte - other.byte,
            }
        } else {
            Len {
                lines: self.line - other.line,
                bytes: self.byte,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApplyDiffMode {
    InsertBefore,
    InsertAfter,
}
