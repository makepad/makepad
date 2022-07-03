use crate::{str::{StrExt, Graphemes}};

/// An iterator over the graphemes of a `str`, and their positions.
/// 
/// This struct is created by the `grapheme_indices` method on `StrExt`.
pub struct GraphemeIndices<'a> {
    origin: usize,
    graphemes: Graphemes<'a>,
}

impl<'a> GraphemeIndices<'a> {
    pub(super) fn new(string: &'a str) -> Self {
        Self {
            origin: string.as_ptr() as usize,
            graphemes: string.graphemes(),
        }
    }
}

impl<'a> Iterator for GraphemeIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next()?;
        Some((grapheme.as_ptr() as usize - self.origin, grapheme))
    }
}

impl<'a> DoubleEndedIterator for GraphemeIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let grapheme = self.graphemes.next_back()?;
        Some((grapheme.as_ptr() as usize - self.origin, grapheme))
    }
}