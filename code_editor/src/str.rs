pub trait StrExt {
    fn column_count(&self, tab_column_count: usize) -> usize;
    fn graphemes(&self) -> Graphemes<'_>;
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;
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

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices {
            graphemes: self.graphemes(),
            start: self.as_ptr() as usize,
        }
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

#[derive(Clone, Debug)]
pub struct GraphemeIndices<'a> {
    graphemes: Graphemes<'a>,
    start: usize,
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        println!("PENELEIN {:?}", grapheme);
        Some((grapheme.as_ptr() as usize - self.start, grapheme))
    }
}
