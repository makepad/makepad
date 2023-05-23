use {
    crate::Diff,
    std::ops::{Add, AddAssign, Sub, SubAssign},
};

#[derive(Clone, Debug, Eq, PartialEq)]
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
            lines: self.lines.len() - 1,
            bytes: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn get(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start.line == range.end.line {
            lines.push(self.lines[range.start.line][range.start.byte..range.end.byte].to_string());
        } else {
            lines.reserve(range.end.line - range.start.line + 1);
            lines.push(self.lines[range.start.line][range.start.byte..].to_string());
            lines.extend(
                self.lines[range.start.line + 1..range.end.line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end.line][..range.end.byte].to_string());
        }
        Text { lines }
    }

    pub fn take(&mut self, len: Len) -> Self {
        let mut lines = self.lines.drain(..len.lines as usize).collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.bytes].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.bytes, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Len) {
        self.lines.drain(..len.lines);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.bytes, "");
    }

    pub fn insert(&mut self, pos: Pos, mut text: Self) {
        if text.len().lines == 0 {
            self.lines[pos.line].replace_range(pos.byte..pos.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[pos.line][..pos.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[pos.line][pos.byte..]);
            self.lines.splice(pos.line..pos.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, pos: Pos, len: Len) {
        use std::iter;

        if len.lines == 0 {
            self.lines[pos.line].replace_range(pos.byte..pos.byte + len.bytes, "");
        } else {
            let mut line = self.lines[pos.line][..pos.byte].to_string();
            line.push_str(&self.lines[pos.line + len.lines][len.bytes..]);
            self.lines
                .splice(pos.line..pos.line + len.lines + 1, iter::once(line));
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Pos {
    pub line: usize,
    pub byte: usize,
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
        if len.lines == 0 {
            Self {
                line: self.line,
                byte: self.byte + len.bytes,
            }
        } else {
            Self {
                line: self.line + len.lines,
                byte: len.bytes,
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Len {
    pub lines: usize,
    pub bytes: usize,
}

impl Add for Len {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if other.lines == 0 {
            Self {
                lines: self.lines,
                bytes: self.lines + other.lines,
            }
        } else {
            Self {
                lines: self.lines + other.lines,
                bytes: other.lines,
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
        if self.lines - other.lines == 0 {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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