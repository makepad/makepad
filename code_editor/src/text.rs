use {
    crate::{Length, Position, Range},
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
        if range.start().line == range.end().line {
            lines.push(
                self.lines[range.start().line]
                    [range.start().byte..range.end().byte]
                    .to_string(),
            );
        } else {
            lines.reserve(range.end().line - range.start().line + 1);
            lines
                .push(self.lines[range.start().line][range.start().byte..].to_string());
            lines.extend(
                self.lines[range.start().line + 1..range.end().line]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[range.end().line][..range.end().byte].to_string());
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
            self.lines[position.line].replace_range(
                position.byte..position.byte,
                text.lines.first().unwrap(),
            );
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[position.line][..position.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[position.line][position.byte..]);
            self.lines
                .splice(position.line..position.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, position: Position, length: Length) {
        use std::iter;

        if length.line_count == 0 {
            self.lines[position.line].replace_range(
                position.byte..position.byte + length.byte_count,
                "",
            );
        } else {
            let mut line = self.lines[position.line][..position.byte].to_string();
            line.push_str(
                &self.lines[position.line + length.line_count][length.byte_count..],
            );
            self.lines.splice(
                position.line..position.line + length.line_count + 1,
                iter::once(line),
            );
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
        let mut lines: Vec<_> = string.split('\n').map(|line| line.to_string()).collect();
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