use crate::{str::{StrExt, Words}};

/// An iterator over the words of a `str`, and their positions.
/// 
/// This struct is created by the `word_indices` method on `StrExt`.
pub struct WordIndices<'a> {
    origin: usize,
    words: Words<'a>,
}

impl<'a> WordIndices<'a> {
    pub(super) fn new(string: &'a str) -> Self {
        Self {
            origin: string.as_ptr() as usize,
            words: string.words(),
        }
    }
}

impl<'a> Iterator for WordIndices<'a> {
    type Item = (usize, &'a str);

    fn next(&mut self) -> Option<Self::Item> {
        let word = self.words.next()?;
        Some((word.as_ptr() as usize - self.origin, word))
    }
}

impl<'a> DoubleEndedIterator for WordIndices<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let word = self.words.next_back()?;
        Some((word.as_ptr() as usize - self.origin, word))
    }
}