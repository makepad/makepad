use {
    crate::{Diff, Len},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
}

impl Pos {
    pub fn apply_diff(self, diff: &Diff, after: bool) -> Pos {
        use {crate::diff::LenOnlyOp, std::cmp::Ordering};

        let mut pos = Pos::default();
        let mut rem_len = self - Pos::default();
        let mut op_iter = diff.iter().map(|operation| operation.len_only());
        let mut op_slot = op_iter.next();
        loop {
            match op_slot {
                Some(LenOnlyOp::Retain(len)) => match len.cmp(&rem_len) {
                    Ordering::Less | Ordering::Equal => {
                        pos += len;
                        rem_len -= len;
                        op_slot = op_iter.next();
                    }
                    Ordering::Greater => {
                        break pos + rem_len;
                    }
                },
                Some(LenOnlyOp::Insert(len)) => {
                    if after {
                        pos += len;
                    }
                    op_slot = op_iter.next();
                }
                Some(LenOnlyOp::Delete(len)) => match len.cmp(&rem_len) {
                    Ordering::Less | Ordering::Equal => {
                        rem_len -= len;
                        op_slot = op_iter.next();
                    }
                    Ordering::Greater => {
                        break pos;
                    }
                },
                None => {
                    break pos + rem_len;
                }
            }
        }
    }
}

impl Add<Len> for Pos {
    type Output = Self;

    fn add(self, len: Len) -> Self::Output {
        if len.line == 0 {
            Self {
                line: self.line,
                byte: self.byte + len.byte,
            }
        } else {
            Self {
                line: self.line + len.line,
                byte: len.byte,
            }
        }
    }
}

impl AddAssign<Len> for Pos {
    fn add_assign(&mut self, len: Len) {
        *self = *self + len;
    }
}

impl Sub for Pos {
    type Output = Len;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Len {
                line: 0,
                byte: other.byte - self.byte,
            }
        } else {
            Len {
                line: other.line - self.line,
                byte: other.byte,
            }
        }
    }
}
