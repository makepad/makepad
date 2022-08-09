use {
    crate::{ChunksRev, Slice},
    std::str,
};

/// A reverse iterator over the `char`s of a `Rope` or `Slice`.
#[derive(Clone, Debug)]
pub struct CharsRev<'a> {
    chars: Option<str::Chars<'a>>,
    chunks_rev: ChunksRev<'a>,
}

impl<'a> CharsRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            chars: None,
            chunks_rev: slice.chunks_rev(),
        }
    }
}

impl<'a> Iterator for CharsRev<'a> {
    type Item = char;

    /// Advances the iterator and returns the next `char`.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.chars {
                Some(chars) => match chars.next_back() {
                    Some(ch) => break Some(ch),
                    None => {
                        self.chars = None;
                        continue;
                    }
                },
                None => match self.chunks_rev.next() {
                    Some(chunk) => {
                        self.chars = Some(chunk.chars());
                        continue;
                    }
                    None => break None,
                },
            }
        }
    }
}
