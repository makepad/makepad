use crate::{ChunkCursor, Slice};

/// A cursor over the bytes of a `Rope`.
#[derive(Clone, Debug)]
pub struct ByteCursor<'a> {
    chunk_cursor: ChunkCursor<'a>,
    chunk: &'a str,
    byte_index: usize,
}

impl<'a> ByteCursor<'a> {
    /// Returns `true` if `self` is currently pointing to the front of the `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    #[inline]
    pub fn is_at_front(&self) -> bool {
        self.chunk_cursor.is_at_front() && self.byte_index == 0
    }

    /// Returns `true` if `self` is currently pointing to the back of the `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    #[inline]
    pub fn is_at_back(&self) -> bool {
        self.chunk_cursor.is_at_back() && self.byte_index == self.chunk.len()
    }

    /// Returns the byte position of `self` within the `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    #[inline]
    pub fn byte_position(&self) -> usize {
        self.chunk_cursor.byte_position() + self.byte_index
    }

    /// Returns the byte that `self` is currently pointing to.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    #[inline]
    pub fn current(&self) -> char {
        self.chunk[self.byte_index..].chars().next().unwrap()
    }

    /// Moves `self` to the next byte of the `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the back of the `Rope`.
    #[inline]
    pub fn move_next(&mut self) {
        assert!(!self.is_at_back());
        self.byte_index += 1;
        if self.byte_index == self.chunk.len() && !self.chunk_cursor.is_at_back() {
            self.chunk_cursor.move_next();
            self.chunk = self.chunk_cursor.current();
            self.byte_index = 0;
        }
    }

    /// Moves `self` to the previous byte of the `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the front of the `Rope`.
    #[inline]
    pub fn move_prev(&mut self) {
        assert!(!self.is_at_front());
        if self.byte_index == 0 {
            self.chunk_cursor.move_prev();
            self.chunk = self.chunk_cursor.current();
            self.byte_index = self.chunk.len();
        }
        self.byte_index -= 1;
    }

    pub(crate) fn front(slice: Slice<'a>) -> Self {
        let chunk_cursor = slice.chunk_cursor_front();
        let chunk = chunk_cursor.current();
        Self {
            chunk_cursor,
            chunk,
            byte_index: 0,
        }
    }

    pub(crate) fn back(slice: Slice<'a>) -> Self {
        let chunk_cursor = slice.chunk_cursor_back();
        let chunk = chunk_cursor.current();
        Self {
            chunk_cursor,
            chunk,
            byte_index: chunk.len(),
        }
    }

    pub(crate) fn at(slice: Slice<'a>, byte_position: usize) -> Self {
        let chunk_cursor = slice.chunk_cursor_at(byte_position);
        let chunk = chunk_cursor.current();
        let byte_index = chunk_cursor.byte_position() - byte_position;
        Self {
            chunk_cursor,
            chunk,
            byte_index,
        }
    }
}
