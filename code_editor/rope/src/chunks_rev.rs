use crate::{ChunkCursor, Slice};

/// A reverse iterator over the chunks of a `Rope` or `Slice`.
#[derive(Clone, Debug)]
pub struct ChunksRev<'a> {
    chunk_cursor: ChunkCursor<'a>,
}

impl<'a> ChunksRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
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
    /// Runs in amortized O(1) and worst-case O(log n) time.
    fn next(&mut self) -> Option<Self::Item> {
        println!("NEXT");
        if self.chunk_cursor.is_at_front() {
            println!("IS AT FRONT");
            return None;
        }
        println!("IS NOT AT FRONT");
        self.chunk_cursor.move_prev();
        Some(self.chunk_cursor.current())
    }
}
