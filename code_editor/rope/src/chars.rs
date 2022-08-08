use {
    crate::{Chunks, Slice},
    std::str,
};

/// An iterator over the `char`s of a `Rope` or `Slice`.
#[derive(Clone, Debug)]
pub struct Chars<'a> {
    chars: Option<str::Chars<'a>>,
    chunks: Chunks<'a>,
}

impl<'a> Chars<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            chars: None,
            chunks: slice.chunks(),
        }
    }
}

impl<'a> Iterator for Chars<'a> {
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
                Some(chars) => match chars.next() {
                    Some(ch) => break Some(ch),
                    None => {
                        self.chars = None;
                        continue;
                    }
                },
                None => match self.chunks.next() {
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
