use {
    crate::{Diff, Length, Position, Range},
    std::ops::AddAssign,
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
        self.length() == Length::default()
    }

    pub fn length(&self) -> Length {
        Length {
            line_count: self.lines.len() - 1,
            byte_count: self.lines.last().unwrap().len(),
        }
    }

    pub fn as_lines(&self) -> &[String] {
        &self.lines
    }

    pub fn slice(&self, range: Range) -> Self {
        let mut lines = Vec::new();
        if range.start().line_index == range.end().line_index {
            lines.push(
                self.lines[range.start().line_index]
                    [range.start().byte_index..range.end().byte_index]
                    .to_string(),
            );
        } else {
            lines.reserve(range.end().line_index - range.start().line_index + 1);
            lines
                .push(self.lines[range.start().line_index][range.start().byte_index..].to_string());
            lines.extend(
                self.lines[range.start().line_index + 1..range.end().line_index]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line_index][..range.end().byte_index].to_string());
        }
        Text { lines }
    }

    pub fn take(&mut self, len: Length) -> Self {
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

    pub fn skip(&mut self, len: Length) {
        self.lines.drain(..len.line_count);
        self.lines
            .first_mut()
            .unwrap()
            .replace_range(..len.byte_count, "");
    }

    pub fn insert(&mut self, position: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[position.line_index].replace_range(
                position.byte_index..position.byte_index,
                text.lines.first().unwrap(),
            );
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line_index][..position.byte_index]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line_index][position.byte_index..]);
            self.lines
                .splice(position.line_index..position.line_index + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line_index].replace_range(
                position.byte_index..position.byte_index + length.byte_count,
                "",
            );
        } else {
            let mut line = self.lines[position.line_index][..position.byte_index].to_string();
            line.push_str(
                &self.lines[position.line_index + length.line_count][length.byte_count..],
            );
            self.lines.splice(
                position.line_index..position.line_index + length.line_count + 1,
                iter::once(line),
            );
        }
    }

    pub fn apply_diff(&mut self, diff: Diff) {
        use super::diff::Operation;

        let mut position = Position::default();
        for operation in diff {
            match operation {
                Operation::Delete(length) => self.delete(position, length),
                Operation::Retain(length) => position += length,
                Operation::Insert(text) => {
                    let length = text.length();
                    self.insert(position, text);
                    position += length;
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

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        let mut lines: Vec<_> = string.lines().map(|line| line.to_string()).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self { lines }
    }
}
