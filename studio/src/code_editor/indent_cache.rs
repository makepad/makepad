use {
    crate::makepad_live_tokenizer::{
        delta::{Delta, OperationRange},
        text::Text,
    },
    std::{
        ops::{Deref, Index},
        slice::Iter,
    },
};

pub struct IndentCache {
    lines: Vec<Line>,
}

impl IndentCache {
    pub fn new(text: &Text) -> IndentCache {
        let mut cache = IndentCache {
            lines: (0..text.as_lines().len())
                .map(|_| Line::default())
                .collect::<Vec<_>>(),
        };
        cache.refresh(text);
        cache
    }

    pub fn invalidate(&mut self, delta: &Delta) {
        for operation_range in delta.operation_ranges() {
            match operation_range {
                OperationRange::Insert(range) => {
                    self.lines[range.start.line] = Line::default();
                    self.lines.splice(
                        range.start.line..range.start.line,
                        (0..range.end.line - range.start.line).map(|_| Line::default()),
                    );
                }
                OperationRange::Delete(range) => {
                    self.lines.drain(range.start.line..range.end.line);
                    self.lines[range.start.line] = Line::default();
                }
            }
        }
    }

    pub fn refresh(&mut self, text: &Text) {
        for (index, line) in self.lines.iter_mut().enumerate() {
            if line.leading_whitespace.is_some() {
                continue;
            }
            line.leading_whitespace = Some(
                text.as_lines()[index]
                    .iter()
                    .position(|ch| !ch.is_whitespace()),
            );
        }

        let mut leading_whitespace_above = 0;
        for line_info in self.lines.iter_mut() {
            if let Some(leading_whitespace) = line_info.leading_whitespace.unwrap() {
                leading_whitespace_above = leading_whitespace;
            }
            line_info.leading_whitespace_above = Some(leading_whitespace_above);
        }

        let mut leading_whitespace_below = 0;
        for line_info in self.lines.iter_mut().rev() {
            if let Some(leading_whitespace) = line_info.leading_whitespace.unwrap() {
                leading_whitespace_below = leading_whitespace;
            }
            line_info.leading_whitespace_below = Some(leading_whitespace_below);
        }
    }
}

impl Deref for IndentCache {
    type Target = [Line];

    fn deref(&self) -> &Self::Target {
        &self.lines
    }
}

impl Index<usize> for IndentCache {
    type Output = Line;

    fn index(&self, index: usize) -> &Self::Output {
        &self.lines[index]
    }
}

impl<'a> IntoIterator for &'a IndentCache {
    type Item = &'a Line;
    type IntoIter = Iter<'a, Line>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct Line {
    leading_whitespace: Option<Option<usize>>,
    leading_whitespace_above: Option<usize>,
    leading_whitespace_below: Option<usize>,
}

impl Line {
    pub fn leading_whitespace(&self) -> Option<usize> {
        self.leading_whitespace.unwrap()
    }

    pub fn leading_whitespace_above(&self) -> usize {
        self.leading_whitespace_above.unwrap()
    }

    pub fn virtual_leading_whitespace(&self) -> usize {
        self.leading_whitespace_above
            .unwrap()
            .max(self.leading_whitespace_below.unwrap())
    }
}
