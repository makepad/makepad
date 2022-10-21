use crate::makepad_micro_serde::{DeBin, DeBinErr, SerBin};
use {
    crate::{
        delta::{Delta, Operation},
        position::Position,
        range::Range,
        size::Size,
    },
    std::{fmt, iter, mem, ops::AddAssign},
};

/// A type for representing text.
///
/// A text is structured as a vec of lines, where each line is a vec of chars. This is not a very
/// compact representation, since most characters are expected to be ASCII, and thus take up only
/// a single byte, whereas a char always takes up 4 bytes. This means that for a typical text, up
/// to 75% of memory it uses is wasted. However, this representation is very convenient to work
/// with, because it closely matches the structure expected by most operations performed on a text.
/// Since large texts are relatively rare, we've thus decided to accept this inefficiency for now.
/// In the future, we might want to replace this representation with a more efficient one, such as
/// an UTF-8 rope.
///
/// A text maintains the invariant that it always contains at least one (possibly empty) line.
#[derive(Clone, Debug, Eq, Hash, PartialEq, SerBin, DeBin)]
pub struct Text {
    lines: Vec<Vec<char>>,
}

impl Text {
    /// Creates an empty text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::Text;
    /// 
    /// let text = Text::new();
    /// ```
    pub fn new() -> Text {
        Text::default()
    }

    /// Create a text from a vec of lines.
    /// 
    /// # Panics
    /// 
    /// Panics if the vec is empty.
    pub fn from_lines(lines: Vec<Vec<char>>) -> Text {
        if lines.is_empty(){
            Text{lines:vec![vec![' ']]}
        }
        else{
            Text { lines }
        }
    }

    /// Returns `true` if this text is empty.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::Text;
    /// 
    /// let text = Text::new();
    /// assert!(text.is_empty());
    /// let text = Text::from("abc\ndef");
    /// assert!(!text.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len().is_zero()
    }

    /// Returns the length of this text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Size, Text};
    /// 
    /// let text = Text::from("abc\ndef");
    /// assert!(text.len() == Size { line: 1, column: 3 });
    /// ```
    pub fn len(&self) -> Size {
        Size {
            line: self.lines.len() as u32 - 1 ,
            column: self.lines.last().unwrap().len() as u32,
        }
    }

    /// Returns a slice of the lines in this text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Size, Text};
    /// 
    /// let text = Text::from("abc\ndef");
    /// assert_eq!(text.as_lines(), &[vec!['a', 'b', 'c'], vec!['d', 'e', 'f']]);
    /// ```
    pub fn as_lines(&self) -> &[Vec<char>] {
        &self.lines
    }

    /// Copies the given range from this text into a new text.
    ///
    /// # Panics
    ///
    /// Panics if the range is out of bounds.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Position, Range, Text};
    /// 
    /// let text = Text::from("abc\ndef");
    /// assert_eq!(
    ///     text.copy(
    ///         Range {
    ///             start: Position { line: 0, column: 1 },
    ///             end: Position { line: 1, column: 2 }
    ///         }
    ///     ),
    ///     Text::from("bc\nde"),
    /// );
    /// ```
    pub fn copy(&self, range: Range) -> Text {
        Text {
            lines: if range.start.line == range.end.line {
                vec![
                    self.lines[range.start.line][range.start.column..range.end.column]
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                ]
            } else {
                let mut lines = Vec::with_capacity(range.end.line - range.start.line + 1);
                lines.push(
                    self.lines[range.start.line][range.start.column..]
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                lines.extend(
                    self.lines[range.start.line + 1..range.end.line]
                        .iter()
                        .cloned(),
                );
                lines.push(
                    self.lines[range.end.line][..range.end.column]
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                );
                lines
            },
        }
    }

    /// Appends the given range of this text to the given string.
    /// 
    /// # Panics
    /// 
    /// Panics if the range is out of bounds.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Position, Range, Text};
    /// 
    /// let text = Text::from("abc\ndef");
    /// let mut string = String::new();
    /// text.append_to_string(
    ///     Range { start: Position { line: 0, column: 1 }, end: Position { line: 1, column: 2 }},
    ///     &mut string
    /// );
    /// assert_eq!(string, "bc\nde");
    /// ```
    pub fn append_to_string(&self, range: Range, out: &mut String) {
        if range.start.line == range.end.line {
            out.extend(
                self.lines[range.start.line][range.start.column..range.end.column]
                    .iter()
                    .cloned(),
            );
        } else {
            out.extend(
                self.lines[range.start.line][range.start.column..]
                    .iter()
                    .cloned()
                    .chain(iter::once('\n'))
                    .chain(
                        self.lines[range.start.line + 1..range.end.line]
                            .iter()
                            .flat_map(|line| line.iter().cloned().chain(iter::once('\n'))),
                    )
                    .chain(
                        self.lines[range.end.line][..range.end.column]
                            .iter()
                            .cloned(),
                    ),
            );
        }
    }

    /// Removes the given amount of text from the start of this text, and returns it as a new text.
    /// 
    /// ```
    /// use makepad_editor_core::{Size, Text};
    /// 
    /// let mut text = Text::from("abc\ndef");
    /// assert_eq!(text.take(Size { line: 1, column: 1 }), Text::from("abc\nd"));
    /// assert_eq!(text, Text::from("ef"));
    /// ```
    pub fn take(&mut self, len: Size) -> Text {
        let mut lines = self.lines.drain(..len.line as usize).collect::<Vec<_>>();
        lines.push(
            self.lines
                .first_mut()
                .unwrap()
                .drain(..len.column as usize)
                .collect::<Vec<_>>(),
        );
        Text { lines }
    }

    /// Removes the given amount of text from the start of this text.
    /// 
    /// ```
    /// use makepad_editor_core::{Size, Text};
    /// 
    /// let mut text = Text::from("abc\ndef");
    /// text.skip(Size { line: 1, column: 1 });
    /// assert_eq!(text, Text::from("ef"));
    /// ```
    pub fn skip(&mut self, len: Size) {
        self.lines.drain(..len.line as usize);
        self.lines.first_mut().unwrap().drain(..len.column as usize);
    }

    /// Inserts the given text at the given position in this text.
    /// 
    /// # Panics
    /// 
    /// Panics if the position is out of bounds.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Position, Text};
    /// 
    /// let mut text = Text::from("abc\ndef");
    /// text.insert(Position { line: 1, column: 1 }, Text::from("xyz"));
    /// assert_eq!(text, Text::from("abc\ndxyzef"));
    /// ```
    pub fn insert(&mut self, position: Position, mut text: Text) {
        if text.len().line == 0 {
            self.lines[position.line].splice(
                position.column..position.column,
                text.lines.first().unwrap().iter().cloned(),
            );
        } else {
            text.lines.first_mut().unwrap().splice(
                ..0,
                self.lines[position.line][..position.column].iter().cloned(),
            );
            text.lines
                .last_mut()
                .unwrap()
                .extend(self.lines[position.line][position.column..].iter().cloned());
            self.lines
                .splice(position.line..position.line + 1, text.lines.into_iter());
        }
    }

    /// Deletes the given amount of text at the given position from this text.
    /// 
    /// # Panics
    /// 
    /// Panics if the position and/or the amount is out of bounds.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::{Position, Size, Text};
    /// 
    /// let mut text = Text::from("abc\ndef");
    /// text.delete(Position { line: 0, column: 2 }, Size { line: 1, column: 1 });
    /// assert_eq!(text, Text::from("abef"));
    /// ```
    pub fn delete(&mut self, position: Position, count: Size) {
        if count.line == 0 {
            self.lines[position.line].splice(
                position.column..position.column + count.column as usize,
                iter::empty(),
            );
        } else {
            let mut line = mem::replace(&mut self.lines[position.line], Vec::new());
            line.splice(
                position.column..,
                self.lines[position.line + count.line as usize][count.column as usize..]
                    .iter()
                    .cloned(),
            );
            self.lines.splice(
                position.line..position.line + count.line as usize + 1,
                iter::once(line),
            );
        }
    }

    /// Applies the given delta to this text.
    /// 
    /// # Panics
    /// 
    /// Panics if the delta is not compatible with this text.
    pub fn apply_delta(&mut self, delta: Delta) {
        let mut position = Position::origin();
        for operation in delta {
            match operation {
                Operation::Retain(count) => position += count,
                Operation::Insert(text) => {
                    let len = text.len();
                    self.insert(position, text);
                    position += len;
                }
                Operation::Delete(count) => self.delete(position, count),
            }
        }
    }
}

impl AddAssign for Text {
    fn add_assign(&mut self, other: Text) {
        self.lines
            .last_mut()
            .unwrap()
            .extend(other.lines.first().unwrap());
        self.lines.extend(other.lines.into_iter().skip(1))
    }
}

impl Default for Text {
    fn default() -> Text {
        Text::from_lines(vec![vec![]])
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sep = "";
        for line in self.lines.iter() {
            write!(f, "{}", sep)?;
            for ch in line {
                write!(f, "{}", ch)?;
            }
            sep = "\n";
        }
        Ok(())
    }
}

impl From<String> for Text {
    fn from(string: String) -> Text {
        Text::from(string.as_str())
    }
}

impl From<&str> for Text {
    fn from(string: &str) -> Text {
        Text::from_lines(
            string
                .lines()
                .map(|line| line.chars().collect::<Vec<_>>())
                .collect::<Vec<_>>(),
        )
    }
}
