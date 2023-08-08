use {
    crate::{change, Change, Extent, Point, Range},
    std::{io, io::BufRead},
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Text {
    lines: Vec<String>,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
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

    pub fn insert(&mut self, point: Point, mut additional_text: Self) {
        if additional_text.extent().line_count == 0 {
            self.lines[point.line_index].replace_range(
                point.byte_index..point.byte_index,
                additional_text.lines.first().unwrap(),
            );
        } else {
            additional_text
                .lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line_index][..point.byte_index]);
            additional_text
                .lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line_index][point.byte_index..]);
            self.lines.splice(
                point.line_index..point.line_index + 1,
                additional_text.lines,
            );
        }
    }

    pub fn delete(&mut self, range: Range) {
        use std::iter;

        if range.start().line_index == range.end().line_index {
            self.lines[range.start().line_index]
                .replace_range(range.start().byte_index..range.end().byte_index, "");
        } else {
            let mut line =
                self.lines[range.start().line_index][..range.start().byte_index].to_string();
            line.push_str(&self.lines[range.end().line_index][range.end().byte_index..]);
            self.lines.splice(
                range.start().line_index..range.end().line_index + 1,
                iter::once(line),
            );
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
