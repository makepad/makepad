pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn graphemes(&self) -> Graphemes<'_>;
}

impl StrExt for str {
    fn column_count(&self, tab_column_count: usize) -> usize {
        use crate::char::CharExt;

        self.chars()
            .map(|char| char.column_count(tab_column_count))
            .sum()
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes { string: self }
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
