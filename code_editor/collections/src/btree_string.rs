use {
    crate::{btree, BTree},
    std::ops::{AddAssign, SubAssign},
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

    pub fn cursor_front(&self) -> Cursor<'_> {
        let cursor = self.btree.cursor_front();
        let chunk = &cursor.chunk();
        Cursor {
            cursor,
            chunk,
            index: 0,
        }
    }

    pub fn cursor_back(&self) -> Cursor<'_> {
        let cursor = self.btree.cursor_back();
        let chunk = &cursor.chunk();
        Cursor {
            cursor,
            chunk,
            index: 0,
        }
    }

    pub fn bytes(&self) -> Bytes<'_> {
        Bytes {
            string: self,
            cursor_front: None,
            cursor_back: None,
        }
    }

    pub fn chars(&self) -> Chars<'_> {
        Chars {
            string: self,
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
        BTreeString { btree: self.builder.build() }
    }
}

pub struct Cursor<'a> {
    cursor: btree::Cursor<'a, String>,
    chunk: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn is_at_start(&self) -> bool {
        self.cursor.is_at_start() && self.index == 0
    }

    pub fn is_at_end(&self) -> bool {
        self.cursor.is_at_end()
    }

    pub fn is_at_char_boundary(&self) -> bool {
        self.chunk.is_char_boundary(self.index)
    }

    pub fn position(&self) -> usize {
        self.cursor.position() + self.index
    }

    pub fn chunk(&self) -> &str {
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
        self.cursor.move_next_chunk();
        self.chunk = &self.cursor.chunk();
        self.index = 0;
    }

    pub fn move_prev_chunk(&mut self) {
        self.cursor.move_prev_chunk();
        self.chunk = &self.cursor.chunk();
        self.index = self.chunk.len();
    }

    pub fn move_next_byte(&mut self) {
        self.index += 1;
        if self.index == self.chunk.len() {
            self.cursor.move_next_chunk();
        }
    }

    pub fn move_prev_byte(&mut self) {
        if self.index == 0 {
            self.cursor.move_prev_chunk();
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
            self.move_prev_chunk();
        }
        self.index -= 1;
        while !self.chunk.is_char_boundary(self.index) {
            self.index -= 1;
        }
    }
}

pub struct Bytes<'a> {
    string: &'a BTreeString,
    cursor_front: Option<Cursor<'a>>,
    cursor_back: Option<Cursor<'a>>,
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor_front = self
            .cursor_front
            .get_or_insert_with(|| self.string.cursor_front());
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
            .get_or_insert_with(|| self.string.cursor_back());
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
    string: &'a BTreeString,
    cursor_front: Option<Cursor<'a>>,
    cursor_back: Option<Cursor<'a>>,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        let cursor_front = self
            .cursor_front
            .get_or_insert_with(|| self.string.cursor_front());
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
            .get_or_insert_with(|| self.string.cursor_back());
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
    use {proptest::prelude::*, super::*};

    proptest! {
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
    }
}