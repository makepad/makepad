use {
    crate::{btree, BTree},
    std::ops::{AddAssign, RangeBounds, SubAssign},
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

    pub fn len(&self) -> usize {
        self.btree.len()
    }

    pub fn char_count(&self) -> usize {
        self.btree.info().char_count
    }

    pub fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_> {
        Slice {
            slice: self.btree.slice(range)
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
            slice: self.slice(..),
            cursor_front: None,
            cursor_back: None,
        }
    }

    pub fn bytes(&self) -> Bytes<'_> {
        Bytes {
            slice: self.slice(..),
            cursor_front: None,
            cursor_back: None,
        }
    }

    pub fn chars(&self) -> Chars<'_> {
        Chars {
            slice: self.slice(..),
            cursor_front: None,
            cursor_back: None,
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

    pub fn cursor_front(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_front();
        let chunk = &cursor.chunk()[cursor.range()];
        let index = cursor.start() - cursor.position();
        Cursor {
            cursor,
            chunk,
            index,
        }
    }

    pub fn cursor_back(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_back();
        let chunk = &cursor.chunk()[cursor.range()];
        let index = cursor.end() - cursor.position();
        Cursor {
            cursor,
            chunk,
            index,
        }
    }

    pub fn chunks(self) -> Chunks<'a> {
        Chunks {
            slice: self,
            cursor_front: None,
            cursor_back: None,
        }
    }

    pub fn bytes(self) -> Bytes<'a> {
        Bytes {
            slice: self,
            cursor_front: None,
            cursor_back: None,
        }
    }

    pub fn chars(self) -> Chars<'a> {
        Chars {
            slice: self,
            cursor_front: None,
            cursor_back: None,
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
        self.cursor.is_at_back() && self.index == self.chunk.len()
    }

    pub fn is_at_char_boundary(&self) -> bool {
        self.chunk.is_char_boundary(self.index)
    }

    pub fn position(&self) -> usize {
        (self.cursor.position() + self.index) - self.cursor.start()
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
        if self.is_at_end() {
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
        self.index += len_utf8_from_first_byte(self.byte());
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
    slice: Slice<'a>,
    cursor_front: Option<Cursor<'a>>,
    cursor_back: Option<Cursor<'a>>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor_front = self
            .cursor_front
            .get_or_insert_with(|| self.slice.cursor_front());
        if self.cursor_back.as_ref().map_or_else(
            || cursor_front.is_at_end(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        };
        let chunk = cursor_front.chunk();
        cursor_front.move_next_chunk();
        Some(chunk)
    }
}

impl<'a> DoubleEndedIterator for Chunks<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let cursor_back = self
            .cursor_back
            .get_or_insert_with(|| self.slice.cursor_back());
        if self.cursor_front.as_ref().map_or_else(
            || cursor_back.is_at_start(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_chunk();
        let chunk = cursor_back.chunk();
        Some(chunk)
    }
}

pub struct Bytes<'a> {
    slice: Slice<'a>,
    cursor_front: Option<Cursor<'a>>,
    cursor_back: Option<Cursor<'a>>,
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor_front = self
            .cursor_front
            .get_or_insert_with(|| self.slice.cursor_front());
        if self.cursor_back.as_ref().map_or_else(
            || cursor_front.is_at_end(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        let byte = cursor_front.byte();
        cursor_front.move_next_byte();
        Some(byte)
    }
}

impl<'a> DoubleEndedIterator for Bytes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let cursor_back = self
            .cursor_back
            .get_or_insert_with(|| self.slice.cursor_back());
        if self.cursor_front.as_ref().map_or_else(
            || cursor_back.is_at_start(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_byte();
        let byte = cursor_back.byte();
        Some(byte)
    }
}

pub struct Chars<'a> {
    slice: Slice<'a>,
    cursor_front: Option<Cursor<'a>>,
    cursor_back: Option<Cursor<'a>>,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor_front = self
            .cursor_front
            .get_or_insert_with(|| self.slice.cursor_front());
        if self.cursor_back.as_ref().map_or_else(
            || cursor_front.is_at_end(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        let char = cursor_front.char();
        cursor_front.move_next_char();
        Some(char)
    }
}

impl<'a> DoubleEndedIterator for Chars<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let cursor_back = self
            .cursor_back
            .get_or_insert_with(|| self.slice.cursor_back());
        if self.cursor_front.as_ref().map_or_else(
            || cursor_back.is_at_start(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_char();
        let char = cursor_back.char();
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

    fn info(&self) -> Self::Info {
        Info {
            char_count: self.chars().count(),
        }
    }

    fn can_split_at(&self, index: usize) -> bool {
        self.is_char_boundary(index)
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

impl AddAssign for Info {
    fn add_assign(&mut self, other: Self) {
        self.char_count += other.char_count;
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Self) {
        self.char_count -= other.char_count;
    }
}

fn len_utf8_from_first_byte(byte: u8) -> usize {
    if byte < 0x80 {
        1
    } else if byte < 0xE0 {
        2
    } else if byte < 0xF0 {
        3
    } else {
        4
    }
}

#[cfg(test)]
mod tests {
    use {std::ops::Range, super::*, proptest::prelude::*};

    fn string_and_index() -> impl Strategy<Value = (String, usize)> {
        any::<String>().prop_flat_map(|string| {
            let string_len = string.len();
            (Just(string), 0..=string_len)
        }.prop_map(|(string, mut index)| {
            while !string.is_char_boundary(index) {
                index -= 1;
            }
            (string, index)
        }))
    }

    fn string_and_range() -> impl Strategy<Value = (String, Range<usize>)> {
        string_and_index().prop_flat_map(|(string, end)| {
            (Just(string), 0..=end, Just(end))
        }).prop_map(|(string, mut start, end)| {
            while !string.is_char_boundary(start) {
                start -= 1;
            }
            (string, start..end)
        })
    }

    proptest! {
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
        fn test_chunks(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn test_chunks_rev(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string
                    .chunks()
                    .rev()
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
                btree_string.bytes().rev().collect::<Vec<_>>(),
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
                btree_string.chars().rev().collect::<Vec<_>>(),
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
    }
}
