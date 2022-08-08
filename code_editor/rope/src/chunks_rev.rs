use crate::{Cursor, Slice};

/// A reverse iterator over the chunks of a `Rope` or `Slice`.
#[derive(Clone, Debug)]
pub struct ChunksRev<'a> {
    is_at_end: bool,
    cursor: Cursor<'a>,
}

impl<'a> ChunksRev<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            is_at_end: true,
            cursor: slice.cursor_back(),
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
        if self.is_at_end {
            self.is_at_end = false;
        } else {
            if self.cursor.is_at_front() {
                return None;
            }
            self.cursor.move_prev();
        }
        Some(self.cursor.current())
    }
}
