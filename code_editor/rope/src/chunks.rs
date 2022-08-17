use crate::{ChunkCursor, Slice};

/// An iterator over the chunks in a [`Rope`] or [`Slice`].
/// 
/// This `struct` is created by the [`chunks`](crate::Rope::chunks) method on [`Rope`] or the
/// [`chunks`](crate::Slice::chunks) method on [`Slice`].
/// 
/// [`Rope`]: crate::Rope
#[derive(Clone, Debug)]
pub struct Chunks<'a> {
    is_at_end: bool,
    chunk_cursor: ChunkCursor<'a>,
}

impl<'a> Chunks<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            is_at_end: false,
            chunk_cursor: slice.chunk_cursor_front(),
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    /// Advances the iterator and returns a reference to the next chunk.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log(n)) time.
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_at_end {
            return None;
        }
        let chunk = self.chunk_cursor.current();
        if self.chunk_cursor.is_at_back() {
            self.is_at_end = true;
        } else {
            self.chunk_cursor.move_next();
        }
        Some(chunk)
    }
}
