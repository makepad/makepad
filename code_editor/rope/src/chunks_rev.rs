use crate::{ChunkCursor, Slice};

/// A reverse iterator over the chunks in a [`Rope`] or [`Slice`].
/// 
/// This `struct` is created by the [`chunks_rev`](crate::Rope::chunks_rev) method on [`Rope`] or
/// the [`chunks_rev`](crate::Slice::chunks_rev) method on [`Slice`].
/// 
/// [`Rope`]: crate::Rope
#[derive(Clone, Debug)]
pub struct ChunksRev<'a> {
    is_at_end: bool,
    chunk_cursor: ChunkCursor<'a>,
}

impl<'a> ChunksRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            is_at_end: true,
            chunk_cursor: slice.chunk_cursor_back(),
        }
    }
}

impl<'a> Iterator for ChunksRev<'a> {
    type Item = &'a str;

    /// Advances the iterator and returns a reference to the next chunk.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log(n)) time.
    fn next(&mut self) -> Option<Self::Item> {
        if self.is_at_end {
            self.is_at_end = false;
        } else {
            if self.chunk_cursor.is_at_front() {
                return None;
            }
            self.chunk_cursor.move_prev();
        }
        Some(self.chunk_cursor.current())
    }
}
