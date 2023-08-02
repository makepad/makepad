use {
    crate::{Diff, TextLen},
    std::ops::{Add, AddAssign, Sub},
};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextPos {
    pub line: usize,
    pub byte: usize,
}

impl TextPos {
    pub fn is_at_first_line(self) -> bool {
        self.line == 0
    }

    pub fn is_at_last_line(self, line_count: usize) -> bool {
        self.line == line_count
    }

    pub fn is_at_start_of_line(self) -> bool {
        self.byte == 0
    }

    pub fn is_at_end_of_line(self, lines: &[String]) -> bool {
        self.byte == lines[self.line].len()
    }

    pub fn apply_diff(self, diff: &Diff, mode: ApplyDiffMode) -> TextPos {
        use {crate::text_diff::OpInfo, std::cmp::Ordering};

        let mut diffed_pos = TextPos::default();
        let mut offset_to_pos = self - TextPos::default();
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
                    if offset_to_pos == TextLen::default() {
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
                        offset_to_pos = TextLen::default();
                        op_info_op = op_infos.next();
                    }
                },
                None => break diffed_pos + offset_to_pos,
            }
        }
    }
}

impl Add<TextLen> for TextPos {
    type Output = Self;

    fn add(self, length: TextLen) -> Self::Output {
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

impl AddAssign<TextLen> for TextPos {
    fn add_assign(&mut self, length: TextLen) {
        *self = *self + length;
    }
}

impl Sub for TextPos {
    type Output = TextLen;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            TextLen {
                lines: 0,
                bytes: self.byte - other.byte,
            }
        } else {
            TextLen {
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
