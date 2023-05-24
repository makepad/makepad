use {
    crate::Diff,
    std::ops::{Add, AddAssign, Sub, SubAssign},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == Len::default()
    }

    pub fn len(&self) -> Len {
        Len {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn get(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start.line_index == range.end.line_index {
            lines.push(
                self.lines[range.start.line_index][range.start.byte_index..range.end.byte_index]
                    .to_string(),
            );
        } else {
            lines.reserve(range.end.line_index - range.start.line_index + 1);
            lines.push(self.lines[range.start.line_index][range.start.byte_index..].to_string());
            lines.extend(
                self.lines[range.start.line_index + 1..range.end.line_index]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end.line_index][..range.end.byte_index].to_string());
        }
        Text { lines }
    }

    pub fn take(&mut self, len: Len) -> Self {
        let mut lines = self
            .lines
            .drain(..len.line_count as usize)
            .collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte_count].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Len) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, pos: Pos, mut text: Self) {
        if text.len().line_count == 0 {
            self.lines[pos.line_index]
                .replace_range(pos.byte_index..pos.byte_index, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[pos.line_index][..pos.byte_index]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[pos.line_index][pos.byte_index..]);
            self.lines
                .splice(pos.line_index..pos.line_index + 1, text.lines);
        }
    }

    pub fn delete(&mut self, pos: Pos, len: Len) {
        use std::iter;

        if len.line_count == 0 {
            self.lines[pos.line_index]
                .replace_range(pos.byte_index..pos.byte_index + len.byte_count, "");
        } else {
            let mut line = self.lines[pos.line_index][..pos.byte_index].to_string();
            line.push_str(&self.lines[pos.line_index + len.line_count][len.byte_count..]);
            self.lines.splice(
                pos.line_index..pos.line_index + len.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Op;

        let mut pos = Pos::default();
        for op in diff {
            match op {
                Op::Retain(len) => pos += len,
                Op::Insert(text) => {
                    let len = text.len();
                    self.insert(pos, text);
                    pos += len;
                }
                Op::Delete(text) => self.delete(pos, text.len()),
            }
        }
    }

    pub fn into_lines(self) -> Vec<String> {
        self.lines
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, mut other: Self) {
        other
            .lines
            .first_mut()
            .unwrap()
            .replace_range(..0, self.lines.last().unwrap());
        self.lines
            .splice(self.lines.len() - 1..self.lines.len(), other.lines);
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl From<Vec<String>> for Text {
    fn from(mut lines: Vec<String>) -> Self {
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
    }
}

impl FromIterator<String> for Text {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        iter.into_iter().collect::<Vec<_>>().into()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Pos {
    pub line_index: usize,
    pub byte_index: usize,
}

impl Pos {
    pub fn apply_diff(self, diff: &Diff, after: bool) -> Pos {
        use {super::diff::LenOnlyOp, std::cmp::Ordering};

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
                        break pos + len;
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
        if len.line_count == 0 {
            Self {
                line_index: self.line_index,
                byte_index: self.byte_index + len.byte_count,
            }
        } else {
            Self {
                line_index: self.line_index + len.line_count,
                byte_index: len.byte_count,
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
        if self.line_index == other.line_index {
            Len {
                line_count: 0,
                byte_count: self.byte_index - other.byte_index,
            }
        } else {
            Len {
                line_count: self.line_index - other.line_index,
                byte_count: self.byte_index,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, PartialOrd, Ord)]
pub struct Len {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Add for Len {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.line_count + other.line_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.line_count,
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
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        if self.line_count - other.line_count == 0 {
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

impl SubAssign for Len {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

impl Range {
    pub fn is_empty(&self) -> bool {
        self.len() == Len::default()
    }

    pub fn len(&self) -> Len {
        self.end - self.start
    }
}
