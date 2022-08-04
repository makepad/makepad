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
    pub fn is_empty(self) -> bool {
        self.byte_len() == 0
    }

    pub fn byte_len(self) -> usize {
        self.end_info.byte_count - self.start_info.byte_count
    }

    pub fn char_len(self) -> usize {
        self.end_info.char_count - self.start_info.char_count
    }

    pub fn char_at_byte(self, byte_index: usize) -> usize {
        self.info_at_byte(byte_index).char_count
    }

    pub fn slice<R: RangeBounds<usize>>(&self, byte_range: R) -> Slice<'_> {
        let byte_range = crate::range_bounds_to_range(byte_range, self.byte_len());
        Slice::new(
            &self.rope,
            self.start_info.byte_count + byte_range.start,
            self.start_info.byte_count + byte_range.end,
        )
    }

    pub fn cursor_front(self) -> Cursor<'a> {
        Cursor::front(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    pub fn cursor_back(self) -> Cursor<'a> {
        Cursor::back(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
        )
    }

    pub fn cursor_at(self, byte_index: usize) -> Cursor<'a> {
        Cursor::at(
            self.rope.root(),
            self.start_info.byte_count,
            self.end_info.byte_count,
            byte_index,
        )
    }

    pub fn chunks(self) -> Chunks<'a> {
        Chunks::new(self)
    }

    pub fn chunks_rev(self) -> ChunksRev<'a> {
        ChunksRev::new(self)
    }

    pub fn bytes(self) -> Bytes<'a> {
        Bytes::new(self)
    }

    pub fn bytes_rev(self) -> BytesRev<'a> {
        BytesRev::new(self)
    }

    pub fn chars(self) -> Chars<'a> {
        Chars::new(self)
    }

    pub fn chars_rev(self) -> CharsRev<'a> {
        CharsRev::new(self)
    }

    pub(crate) fn new(rope: &'a Rope, byte_start: usize, byte_end: usize) -> Self {
        Self {
            rope,
            start_info: rope.info_at_byte(byte_start),
            end_info: rope.info_at_byte(byte_end),
        }
    }

    pub(crate) fn info_at_byte(&self, byte_index: usize) -> Info {
        if byte_index == 0 {
            return Info::new();
        }
        if byte_index == self.byte_len() {
            return self.end_info - self.start_info;
        }
        self.rope.info_at_byte(self.start_info.byte_count + byte_index) - self.start_info
    }
}
