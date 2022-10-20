use {
    crate::{Bytes, BytesRev, Chars, CharsRev, ChunkCursor, Chunks, ChunksRev, Cursor, Info, Rope},
    std::{
        cmp::Ordering,
        hash::{Hash, Hasher},
        ops::RangeBounds,
    },
};

/// A view into a a [`Rope`].
#[derive(Clone, Copy, Debug)]
pub struct Slice<'a> {
    rope: &'a Rope,
    start_info: Info,
    end_info: Info,
}

impl<'a> Slice<'a> {
    /// Converts `self` to a [`Rope`].
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn to_rope(self) -> Rope {
        let mut rope = self.rope.clone();
        rope.truncate_back(self.end_info.byte_count);
        rope.truncate_front(self.start_info.byte_count);
        rope
    }

    /// Returns `true` if `self` is empty.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn is_empty(self) -> bool {
        self.byte_len() == 0
    }

    /// Returns the length of `self` in bytes.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn byte_len(self) -> usize {
        self.end_info.byte_count - self.start_info.byte_count
    }

    /// Returns the length of `self` in `char`s.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn char_len(self) -> usize {
        self.end_info.char_count - self.start_info.char_count
    }

    /// Returns the length of `self` in lines.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn line_len(self) -> usize {
        self.end_info.line_break_count - self.start_info.line_break_count + 1
    }

    /// Returns `true` if `byte_index` lies on a `char` boundary.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn is_char_boundary(self, byte_index: usize) -> bool {
        assert!(byte_index <= self.byte_len());
        self.rope
            .is_char_boundary(self.start_info.byte_count + byte_index)
    }

    /// Converts the given `byte_index` to a `char` index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it does not lie
    /// on a `char` boundary.
    pub fn byte_to_char(self, byte_index: usize) -> usize {
        self.info_at(byte_index).char_count
    }

    /// Converts the given `byte_index` to a line index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it does not lie
    /// on a `char` boundary.
    pub fn byte_to_line(self, byte_index: usize) -> usize {
        assert!(byte_index <= self.byte_len());
        self.info_at(byte_index).line_break_count + 1
    }

    /// Converts the given `char_index` to a byte index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `char_index` is greater than the length of `self` in chars.
    pub fn char_to_byte(self, char_index: usize) -> usize {
        assert!(char_index <= self.char_len());
        if char_index == 0 {
            return 0;
        }
        if char_index == self.char_len() {
            return self.byte_len();
        }
        self.rope
            .char_to_byte(self.start_info.char_count + char_index)
            - self.start_info.byte_count
    }

    /// Converts the given `line_index` to a byte index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `line_index` is greater than or equal to the length of `self` in lines.
    pub fn line_to_byte(self, line_index: usize) -> usize {
        assert!(line_index < self.line_len());
        if line_index == 0 {
            return 0;
        }
        let byte_index = self
            .rope
            .line_to_byte(self.start_info.line_break_count + line_index)
            - self.start_info.byte_count;
        byte_index.min(self.byte_len())
    }

    /// Returns the slice of `self` corresponding to the given `byte_range`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_range` is out of bounds.
    pub fn slice<R: RangeBounds<usize>>(self, byte_range: R) -> Slice<'a> {
        let byte_range = crate::range_bounds_to_range(byte_range, self.byte_len());
        Slice::new(
            &self.rope,
            self.start_info.byte_count + byte_range.start,
            self.start_info.byte_count + byte_range.end,
        )
    }

    /// Returns a [`ChunkCursor`] at the front chunk of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunk_cursor_front(self) -> ChunkCursor<'a> {
        ChunkCursor::front(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    /// Returns a [`ChunkCursor`] at the back chunk of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunk_cursor_back(self) -> ChunkCursor<'a> {
        ChunkCursor::back(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    /// Returns a [`ChunkCursor`] at the chunk containing the given `byte_position` within `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes.
    pub fn chunk_cursor_at(self, byte_position: usize) -> ChunkCursor<'a> {
        assert!(byte_position <= self.byte_len());
        ChunkCursor::at(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
            self.start_info.byte_count + byte_position,
        )
    }

    /// Returns a [`Cursor`] at the front of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn cursor_front(self) -> Cursor<'a> {
        Cursor::front(self)
    }

    /// Returns a [`Cursor`] at the back of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn cursor_back(self) -> Cursor<'a> {
        Cursor::back(self)
    }

    /// Returns a [`Cursor`] at the given `byte_position` within `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes.
    pub fn cursor_at(self, byte_position: usize) -> Cursor<'a> {
        Cursor::at(self, byte_position)
    }

    /// Returns an iterator over the chunks of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunks(self) -> Chunks<'a> {
        Chunks::new(self)
    }

    /// Returns a reverse iterator over the chunks of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunks_rev(self) -> ChunksRev<'a> {
        ChunksRev::new(self)
    }

    /// Returns an iterator over the bytes of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn bytes(self) -> Bytes<'a> {
        Bytes::new(self)
    }

    /// Returns a reverse iterator over the bytes of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn bytes_rev(self) -> BytesRev<'a> {
        BytesRev::new(self)
    }

    /// Returns an iterator over the `char`s of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chars(self) -> Chars<'a> {
        Chars::new(self)
    }

    /// Returns an iterator over the `char`s of `self.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chars_rev(self) -> CharsRev<'a> {
        CharsRev::new(self)
    }

    pub(crate) fn new(rope: &'a Rope, byte_start: usize, byte_end: usize) -> Self {
        use crate::StrUtils;

        let start_info = if byte_start == 0 {
            Info::new()
        } else if byte_start == rope.byte_len() {
            rope.root().info()
        } else {
            let (chunk, mut start_info) = rope.root().chunk_at_byte(byte_start);
            let byte_index = byte_start - start_info.byte_count;
            start_info += Info::from(&chunk[..byte_index]);
            if chunk[..byte_index].last_is_cr() && chunk[byte_index..].first_is_lf() {
                start_info.line_break_count -= 1;
            }
            start_info
        };
        Self {
            rope,
            start_info,
            end_info: if byte_start == byte_end {
                start_info
            } else {
                rope.info_at(byte_end)
            },
        }
    }

    pub(crate) fn info_at(&self, byte_index: usize) -> Info {
        assert!(byte_index <= self.byte_len());
        if byte_index == 0 {
            return Info::new();
        }
        if byte_index == self.byte_len() {
            return self.end_info - self.start_info;
        }
        self.rope.info_at(self.start_info.byte_count + byte_index) - self.start_info
    }
}

impl<'a> Eq for Slice<'a> {}

impl<'a> Hash for Slice<'a> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for chunk in self.chunks() {
            state.write(chunk.as_bytes());
        }
        state.write_u8(0xff)
    }
}

impl<'a> Ord for Slice<'a> {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut chunks_0 = self.chunks();
        let mut chunks_1 = other.chunks();
        let mut chunk_0 = chunks_0.next().unwrap_or("").as_bytes();
        let mut chunk_1 = chunks_1.next().unwrap_or("").as_bytes();
        loop {
            match chunk_0.len().cmp(&chunk_1.len()) {
                Ordering::Less => {
                    let len = chunk_0.len();
                    if len == 0 {
                        break Ordering::Less;
                    }
                    let cmp = chunk_0.cmp(&chunk_1[..len]);
                    if cmp != Ordering::Equal {
                        break cmp;
                    }
                    chunk_0 = chunks_0.next().unwrap_or("").as_bytes();
                    chunk_1 = &chunk_1[len..];
                }
                Ordering::Equal => {
                    if chunk_0.len() == 0 {
                        break Ordering::Equal;
                    }
                    let cmp = chunk_0.cmp(&chunk_1);
                    if cmp != Ordering::Equal {
                        break cmp;
                    }
                    chunk_0 = chunks_0.next().unwrap_or("").as_bytes();
                    chunk_1 = chunks_1.next().unwrap_or("").as_bytes();
                }
                Ordering::Greater => {
                    let len = chunk_1.len();
                    if len == 0 {
                        break Ordering::Greater;
                    }
                    let cmp = chunk_0[..len].cmp(&chunk_1);
                    if cmp != Ordering::Equal {
                        break cmp;
                    }
                    chunk_0 = &chunk_0[len..];
                    chunk_1 = chunks_1.next().unwrap_or("").as_bytes();
                }
            }
        }
    }
}

impl<'a> PartialEq for Slice<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<'a> PartialOrd for Slice<'a> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
