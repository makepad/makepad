pub trait StrExt {
    fn width(&self, tab_width: usize) -> usize;
    fn graphemes(&self) -> Graphemes<'_>;
}

impl StrExt for str {
    fn width(&self, tab_width: usize) -> usize {
        use crate::char::CharExt;

        self.chars().map(|char| char.width(tab_width)).sum()
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
        let mut index = 1;
        while !self.string.is_char_boundary(index) {
            index += 1;
        }
        let (grapheme, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(grapheme)
    }
}

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.string.is_empty() {
            return None;
        }
        let mut index = self.string.len() - 1;
        while !self.string.is_char_boundary(index) {
            index -= 1;
        }
        let (remaining_string, grapheme) = self.string.split_at(index);
        self.string = remaining_string;
        Some(grapheme)
    }
}
