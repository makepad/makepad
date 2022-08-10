mod branch;
mod builder;
mod bytes;
mod bytes_rev;
mod char_cursor;
mod chars;
mod chars_rev;
mod chunks;
mod chunks_rev;
mod chunk_cursor;
mod info;
mod leaf;
mod node;
mod rope;
mod slice;
mod str_utils;

pub use self::{
    builder::Builder, bytes::Bytes, bytes_rev::BytesRev, char_cursor::CharCursor, chars::Chars,
    chars_rev::CharsRev, chunks::Chunks, chunks_rev::ChunksRev, chunk_cursor::ChunkCursor, rope::Rope,
    slice::Slice,
};

use {
    self::{branch::Branch, info::Info, leaf::Leaf, node::Node, str_utils::StrUtils},
    std::ops::{Range, RangeBounds},
};

fn range_bounds_to_range<R: RangeBounds<usize>>(range: R, len: usize) -> Range<usize> {
    use std::ops::Bound;

    let start = match range.start_bound() {
        Bound::Excluded(&start) => start.checked_add(1).unwrap(),
        Bound::Included(&start) => start,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Excluded(&end) => end,
        Bound::Included(&end) => end.checked_add(1).unwrap(),
        Bound::Unbounded => len,
    };
    assert!(start <= end);
    assert!(end <= len);
    start..end
}

#[cfg(test)]
mod tests;
