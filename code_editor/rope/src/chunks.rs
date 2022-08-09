use crate::{ChunkCursor, Slice};

/// An iterator over the chunks of a `Rope` or `Slice`.
#[derive(Clone, Debug)]
pub struct Chunks<'a> {
    chunk_cursor: ChunkCursor<'a>,
}

impl<'a> Chunks<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
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
    /// Runs in amortized O(1) and worst-case O(log n) time.
    fn next(&mut self) -> Option<Self::Item> {
        if self.chunk_cursor.is_at_back() {
            return None;
        }
        let chunk = self.chunk_cursor.current();
        self.chunk_cursor.move_next();
        Some(chunk)
    }
}
