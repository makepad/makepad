use {
    crate::{btree, BTree},
    std::ops::{Add, AddAssign, RangeBounds, Sub, SubAssign},
};

#[derive(Clone)]
pub struct BTreeString {
    btree: BTree<String, Info>,
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

    pub fn char_len(&self) -> usize {
        self.btree.info().char_count
    }

    pub fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_> {
        Slice {
            slice: self.btree.slice(range),
        }
    }

    pub fn cursor_front(&self) -> Cursor<'_> {
        self.slice(..).cursor_front()
    }

    pub fn cursor_back(&self) -> Cursor<'_> {
        self.slice(..).cursor_back()
    }

    pub fn chunks(&self) -> Chunks<'_> {
        self.slice(..).chunks()
    }

    pub fn bytes(&self) -> Bytes<'_> {
        self.slice(..).bytes()
    }

    pub fn chars(&self) -> Chars<'_> {
        self.slice(..).chars()
    }

    pub fn append(&mut self, other: Self) {
        self.btree.append(other.btree);
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        Self {
            btree: self.btree.split_off(at),
        }
    }

    pub fn truncate_front(&mut self, start: usize) {
        self.btree.truncate_front(start)
    }

    pub fn truncate_back(&mut self, end: usize) {
        self.btree.truncate_back(end)
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
    builder: btree::Builder<String, Info>,
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
    slice: btree::Slice<'a, String, Info>,
}

impl<'a> Slice<'a> {
    pub fn is_empty(&self) -> bool {
        self.slice.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slice.len()
    }

    pub fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_> {
        Slice {
            slice: self.slice.slice(range),
        }
    }

    pub fn cursor_front(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_front();
        let (current, range) = cursor.current();
        Cursor {
            cursor,
            current: &current[range],
            index: 0,
        }
    }

    pub fn cursor_back(self) -> Cursor<'a> {
        let cursor = self.slice.cursor_back();
        let (current, range) = cursor.current();
        let current = &current[range];
        let index = current.len();
        Cursor {
            cursor,
            current,
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
    cursor: btree::Cursor<'a, String, Info>,
    current: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
    pub fn is_at_front(&self) -> bool {
        self.index == 0 && self.cursor.is_at_front()
    }

    pub fn is_at_back(&self) -> bool {
        self.index == self.current.len()
    }

    pub fn is_at_char_boundary(&self) -> bool {
        self.current.is_char_boundary(self.index)
    }

    pub fn position(&self) -> usize {
        self.cursor.position() + self.index
    }

    pub fn current_chunk(&self) -> &'a str {
        self.current
    }

    pub fn current_byte(&self) -> u8 {
        self.current.as_bytes()[self.index]
    }

    pub fn current_char(&self) -> char {
        self.current[self.index..].chars().next().unwrap()
    }

    pub fn move_next_chunk(&mut self) {
        if self.cursor.is_at_back() {
            self.index = self.current.len();
            return;
        }
        self.move_next();
        self.index = 0;
    }

    pub fn move_prev_chunk(&mut self) {
        if self.index == self.current.len() {
            self.index = 0;
            return;
        }
        self.move_prev();
        self.index = 0;
    }

    pub fn move_next_byte(&mut self) {
        self.index += 1;
        if self.index == self.current.len() && !self.cursor.is_at_back() {
            self.move_next();
            self.index = 0;
        }
    }

    pub fn move_prev_byte(&mut self) {
        if self.index == 0 {
            self.move_prev();
            self.index = self.current.len();
        }
        self.index -= 1;
    }

    pub fn move_next_char(&mut self) {
        self.index += self.current_byte().utf8_char_len();
        if self.index == self.current.len() && !self.cursor.is_at_back() {
            self.move_next();
            self.index = 0;
        }
    }

    pub fn move_prev_char(&mut self) {
        if self.index == 0 {
            self.move_prev();
            self.index = self.current.len();
        }
        self.index -= 1;
        while !self.is_at_char_boundary() {
            self.index -= 1;
        }
    }

    fn move_next(&mut self) {
        self.cursor.move_next();
        let (current, range) = self.cursor.current();
        self.current = &current[range];
    }

    fn move_prev(&mut self) {
        self.cursor.move_prev();
        let (current, range) = self.cursor.current();
        self.current = &current[range];
    }
}

#[derive(Clone)]
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
            || cursor_front.is_at_back(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        let chunk = cursor_front.current_chunk();
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
            || cursor_back.is_at_front(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_chunk();
        Some(cursor_back.current_chunk())
    }
}

#[derive(Clone)]
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
            || cursor_front.is_at_back(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        let byte = cursor_front.current_byte();
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
            || cursor_back.is_at_front(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_byte();
        Some(cursor_back.current_byte())
    }
}

#[derive(Clone)]
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
            || cursor_front.is_at_back(),
            |cursor_back| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        let byte = cursor_front.current_char();
        cursor_front.move_next_char();
        Some(byte)
    }
}

impl<'a> DoubleEndedIterator for Chars<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let cursor_back = self
            .cursor_back
            .get_or_insert_with(|| self.slice.cursor_back());
        if self.cursor_front.as_ref().map_or_else(
            || cursor_back.is_at_front(),
            |cursor_front| cursor_front.position() == cursor_back.position(),
        ) {
            return None;
        }
        cursor_back.move_prev_char();
        Some(cursor_back.current_char())
    }
}

impl btree::Chunk for String {
    const MAX_LEN: usize = 8;

    fn len(&self) -> usize {
        self.len()
    }

    fn is_boundary(&self, index: usize) -> bool {
        self.is_char_boundary(index)
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        self.push_str(&other[..end]);
        other.replace_range(..end, "");
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        other.replace_range(..0, &self[start..]);
        self.truncate(start);
    }

    fn truncate_front(&mut self, start: usize) {
        self.replace_range(..start, "");
    }

    fn truncate_back(&mut self, end: usize) {
        self.truncate(end)
    }
}

#[derive(Clone, Copy)]
pub struct Info {
    char_count: usize,
}

impl btree::Info<String> for Info {
    fn from_chunk(string: &String) -> Self {
        Self {
            char_count: string.count_chars(),
        }
    }
}

impl Add for Info {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            char_count: self.char_count + other.char_count,
        }
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Default for Info {
    fn default() -> Self {
        Self { char_count: 0 }
    }
}

impl Sub for Info {
    type Output = Self;

    fn sub(self, other: Self) -> Self::Output {
        Self {
            char_count: self.char_count - other.char_count,
        }
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

trait U8Ext {
    fn is_utf8_char_boundary(self) -> bool;

    fn utf8_char_len(self) -> usize;
}

impl U8Ext for u8 {
    fn is_utf8_char_boundary(self) -> bool {
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
}

impl StrExt for str {
    fn count_chars(&self) -> usize {
        let mut count = 0;
        for byte in self.bytes() {
            if byte.is_utf8_char_boundary() {
                count += 1;
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use {super::*, proptest::prelude::*};

    fn string() -> impl Strategy<Value = String> {
        ".*"
    }

    fn string_and_index() -> impl Strategy<Value = (String, usize)> {
        string().prop_flat_map(|string| {
            {
                let len = string.len();
                (Just(string), 0..=len)
            }
            .prop_map(|(string, mut index)| {
                while !string.is_char_boundary(index) {
                    index -= 1;
                }
                (string, index)
            })
        })
    }

    proptest! {
        #[test]
        fn is_empty(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.is_empty(), string.is_empty());
        }

        #[test]
        fn len(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.len(), string.len());
        }

        #[test]
        fn test_char_len(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.char_len(), string.count_chars());
        }

        #[test]
        fn chunks(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn chunks_rev(string in string()) {
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
        fn bytes(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.bytes().collect::<Vec<_>>(),
                string.bytes().collect::<Vec<_>>()
            );
        }

        #[test]
        fn bytes_rev(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.bytes().rev().collect::<Vec<_>>(),
                string.bytes().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn chars(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.chars().collect::<Vec<_>>(),
                string.chars().collect::<Vec<_>>()
            );
        }

        #[test]
        fn chars_rev(string in string()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.chars().rev().collect::<Vec<_>>(),
                string.chars().rev().collect::<Vec<_>>()
            );
        }

        #[test]
        fn append(mut string in string(), other_string in string()) {
            let mut btree_string = BTreeString::from(&string);
            btree_string.append(BTreeString::from(&other_string));
            string.push_str(&other_string);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn split_off((mut string, at) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            let other_string = string.split_off(at);
            let other_btree_string = btree_string.split_off(at);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
            assert_eq!(other_btree_string.chunks().collect::<String>(), other_string);
        }

        #[test]
        fn truncate_front((mut string, start) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            string.replace_range(..start, "");
            btree_string.truncate_front(start);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }

        #[test]
        fn truncate_back((mut string, end) in string_and_index()) {
            let mut btree_string = BTreeString::from(&string);
            string.truncate(end);
            btree_string.truncate_back(end);
            assert_eq!(btree_string.chunks().collect::<String>(), string);
        }
    }
}
