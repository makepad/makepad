use {
    crate::{ChunksRev, Slice},
    std::str,
};

/// A reverse iterator over the bytes in a [`Rope`] or [`Slice`].
/// 
/// This `struct` is created by the [`bytes_rev`](crate::Rope::bytes_rev) method on [`Rope`] or the
/// [`bytes_rev`](crate::Slice::bytes_rev) method on [`Slice`].
/// 
/// [`Rope`]: crate::Rope
#[derive(Clone, Debug)]
pub struct BytesRev<'a> {
    bytes: Option<str::Bytes<'a>>,
    chunks_rev: ChunksRev<'a>,
}

impl<'a> BytesRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            bytes: None,
            chunks_rev: slice.chunks_rev(),
        }
    }
}

impl<'a> Iterator for BytesRev<'a> {
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
                Some(bytes) => match bytes.next_back() {
                    Some(byte) => break Some(byte),
                    None => {
                        self.bytes = None;
                        continue;
                    }
                },
                None => match self.chunks_rev.next() {
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
