pub trait StrExt {
    fn is_grapheme_boundary(&self, index: usize) -> bool;
    fn prev_grapheme_boundary(&self, index: usize) -> Option<usize>;
    fn next_grapheme_boundary(&self, index: usize) -> Option<usize>;
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
}
