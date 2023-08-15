use {
    crate::{change, Change, Extent, Point, Range},
    std::{fmt, io, io::BufRead, iter},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn newline() -> Self {
        Self {
            lines: vec![String::new(), String::new()],
        }
    }

    pub fn from_buf_reader<R>(reader: R) -> io::Result<Self>
    where
        R: BufRead,
    {
        Ok(Self {
            lines: reader.lines().collect::<Result<_, _>>()?,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.extent() == Extent::zero()
    }

    pub fn extent(&self) -> Extent {
        Extent {
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

    pub fn insert(&mut self, point: Point, mut text: Self) {
        if text.extent().line_count == 0 {
            self.lines[point.line]
                .replace_range(point.byte..point.byte, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line][..point.byte]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line][point.byte..]);
            self.lines.splice(point.line..point.line + 1, text.lines);
        }
    }

    pub fn delete(&mut self, range: Range) {
        if range.start().line == range.end().line {
            self.lines[range.start().line].replace_range(range.start().byte..range.end().byte, "");
        } else {
            let mut line = self.lines[range.start().line][..range.start().byte].to_string();
            line.push_str(&self.lines[range.end().line][range.end().byte..]);
            self.lines
                .splice(range.start().line..range.end().line + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change.kind {
            change::ChangeKind::Insert(point, additional_text) => {
                self.insert(point, additional_text)
            }
            change::ChangeKind::Delete(range) => self.delete(range),
        }
    }

    pub fn into_line_count(self) -> Vec<String> {
        self.lines
    }
}

impl Default for Text {
    fn default() -> Self {
        Self {
            lines: vec![String::new()],
        }
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (last_line, remaining_lines) = self.lines.split_last().unwrap();
        for line in remaining_lines {
            writeln!(f, "{}", line)?;
        }
        write!(f, "{}", last_line)
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Self {
        Self {
            lines: string.lines().map(|string| string.to_owned()).collect(),
        }
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
