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
            ChangeKind::Insert(point, additional_text) => self.insert(point, additional_text),
            ChangeKind::Delete(range) => self.delete(range),
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Point {
    pub line: usize,
    pub byte: usize,
}

impl Point {
    pub fn zero() -> Self {
        Self::default()
    }

    pub fn apply_change(self, change: &Change) -> Self {
        match change.kind {
            ChangeKind::Insert(point, ref text) => match self.cmp(&point) {
                Ordering::Less => self,
                Ordering::Equal => match change.drift {
                    Drift::Before => self + text.extent(),
                    Drift::After => self,
                },
                Ordering::Greater => point + text.extent() + (self - point),
            },
            ChangeKind::Delete(range) => {
                if self < range.start() {
                    self
                } else {
                    range.start() + (self - range.end().min(self))
                }
            }
        }
    }
}

impl Add<Extent> for Point {
    type Output = Self;

    fn add(self, extent: Extent) -> Self::Output {
        if extent.line_count == 0 {
            Self {
                line: self.line,
                byte: self.byte + extent.byte_count,
            }
        } else {
            Self {
                line: self.line + extent.line_count,
                byte: extent.byte_count,
            }
        }
    }
}

impl AddAssign<Extent> for Point {
    fn add_assign(&mut self, extent: Extent) {
        *self = *self + extent;
    }
}

impl Sub for Point {
    type Output = Extent;

    fn sub(self, other: Self) -> Self::Output {
        if self.line == other.line {
            Extent {
                line_count: 0,
                byte_count: self.byte - other.byte,
            }
        } else {
            Extent {
                line_count: self.line - other.line,
                byte_count: self.byte,
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Extent {
    pub line_count: usize,
    pub byte_count: usize,
}

impl Extent {
    pub fn zero() -> Extent {
        Self::default()
    }
}

impl Add for Extent {
    type Output = Extent;

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

impl AddAssign for Extent {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Extent {
    type Output = Extent;

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

impl SubAssign for Extent {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Range {
    start: Point,
    end: Point,
}

impl Range {
    pub fn new(start: Point, end: Point) -> Option<Self> {
        if start > end {
            return None;
        }
        Some(Self { start, end })
    }

    pub fn from_start_and_extent(start: Point, extent: Extent) -> Self {
        Self {
            start,
            end: start + extent,
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub fn start(self) -> Point {
        self.start
    }

    pub fn end(self) -> Point {
        self.end
    }

    pub fn extent(self) -> Extent {
        self.end - self.start
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Change {
    pub drift: Drift,
    pub kind: ChangeKind,
}

impl Change {
    pub fn invert(self, text: &Text) -> Self {
        Self {
            drift: self.drift,
            kind: match self.kind {
                ChangeKind::Insert(point, text) => {
                    ChangeKind::Delete(Range::from_start_and_extent(point, text.extent()))
                }
                ChangeKind::Delete(range) => ChangeKind::Insert(range.start(), text.slice(range)),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Drift {
    Before,
    After,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ChangeKind {
    Insert(Point, Text),
    Delete(Range),
}
