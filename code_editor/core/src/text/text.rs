use {
    super::{Diff, Len, Pos, Range},
    std::ops::AddAssign,
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
            line: self.lines.len() - 1,
            byte: self.lines.last().unwrap().len(),
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
        let mut lines = self.lines.drain(..len.line as usize).collect::<Vec<_>>();
        lines.push(self.lines.first().unwrap()[..len.byte].to_string());
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte, "");
        Text { lines }
    }

    pub fn skip(&mut self, len: Len) {
        self.lines.drain(..len.line);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte, "");
    }

    pub fn insert(&mut self, pos: Pos, mut text: Self) {
        if text.len().line == 0 {
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

        if len.line == 0 {
            self.lines[pos.line].replace_range(pos.byte..pos.byte + len.byte, "");
        } else {
            let mut line = self.lines[pos.line][..pos.byte].to_string();
            line.push_str(&self.lines[pos.line + len.line][len.byte..]);
            self.lines
                .splice(pos.line..pos.line + len.line + 1, iter::once(line));
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

impl<const N: usize> From<[String; N]> for Text {
    fn from(lines: [String; N]) -> Self {
        lines.to_vec().into()
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        string.split("\n").map(|string| string.into()).collect()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
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
