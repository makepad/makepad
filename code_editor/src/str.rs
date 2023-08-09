use crate::char::CharExt;

pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn total_indent(&self) -> Option<&str>;
    fn graphemes(&self) -> Graphemes<'_>;
    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn total_indent(&self) -> Option<&str> {
        self.char_indices()
            .find(|(_, char)| !char.is_whitespace())
            .map(|(index, _)| &self[..index])
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
    }

    fn split_whitespace_boundaries(&self) -> SplitWhitespaceBoundaries<'_> {
        SplitWhitespaceBoundaries { string: self }
    }
}

#[derive(Clone, Debug)]
pub struct Graphemes<'a> {
    string: &'a str,
}

impl<'a> Iterator for Graphemes<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut end = 1;
        while !self.string.is_char_boundary(end) {
            end += 1;
        }
        let (grapheme, string) = self.string.split_at(end);
        self.string = string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut start = self.string.len() - 1;
        while !self.string.is_char_boundary(start) {
            start -= 1;
        }
        let (string, grapheme) = self.string.split_at(start);
        self.string = string;
        Some(grapheme)
    }
}

#[derive(Clone, Debug)]
pub struct SplitWhitespaceBoundaries<'a> {
    string: &'a str,
}

impl<'a> Iterator for SplitWhitespaceBoundaries<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut prev_char_is_whitespace = None;
        let index = self
            .string
            .char_indices()
            .find_map(|(index, next_char)| {
                let next_char_is_whitespace = next_char.is_whitespace();
                let is_whitespace_boundary = prev_char_is_whitespace
                    .map_or(false, |prev_char_is_whitespace| {
                        prev_char_is_whitespace != next_char_is_whitespace
                    });
                prev_char_is_whitespace = Some(next_char_is_whitespace);
                if is_whitespace_boundary {
                    Some(index)
                } else {
                    None
                }
            })
            .unwrap_or(self.string.len());
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
