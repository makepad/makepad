use std::{
    cmp::Ordering,
    fmt, io,
    io::BufRead,
    iter,
    ops::{Add, AddAssign, Sub, SubAssign},
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
        self.length() == Length::zero()
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

    pub fn slice(&self, start: Position, length: Length) -> Self {
        let end = start + length;
        let mut lines = Vec::new();
        if start.line_index == end.line_index {
            lines.push(
                self.lines[start.line_index][start.byte_index..end.byte_index].to_string(),
            );
        } else {
            lines.reserve(end.line_index - start.line_index + 1);
            lines.push(self.lines[start.line_index][start.byte_index..].to_string());
            lines.extend(
                self.lines[start.line_index + 1..end.line_index]
                    .iter()
                    .cloned(),
            );
            lines.push(self.lines[end.line_index][..end.byte_index].to_string());
        }
        Text { lines }
    }

    pub fn insert(&mut self, point: Position, mut text: Self) {
        if text.length().line_count == 0 {
            self.lines[point.line_index]
                .replace_range(point.byte_index..point.byte_index, text.lines.first().unwrap());
        } else {
            text.lines
                .first_mut()
                .unwrap()
                .replace_range(..0, &self.lines[point.line_index][..point.byte_index]);
            text.lines
                .last_mut()
                .unwrap()
                .push_str(&self.lines[point.line_index][point.byte_index..]);
            self.lines.splice(point.line_index..point.line_index + 1, text.lines);
        }
    }

    pub fn delete(&mut self, start: Position, length: Length) {
        let end = start + length;
        if start.line_index == end.line_index {
            self.lines[start.line_index].replace_range(start.byte_index..end.byte_index, "");
        } else {
            let mut line = self.lines[start.line_index][..start.byte_index].to_string();
            line.push_str(&self.lines[end.line_index][end.byte_index..]);
            self.lines
                .splice(start.line_index..end.line_index + 1, iter::once(line));
        }
    }

    pub fn apply_change(&mut self, change: Change) {
        match change {
            Change::Insert(position, text) => self.insert(position, text),
            Change::Delete(start, length) => self.delete(start, length),
        }
    }

    pub fn into_lines(self) -> Vec<String> {
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position {
    pub line_index: usize,
    pub byte_index: usize,
}

impl Position {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change, drift: Drift) -> Self {
        match *change {
            Change::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match drift {
                    Drift::Before => point + text.length() + (self - point),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.length() + (self - point),
            },
            Change::Delete(start, length) => {
                let end = start + length;
                if self < start {
                    self
                } else {
                    start + (self - end.min(self))
                }
            }
        }
    }
}

impl Add<Length> for Position {
    type Output = Self;

    fn add(self, extent: Length) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line_index: self.line_index,
                byte_index: self.byte_index + extent.byte_count,
            }
        } else {
            Self {
                line_index: self.line_index + extent.line_count,
                byte_index: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Length> for Position {
    fn add_assign(&mut self, extent: Length) {
        *self = *self + extent;
    }
}

impl Sub for Position {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_index == other.line_index {
            Length {
                line_count: 0,
                byte_count: self.byte_index - other.byte_index,
            }
        } else {
            Length {
                line_count: self.line_index - other.line_index,
                byte_count: self.byte_index,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Length {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Length {
    pub fn zero() -> Length {
        Self::default()
    }
}

impl Add for Length {
    type Output = Length;

    fn add(self, other: Self) -> Self::Output {
        if other.line_count == 0 {
            Self {
                line_count: self.line_count,
                byte_count: self.byte_count + other.byte_count,
            }
        } else {
            Self {
                line_count: self.line_count + other.line_count,
                byte_count: other.byte_count,
            }
        }
    }
}

impl AddAssign for Length {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Length {
    type Output = Length;

    fn sub(self, other: Self) -> Self::Output {
        if self.line_count == other.line_count {
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

impl SubAssign for Length {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Position,
    end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Position, extent: Length) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Position {
        self.start
    }

    pub fn end(self) -> Position {
        self.end
    }

    pub fn extent(self) -> Length {
        self.end - self.start
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Change {
    Insert(Position, Text),
    Delete(Position, Length),
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        match self {
            Self::Insert(position, text) => {
                Change::Delete(position, text.length())
            }
            Self::Delete(start, length) => Change::Insert(start, text.slice(start, length)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}