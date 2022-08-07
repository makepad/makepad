use {
    crate::{Bytes, BytesRev, Chars, CharsRev, Chunks, ChunksRev, Cursor, Info, Rope},
    std::ops::RangeBounds,
};

#[derive(Clone, Copy, Debug)]
pub struct Slice<'a> {
    rope: &'a Rope,
    start_info: Info,
    end_info: Info,
}

impl<'a> Slice<'a> {
    /// Returns `true` if `self` is empty.
    /// 
    /// Runs in O(1) time.
    pub fn is_empty(self) -> bool {
        self.byte_len() == 0
    }

    /// Returns the length of `self` in bytes.
    /// 
    /// Runs in O(1) time.
    pub fn byte_len(self) -> usize {
        self.end_info.byte_count - self.start_info.byte_count
    }

    /// Returns the length of `self` in `char`s.
    /// 
    /// Runs in O(1) time.
    pub fn char_len(self) -> usize {
        self.end_info.char_count - self.start_info.char_count
    }

    /// Returns the length of `self` in lines.
    /// 
    /// Runs in O(1) time.
    pub fn line_len(self) -> usize {
        self.end_info.line_break_count - self.start_info.line_break_count + 1
    }

    /// Converts the given `byte_index` to a `char` index.
    /// 
    /// Runs in O(log n) time.
    pub fn byte_to_char(self, byte_index: usize) -> usize {
        self.info_at(byte_index).char_count
    }

    /// Converts the given `byte_index` to a line index.
    /// 
    /// Runs in O(log n) time.
    pub fn byte_to_line(self, byte_index: usize) -> usize {
        self.info_at(byte_index).line_break_count + 1
    }

    /// Converts the given `char_index` to a byte index.
    /// 
    /// Runs in O(log n) time.
    pub fn char_to_byte(self, char_index: usize) -> usize {
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
    /// Runs in O(log n) time.
    pub fn line_to_byte(self, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        (self
            .rope
            .line_to_byte(self.start_info.line_break_count + line_index))
        .min(self.end_info.byte_count)
            - self.start_info.byte_count
    }

    /// Returns the slice of `self` corresponding to the given `byte_range`.
    /// 
    /// Runs in O(log n) time.
    pub fn slice<R: RangeBounds<usize>>(&self, byte_range: R) -> Slice<'_> {
        let byte_range = crate::range_bounds_to_range(byte_range, self.byte_len());
        Slice::new(
            &self.rope,
            self.start_info.byte_count + byte_range.start,
            self.start_info.byte_count + byte_range.end,
        )
    }

    /// Returns a `Cursor` at the front of `self`.
    pub fn cursor_front(self) -> Cursor<'a> {
        Cursor::front(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    /// Returns a `Cursor` at the back of `self`.
    ///
    /// Runs in O(log n) time.
    pub fn cursor_back(self) -> Cursor<'a> {
        Cursor::back(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    /// Returns a `Cursor` at the given `byte_index` of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn cursor_at(self, byte_index: usize) -> Cursor<'a> {
        Cursor::at(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
            byte_index,
        )
    }

    /// Returns an iterator over the chunks of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn chunks(self) -> Chunks<'a> {
        Chunks::new(self)
    }

    /// Returns a reverse iterator over the chunks of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn chunks_rev(self) -> ChunksRev<'a> {
        ChunksRev::new(self)
    }

    /// Returns an iterator over the bytes of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn bytes(self) -> Bytes<'a> {
        Bytes::new(self)
    }

    /// Returns a reverse iterator over the bytes of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn bytes_rev(self) -> BytesRev<'a> {
        BytesRev::new(self)
    }

    /// Returns an iterator over the `char`s of `self`.
    /// 
    /// Runs in O(log n) time.
    pub fn chars(self) -> Chars<'a> {
        Chars::new(self)
    }

    /// Returns an iterator over the `char`s of `self.
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
        if byte_index == 0 {
            return Info::new();
        }
        if byte_index == self.byte_len() {
            return self.end_info - self.start_info;
        }
        self.rope.info_at(self.start_info.byte_count + byte_index) - self.start_info
    }
}
