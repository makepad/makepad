use {
    crate::{Chunks, Slice},
    std::str,
};

/// An iterator over the bytes in a [`Rope`] or [`Slice`].
/// 
/// This `struct` is created by the [`bytes`](crate::Rope::bytes) method on [`Rope`] or the
/// [`bytes`](crate::Slice::bytes) method on [`Slice`].
/// 
/// [`Rope`]: crate::Rope
#[derive(Clone, Debug)]
pub struct Bytes<'a> {
    bytes: Option<str::Bytes<'a>>,
    chunks: Chunks<'a>,
}

impl<'a> Bytes<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            bytes: None,
            chunks: slice.chunks(),
        }
    }
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    /// Advances the iterator and returns the next byte.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log(n)) time.
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.bytes {
                Some(bytes) => match bytes.next() {
                    Some(byte) => break Some(byte),
                    None => {
                        self.bytes = None;
                        continue;
                    }
                },
                None => match self.chunks.next() {
                    Some(chunk) => {
                        self.bytes = Some(chunk.bytes());
                        continue;
                    }
                    None => break None,
                },
            }
        }
    }
}
