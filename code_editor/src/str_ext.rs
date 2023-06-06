pub trait StrExt {
    fn is_grapheme_boundary(&self, index: usize) -> bool;
    fn next_grapheme_boundary(&self, index: usize) -> Option<usize>;
    fn graphemes(&self) -> Graphemes<'_>;
}

impl StrExt for str {
    fn is_grapheme_boundary(&self, index: usize) -> bool {
        self.is_char_boundary(index)
    }

    fn next_grapheme_boundary(&self, index: usize) -> Option<usize> {
        if index == self.len() {
            return None;
        }
        let mut index = index;
        loop {
            index += 1;
            if self.is_grapheme_boundary(index) {
                return Some(index);
            }
        }
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
        let (grapheme, remaining_string) = self.string.split_at(index);
        self.string = remaining_string;
        Some(grapheme)
    }
}
