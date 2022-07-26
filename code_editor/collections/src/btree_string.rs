use {
    crate::{btree, btree::Measure, BTree},
    std::ops::{Add, AddAssign, RangeBounds, Sub, SubAssign},
};

#[derive(Clone, Debug)]
pub struct BTreeString {
    btree: BTree<String>,
}

impl BTreeString {
    pub fn new() -> Self {
        Self {
            btree: BTree::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.btree.is_empty()
    }

    pub fn len(&self) -> usize {
        self.btree.len()
    }

    pub fn char_count(&self) -> usize {
        self.btree.measure::<CharMeasure>()
    }

    pub fn char_count_at(&self, position: usize) -> usize {
        self.btree.measure_at::<CharMeasure>(position)
    }

    pub fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_> {
        Slice {
            slice: self.btree.slice(range),
        }
    }

    pub fn cursor_front(&self) -> Cursor<'_> {
        let cursor = self.btree.cursor_front();
        let chunk = &cursor.chunk()[cursor.range()];
        Cursor {
            cursor,
            chunk,
            index: 0,
        }
    }

    pub fn cursor_back(&self) -> Cursor<'_> {
        let cursor = self.btree.cursor_back();
        let chunk = &cursor.chunk()[cursor.range()];
        let index = chunk.len();
        Cursor {
            cursor,
            chunk,
            index,
        }
    }

    pub fn chunks(&self) -> Chunks<'_> {
        Chunks {
            cursor: self.cursor_front(),
        }
    }

    pub fn chunks_rev(&self) -> ChunksRev<'_> {
        ChunksRev {
            cursor: self.cursor_back(),
        }
    }

    pub fn bytes(&self) -> Bytes<'_> {
        Bytes {
            cursor: self.cursor_front(),
        }
    }

    pub fn bytes_rev(&self) -> BytesRev<'_> {
        BytesRev {
            cursor: self.cursor_back(),
        }
    }

    pub fn chars(&self) -> Chars<'_> {
        Chars {
            cursor: self.cursor_front(),
        }
    }

    pub fn chars_rev(&self) -> CharsRev<'_> {
        CharsRev {
            cursor: self.cursor_back(),
        }
    }

    pub fn prepend(&mut self, other: Self) {
        self.btree.prepend(other.btree);
    }

    pub fn append(&mut self, other: Self) {
        self.btree.append(other.btree);
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        Self {
            btree: self.btree.split_off(at),
        }
    }

    pub fn truncate_front(&mut self, end: usize) {
        self.btree.truncate_front(end);
    }

    pub fn truncate_back(&mut self, start: usize) {
        self.btree.truncate_back(start);
    }
}

impl From<String> for BTreeString {
    fn from(string: String) -> Self {
        Self::from(string.as_str())
    }
}

impl From<&String> for BTreeString {
    fn from(string: &String) -> Self {
        Self::from(string.as_str())
    }
}

impl From<&str> for BTreeString {
    fn from(string: &str) -> Self {
        let mut builder = Builder::new();
        builder.push_chunk(string);
        builder.build()
    }
}

pub struct Builder {
    builder: btree::Builder<String>,
    chunk: String,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            builder: btree::Builder::new(),
            chunk: String::new(),
        }
    }

    pub fn push_chunk(&mut self, mut chunk: &str) {
        while !chunk.is_empty() {
            if chunk.len() <= <String as btree::Chunk>::MAX_LEN - self.chunk.len() {
                self.chunk.push_str(chunk);
                break;
            }
            let mut index = <String as btree::Chunk>::MAX_LEN - self.chunk.len();
            while !chunk.is_char_boundary(index) {
                index -= 1;
            }
            let (left_chunk, right_chunk) = chunk.split_at(index);
            self.chunk.push_str(left_chunk);
            chunk = right_chunk;
            self.builder.push_chunk(self.chunk.split_off(0));
        }
    }

    pub fn build(mut self) -> BTreeString {
        self.builder.push_chunk(self.chunk);
        BTreeString {
            btree: self.builder.build(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Slice<'a> {
    slice: btree::Slice<'a, String>,
}

impl<'a> Slice<'a> {
    pub fn is_empty(self) -> bool {
        self.slice.is_empty()
    }

    pub fn len(self) -> usize {
        self.slice.len()
    }

    pub fn char_count(self) -> usize {
        self.slice.measure::<CharMeasure>()
    }

    pub fn char_count_at(&self, position: usize) -> usize {
        self.slice.measure_at::<CharMeasure>(position)
    }

    pub fn cursor_front(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_front();
        let chunk = &cursor.chunk()[cursor.range()];
        Cursor {
            cursor,
            chunk,
            index: 0,
        }
    }

    pub fn cursor_back(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_back();
        let chunk = &cursor.chunk()[cursor.range()];
        let index = chunk.len();
        Cursor {
            cursor,
            chunk,
            index,
        }
    }

    pub fn chunks(self) -> Chunks<'a> {
        Chunks {
            cursor: self.cursor_front(),
        }
    }

    pub fn chunks_rev(self) -> ChunksRev<'a> {
        ChunksRev {
            cursor: self.cursor_back(),
        }
    }

    pub fn bytes(self) -> Bytes<'a> {
        Bytes {
            cursor: self.cursor_front(),
        }
    }

    pub fn bytes_rev(&self) -> BytesRev<'a> {
        BytesRev {
            cursor: self.cursor_back(),
        }
    }

    pub fn chars(self) -> Chars<'a> {
        Chars {
            cursor: self.cursor_front(),
        }
    }

    pub fn chars_rev(self) -> CharsRev<'a> {
        CharsRev {
            cursor: self.cursor_back(),
        }
    }
}

#[derive(Clone)]
pub struct Cursor<'a> {
    cursor: btree::Cursor<'a, String>,
    chunk: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn is_at_start(&self) -> bool {
        self.cursor.is_at_front() && self.index == 0
    }

    pub fn is_at_end(&self) -> bool {
        self.index == self.chunk.len()
    }

    pub fn is_at_char_boundary(&self) -> bool {
        self.chunk.is_char_boundary(self.index)
    }

    pub fn position(&self) -> usize {
        self.cursor.position() + self.index
    }

    pub fn chunk(&self) -> &'a str {
        self.chunk
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn byte(&self) -> u8 {
        self.chunk.as_bytes()[self.index]
    }

    pub fn char(&self) -> char {
        self.chunk[self.index..].chars().next().unwrap()
    }

    pub fn move_next_chunk(&mut self) {
        if self.cursor.is_at_back() {
            self.index = self.chunk.len();
            return;
        }
        self.cursor.move_next_chunk();
        self.chunk = &self.cursor.chunk()[self.cursor.range()];
        self.index = 0;
    }

    pub fn move_prev_chunk(&mut self) {
        if self.index == self.chunk.len() {
            self.index = 0;
            return;
        }
        self.cursor.move_prev_chunk();
        self.chunk = &self.cursor.chunk()[self.cursor.range()];
        self.index = 0;
    }

    pub fn move_next_byte(&mut self) {
        self.index += 1;
        if self.index == self.chunk.len() {
            self.move_next_chunk();
        }
    }

    pub fn move_prev_byte(&mut self) {
        if self.index == 0 {
            self.move_prev_chunk_back();
        }
        self.index -= 1;
    }

    pub fn move_next_char(&mut self) {
        self.index += self.byte().utf8_char_len();
        if self.index == self.chunk.len() {
            self.move_next_chunk();
        }
    }

    pub fn move_prev_char(&mut self) {
        if self.index == 0 {
            self.move_prev_chunk_back();
        }
        self.index -= 1;
        while !self.chunk.is_char_boundary(self.index) {
            self.index -= 1;
        }
    }

    fn move_prev_chunk_back(&mut self) {
        self.cursor.move_prev_chunk();
        self.chunk = &self.cursor.chunk()[self.cursor.range()];
        self.index = self.chunk.len();
    }
}

pub struct Chunks<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_end() {
            return None;
        }
        let chunk = self.cursor.chunk();
        self.cursor.move_next_chunk();
        Some(chunk)
    }
}

pub struct ChunksRev<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for ChunksRev<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_start() {
            return None;
        }
        self.cursor.move_prev_chunk();
        let chunk = self.cursor.chunk();
        Some(chunk)
    }
}

pub struct Bytes<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_end() {
            return None;
        }
        let byte = self.cursor.byte();
        self.cursor.move_next_byte();
        Some(byte)
    }
}

pub struct BytesRev<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for BytesRev<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_start() {
            return None;
        }
        self.cursor.move_prev_byte();
        let byte = self.cursor.byte();
        Some(byte)
    }
}

pub struct Chars<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_end() {
            return None;
        }
        let char = self.cursor.char();
        self.cursor.move_next_char();
        Some(char)
    }
}

pub struct CharsRev<'a> {
    cursor: Cursor<'a>,
}

impl<'a> Iterator for CharsRev<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_start() {
            return None;
        }
        self.cursor.move_prev_char();
        let char = self.cursor.char();
        Some(char)
    }
}

impl btree::Chunk for String {
    type Info = Info;

    const MAX_LEN: usize = 8;

    fn new() -> Self {
        String::new()
    }

    fn len(&self) -> usize {
        self.len()
    }

    fn info_at(&self, index: usize) -> Self::Info {
        Info {
            char_count: self[..index].count_chars(),
        }
    }

    fn is_boundary(&self, index: usize) -> bool {
        self.as_str().is_boundary(index)
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        self.push_str(&other[..end]);
        other.replace_range(..end, "");
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        other.replace_range(..0, &self[start..]);
        self.truncate(start);
    }

    fn truncate_front(&mut self, end: usize) {
        self.replace_range(..end, "");
    }

    fn truncate_back(&mut self, start: usize) {
        self.truncate(start);
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Info {
    char_count: usize,
}

impl btree::Info for Info {
    fn new() -> Self {
        Self { char_count: 0 }
    }
}

impl Add for Info {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            char_count: self.char_count + other.char_count
        }
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for Info {
    type Output = Self;
    
    fn sub(self, other: Self) -> Self::Output {
        Self {
            char_count: self.char_count - other.char_count
        }
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

struct CharMeasure;

impl Measure<String> for CharMeasure {
    fn measure_chunk_at(chunk: &String, index: usize) -> usize {
        chunk[..index].count_chars()
    }

    fn measure_info(info: Info) -> usize {
        info.char_count
    }
}

trait U8Ext {
    fn is_utf8_char_start(self) -> bool;
    fn utf8_char_len(self) -> usize;
}

impl U8Ext for u8 {
    fn is_utf8_char_start(self) -> bool {
        (self as i8) >= -0x40
    }

    fn utf8_char_len(self) -> usize {
        if self < 0x80 {
            1
        } else if self < 0xE0 {
            2
        } else if self < 0xF0 {
            3
        } else {
            4
        }
    }
}

trait StrExt {
    fn count_chars(&self) -> usize;
    fn is_boundary(&self, index: usize) -> bool;
}

impl StrExt for str {
    fn count_chars(&self) -> usize {
        let mut count = 0;
        for byte in self.bytes() {
            if byte.is_utf8_char_start() {
                count += 1;
            }
        }
        count
    }

    fn is_boundary(&self, index: usize) -> bool {
        if index == 0 || index == self.len() {
            return true;
        }
        let bytes = self.as_bytes();
        bytes[index].is_utf8_char_start() && bytes[index - 1] != 0x0D && bytes[index] != 0x0F
    }
}

#[cfg(test)]
mod tests {
    use {super::*, proptest::prelude::*, std::ops::Range};

    fn string_and_index() -> impl Strategy<Value = (String, usize)> {
        any::<String>().prop_flat_map(|string| {
            {
                let string_len = string.len();
                (Just(string), 0..=string_len)
            }
            .prop_map(|(string, mut index)| {
                while !string.is_char_boundary(index) {
                    index -= 1;
                }
                (string, index)
            })
        })
    }

    fn string_and_range() -> impl Strategy<Value = (String, Range<usize>)> {
        string_and_index()
            .prop_flat_map(|(string, end)| (Just(string), 0..=end, Just(end)))
            .prop_map(|(string, mut start, end)| {
                while !string.is_char_boundary(start) {
                    start -= 1;
                }
                (string, start..end)
            })
    }
    
    fn string_and_range_and_index() -> impl Strategy<Value = (String, Range<usize>, usize)> {
        string_and_range().prop_flat_map(|(string, range)| {
            let range_len = range.len();
            (Just(string), Just(range), 0..=range_len)
        })
        .prop_map(|(string, range, mut index)| {
            let slice = &string[range.clone()];
            while !slice.is_char_boundary(index) {
                index -= 1;
            }
            (string, range, index)
        })
    }

    proptest! {
        #[test]
        fn test_is_empty(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.is_empty(), string.is_empty());
        }

        #[test]
        fn test_len(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.len(), string.len());
        }

        #[test]
        fn test_char_count(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.char_count(), string.chars().count());
        }

        #[test]
        fn test_char_count_at((string, index) in string_and_index()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.char_count_at(index), string[..index].chars().count());
        }

        #[test]
        fn test_chunks(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn test_chunks_rev(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string
                    .chunks_rev()
                    .map(|chunk| chunk.chars().rev().collect::<String>())
                    .collect::<String>(),
                string.chars().rev().collect::<String>(),
            );
        }

        #[test]
        fn test_bytes(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.bytes().collect::<Vec<_>>(),
                string.bytes().collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_bytes_rev(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.bytes_rev().collect::<Vec<_>>(),
                string.bytes().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_chars(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.chars().collect::<Vec<_>>(),
                string.chars().collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_chars_rev(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.chars_rev().collect::<Vec<_>>(),
                string.chars().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_prepend(mut string in any::<String>(), other_string in any::<String>()) {
            let mut btree_string = BTreeString::from(&string);
            btree_string.prepend(BTreeString::from(&other_string));
            string.replace_range(..0, &other_string);
            assert_eq!(btree_string.chars().collect::<String>(), string);
        }

        #[test]
        fn test_append(mut string in any::<String>(), other_string in any::<String>()) {
            let mut btree_string = BTreeString::from(&string);
            btree_string.append(BTreeString::from(&other_string));
            string.push_str(&other_string);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn test_split_off((mut string, at) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            let string_2 = string.split_off(at);
            let btree_string_2 = btree_string.split_off(at);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
            assert_eq!(btree_string_2.chunks().collect::<String>(), string_2);
        }

        #[test]
        fn test_truncate_front((mut string, end) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            string.replace_range(..end, "");
            btree_string.truncate_front(end);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn test_truncate_back((mut string, start) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            string.truncate(start);
            btree_string.truncate_back(start);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn test_slice_is_empty((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.is_empty(), slice.is_empty());
        }

        #[test]
        fn test_slice_len((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.len(), slice.len());
        }

        #[test]
        fn test_slice_char_count((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.char_count(), slice.count_chars());
        }

        #[test]
        fn test_slice_char_count_at((string, range, index) in string_and_range_and_index()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.char_count_at(index), slice[..index].count_chars());
        }

        #[test]
        fn test_slice_chunks((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.chunks().collect::<String>(), slice);
        }

        #[test]
        fn test_slice_chunks_rev((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(
                btree_slice
                    .chunks_rev()
                    .map(|chunk| chunk.chars().rev().collect::<String>())
                    .collect::<String>(),
                slice.chars().rev().collect::<String>(),
            );
        }

        #[test]
        fn test_slice_bytes((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.bytes().collect::<Vec<_>>(), slice.bytes().collect::<Vec<_>>());
        }

        #[test]
        fn test_slice_bytes_rev((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(
                btree_slice.bytes_rev().collect::<Vec<_>>(),
                slice.bytes().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn test_slice_chars((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.chars().collect::<Vec<_>>(), slice.chars().collect::<Vec<_>>());
        }

        #[test]
        fn test_slice_chars_rev((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(
                btree_slice.chars_rev().collect::<Vec<_>>(),
                slice.chars().rev().collect::<Vec<_>>()
            );
        }
    }
}
