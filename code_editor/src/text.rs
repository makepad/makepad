use {
    crate::{Diff, Len, Pos, Range},
    std::{borrow::Cow, ops::AddAssign},
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
            lines: self.lines.len() - 1,
            bytes: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line][range.start().byte..range.end().byte].to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines.push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
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
        for operation in diff {
            match operation {
                Op::Delete(len) => self.delete(pos, len),
                Op::Retain(len) => pos += len,
                Op::Insert(text) => {
                    let len = text.len();
                    self.insert(pos, text);
                    pos += len;
                }
            }
        }
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

impl From<char> for Text {
    fn from(char: char) -> Self {
        Self {
            lines: match char {
                '\n' => vec![String::new(), String::new()],
                _ => vec![char.into()],
            },
        }
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.split("\n").map(|line| line.to_owned()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
    }
}

impl From<&String> for Text {
    fn from(string: &String) -> Self {
        string.as_str().into()
    }
}

impl From<String> for Text {
    fn from(string: String) -> Self {
        string.as_str().into()
    }
}

impl From<Cow<'_, str>> for Text {
    fn from(string: Cow<'_, str>) -> Self {
        string.as_ref().into()
    }
}
