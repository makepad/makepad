use crate::{ChunkCursor, Slice};

/// A cursor over a [`Rope`] or [`Slice`].
/// 
/// [`Rope`]: crate::Rope
#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    chunk_cursor: ChunkCursor<'a>,
    chunk: &'a str,
    byte_index: usize,
}

impl<'a> Cursor<'a> {
    /// Returns `true` if `self` is currently pointing to the front of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    /// 
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn is_at_front(&self) -> bool {
        self.chunk_cursor.is_at_front() && self.byte_index == 0
    }

    /// Returns `true` if `self` is currently pointing to the back of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    /// 
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn is_at_back(&self) -> bool {
        self.chunk_cursor.is_at_back() && self.byte_index == self.chunk.len()
    }

    /// Returns `true` if `self` is currently pointing to a `char` boundary.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    #[inline]
    pub fn is_at_char_boundary(&self) -> bool {
        self.chunk.is_char_boundary(self.byte_index)
    }

    /// Returns the byte position of `self` within the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn byte_position(&self) -> usize {
        self.chunk_cursor.byte_position() + self.byte_index
    }

    /// Returns the byte that `self` is currently pointing to, or `None` if `self` is currently
    /// pointing to the back of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn current_byte(&self) -> Option<u8> {
        self.chunk.as_bytes().get(self.byte_index).cloned()
    }

    /// Returns the [`char`] that `self` is currently pointing to, or `None` if `self` is currently
    /// pointing to the back of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is not currently pointing to a `char` boundary.
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn current_char(&self) -> Option<char> {
        self.chunk[self.byte_index..].chars().next()
    }

    /// Moves `self` to the next byte of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    /// 
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the back of the [`Rope`] or [`Slice`].
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn move_next_byte(&mut self) {
        assert!(!self.is_at_back());
        self.byte_index += 1;
        if self.byte_index == self.chunk.len() && !self.chunk_cursor.is_at_back() {
            self.move_next();
        }
    }

    /// Moves `self` to the previous byte of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    /// 
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the front of the [`Rope`] or [`Slice`].
    /// 
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn move_prev_byte(&mut self) {
        assert!(!self.is_at_front());
        if self.byte_index == 0 {
            self.move_prev();
        }
        self.byte_index -= 1;
    }

    /// Moves `self` to the next [`char`] of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the back of the [`Rope`] or [`Slice`].
    /// 
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn move_next_char(&mut self) {
        assert!(!self.is_at_back());
        self.byte_index += utf8_char_width(self.chunk.as_bytes()[self.byte_index]);
        if self.byte_index == self.chunk.len() && !self.chunk_cursor.is_at_back() {
            self.move_next();
        }
    }

    /// Moves `self` to the previous [`char`] of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the front of the [`Rope`] or [`Slice`].
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn move_prev_char(&mut self) {
        assert!(!self.is_at_front());
        if self.byte_index == 0 {
            self.move_prev();
        }
        loop {
            self.byte_index -= 1;
            if self.chunk.is_char_boundary(self.byte_index) {
                break;
            }
        }
    }

    /// Moves `self` to the given `byte_position` within the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_position` is greater than the length of the [`Rope`] or [`Slice`] in bytes.
    ///
    /// [`Rope`]: crate::Rope
    #[inline]
    pub fn move_to(&mut self, byte_position: usize) {
        self.chunk_cursor.move_to(byte_position);
        self.chunk = self.chunk_cursor.current();
        self.byte_index = byte_position - self.chunk_cursor.byte_position();
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
        let byte_index = byte_position - chunk_cursor.byte_position();
        Self {
            chunk_cursor,
            chunk,
            byte_index,
        }
    }

    fn move_next(&mut self) {
        self.chunk_cursor.move_next();
        self.chunk = self.chunk_cursor.current();
        self.byte_index = 0;
    }

    fn move_prev(&mut self) {
        self.chunk_cursor.move_prev();
        self.chunk = self.chunk_cursor.current();
        self.byte_index = self.chunk.len();
    }
}

#[inline]
fn utf8_char_width(byte: u8) -> usize {
    match byte {
        byte if byte < 0x80 => 1,
        byte if byte < 0xe0 => 2,
        byte if byte < 0xf0 => 3,
        _ => 4,
    }
}
