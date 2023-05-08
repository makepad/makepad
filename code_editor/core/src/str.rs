pub trait StrExt {
    fn is_grapheme_boundary(&self, index: usize) -> bool;
    fn prev_grapheme_boundary(&self, index: usize) -> Option<usize>;
    fn next_grapheme_boundary(&self, index: usize) -> Option<usize>;
    fn graphemes(&self) -> Graphemes<'_>;
}

impl StrExt for str {
    fn is_grapheme_boundary(&self, index: usize) -> bool {
        self.is_char_boundary(index)
    }

    fn prev_grapheme_boundary(&self, index: usize) -> Option<usize> {
        if index == 0 {
            return None;
        }
        let mut index = index - 1;
        while !self.is_grapheme_boundary(index) {
            index -= 1;
        }
        Some(index)
    }

    fn next_grapheme_boundary(&self, index: usize) -> Option<usize> {
        if index == self.len() {
            return None;
        }
        let mut index = index + 1;
        while !self.is_grapheme_boundary(index) {
            index += 1;
        }
        Some(index)
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
        let index = self.string.next_grapheme_boundary(0)?;
        let (string_0, string_1) = self.string.split_at(index);
        self.string = string_1;
        Some(string_0)
    }
}
