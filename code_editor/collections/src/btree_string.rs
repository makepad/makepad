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

    pub fn char_len(&self) -> usize {
        self.btree.measured_len::<CharMeasure>()
    }

    pub fn line_len(&self) -> usize {
        self.btree.measured_len::<LineBreakMeasure>() + 1
    }

    pub fn to_char_index(&self, index: usize) -> usize {
        if index == 0 {
            return 0;
        }
        if index == self.len() {
            return self.char_len();
        }
        self.btree.to_measured_index::<CharMeasure>(index)
    }

    pub fn to_line_index(&self, index: usize) -> usize {
        if index == 0 {
            return 0;
        }
        if index == self.len() {
            return self.line_len() - 1;
        }
        self.btree.to_measured_index::<LineBreakMeasure>(index)
    }

    pub fn from_char_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        if char_index == self.char_len() {
            return self.len();
        }
        self.btree.from_measured_index::<CharMeasure>(char_index)
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
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
        }
    }

    pub fn bytes(&self) -> Bytes<'_> {
        Bytes {
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
        }
    }

    pub fn chars(&self) -> Chars<'_> {
        Chars {
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
        }
    }

    pub fn prepend(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        let chunk_0 = other.cursor_back().chunk();
        let mut start = chunk_0.len() - 1;
        while !chunk_0.is_boundary(start) {
            start -= 1;
        }
        let chunk_1 = self.cursor_front().chunk();
        let mut end = 1;
        while !chunk_1.is_boundary(end) {
            end += 1;
        }
        let btree = BTree::from([&chunk_0[start..], &chunk_1[..end]].join(""));
        other.btree.truncate_back(other.len() - (chunk_0.len() - start));
        self.btree.truncate_front(end);
        self.btree.prepend(btree);
        self.btree.prepend(other.btree);
    }

    pub fn append(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        let chunk_0 = self.cursor_back().chunk();
        let mut start = chunk_0.len() - 1;
        while !chunk_0.is_boundary(start) {
            start -= 1;
        }
        let chunk_1 = other.cursor_front().chunk();
        let mut end = 1;
        while !chunk_1.is_boundary(end) {
            end += 1;
        }
        let btree = BTree::from([&chunk_0[start..], &chunk_1[..end]].join(""));
        other.btree.truncate_front(end);
        self.btree.truncate_back(self.len() - (chunk_0.len() - start));
        self.btree.append(btree);
        self.btree.append(other.btree);
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        use std::mem;

        if at == 0 {
            return mem::replace(self, Self::new());
        }
        if at == self.len() {
            return Self::new();
        }
        Self {
            btree: self.btree.split_off(at),
        }
    }

    pub fn truncate_front(&mut self, end: usize) {
        if end == 0 {
            return;
        }
        if end == self.len() {
            *self = Self::new();
            return;
        }
        self.btree.truncate_front(end);
    }

    pub fn truncate_back(&mut self, start: usize) {
        if start == 0 {
            *self = Self::new();
            return;
        }
        if start == self.len() {
            return;
        }
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
            let mut end = self.chunk.len();
            if *self.chunk.as_bytes().last().unwrap() == 0x0D
                && *chunk.as_bytes().first().unwrap() == 0x0A
            {
                end -= 1;
            }
            self.builder
                .push_chunk(self.chunk.drain(..end).collect::<String>());
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

    pub fn char_len(self) -> usize {
        self.slice.measured_len::<CharMeasure>()
    }

    pub fn line_len(self) -> usize {
        self.slice.measured_len::<LineBreakMeasure>() + 1
    }

    pub fn to_char_index(&self, index: usize) -> usize {
        if index == 0 {
            return 0;
        }
        if index == self.len() {
            return self.char_len();
        }
        self.slice.to_measured_index::<CharMeasure>(index)
    }

    pub fn to_line_index(&self, index: usize) -> usize {
        if index == 0 {
            return 0;
        }
        if index == self.len() {
            return self.line_len() - 1;
        }
        self.slice.to_measured_index::<LineBreakMeasure>(index)
    }

    pub fn from_char_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        if char_index == self.char_len() {
            return self.len();
        }
        self.slice.from_measured_index::<CharMeasure>(char_index)
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
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
        }
    }

    pub fn bytes(self) -> Bytes<'a> {
        Bytes {
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
        }
    }

    pub fn chars(self) -> Chars<'a> {
        Chars {
            cursor_front: self.cursor_front(),
            cursor_back: self.cursor_back(),
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
            self.move_prev_back_chunk();
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
            self.move_prev_back_chunk();
        }
        self.index -= 1;
        while !self.chunk.is_char_boundary(self.index) {
            self.index -= 1;
        }
    }

    fn move_prev_back_chunk(&mut self) {
        self.cursor.move_prev_chunk();
        self.chunk = &self.cursor.chunk()[self.cursor.range()];
        self.index = self.chunk.len();
    }
}

pub struct Chunks<'a> {
    cursor_front: Cursor<'a>,
    cursor_back: Cursor<'a>,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        let chunk = self.cursor_front.chunk();
        self.cursor_front.move_next_chunk();
        Some(chunk)
    }
}

impl<'a> DoubleEndedIterator for Chunks<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        self.cursor_back.move_prev_chunk();
        let chunk = self.cursor_back.chunk();
        Some(chunk)
    }
}

pub struct Bytes<'a> {
    cursor_front: Cursor<'a>,
    cursor_back: Cursor<'a>,
}

impl<'a> Iterator for Bytes<'a> {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        let byte = self.cursor_front.byte();
        self.cursor_front.move_next_byte();
        Some(byte)
    }
}

impl<'a> DoubleEndedIterator for Bytes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        self.cursor_back.move_prev_byte();
        let byte = self.cursor_back.byte();
        Some(byte)
    }
}

pub struct Chars<'a> {
    cursor_front: Cursor<'a>,
    cursor_back: Cursor<'a>,
}

impl<'a> Iterator for Chars<'a> {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        let char = self.cursor_front.char();
        self.cursor_front.move_next_char();
        Some(char)
    }
}

impl<'a> DoubleEndedIterator for Chars<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.cursor_front.position() == self.cursor_back.position() {
            return None;
        }
        self.cursor_back.move_prev_char();
        let char = self.cursor_back.char();
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

    fn summed_info_at(&self, index: usize) -> Self::Info {
        Info {
            char_count: self[..index].count_chars(),
            line_break_count: self[..index].count_line_breaks(),
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
    line_break_count: usize,
}

impl btree::Info for Info {
    fn new() -> Self {
        Self {
            char_count: 0,
            line_break_count: 0,
        }
    }
}

impl Add for Info {
    type Output = Self;

    fn add(self, other: Self) -> Self::Output {
        Self {
            char_count: self.char_count + other.char_count,
            line_break_count: self.line_break_count + other.line_break_count,
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
            char_count: self.char_count - other.char_count,
            line_break_count: self.line_break_count - other.line_break_count,
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
    fn to_measured_index(chunk: &String, index: usize) -> usize {
        chunk[..index].count_chars()
    }

    fn from_measured_index(chunk: &String, measured_index: usize) -> usize {
        chunk.char_indices().nth(measured_index).map_or(chunk.len(), |(index, _)| index)
    }

    fn from_info(info: Info) -> usize {
        info.char_count
    }
}

struct LineBreakMeasure;

impl Measure<String> for LineBreakMeasure {
    fn to_measured_index(chunk: &String, index: usize) -> usize {
        chunk[..index].count_line_breaks()
    }

    fn from_measured_index(_chunk: &String, _measured_index: usize) -> usize {
        unimplemented!()
    }

    fn from_info(info: Info) -> usize {
        info.line_break_count
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
    fn count_line_breaks(&self) -> usize;
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

    fn count_line_breaks(&self) -> usize {
        let mut count = 0;
        let bytes = self.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte >= 0x0A && byte <= 0x0D {
                count += 1;
                if byte == 0x0D && index + 1 < bytes.len() && bytes[index + 1] == 0x0A {
                    index += 2;
                } else {
                    index += 1;
                };
            } else if byte == 0xC2 && index + 1 < bytes.len() && bytes[index + 1] == 0x85 {
                count += 1;
                index += 2;
            } else if byte == 0xE2
                && index + 2 < bytes.len()
                && bytes[index + 1] == 0x80
                && bytes[index + 2] >> 1 == 0x54
            {
                count += 1;
                index += 3;
            } else {
                index += 1;
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

    fn string_and_char_index() -> impl Strategy<Value = (String, usize)> {
        any::<String>().prop_flat_map(|string| {
            let char_len = string.chars().count();
            (Just(string), 0..=char_len)
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
        string_and_range()
            .prop_flat_map(|(string, range)| {
                let len = range.len();
                (Just(string), Just(range), 0..=len)
            })
            .prop_map(|(string, range, mut index)| {
                let slice = &string[range.clone()];
                while !slice.is_char_boundary(index) {
                    index -= 1;
                }
                (string, range, index)
            })
    }

    fn string_and_range_and_char_index() -> impl Strategy<Value = (String, Range<usize>, usize)> {
        string_and_range()
            .prop_flat_map(|(string, range)| {
                let char_len = string[range.clone()].chars().count();
                (Just(string), Just(range), 0..=char_len)
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
        fn test_char_len(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.char_len(), string.count_chars());
        }

        #[test]
        fn test_line_len(string in any::<String>()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.line_len(), string.count_line_breaks() + 1);
        }

        #[test]
        fn test_to_char_index((string, index) in string_and_index()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.to_char_index(index), string[..index].chars().count());
        }

        #[test]
        fn test_to_line_index((string, index) in string_and_index()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(btree_string.to_line_index(index), string[..index].count_line_breaks());
        }

        #[test]
        fn test_from_char_index((string, char_index) in string_and_char_index()) {
            let btree_string = BTreeString::from(&string);
            assert_eq!(
                btree_string.from_char_index(char_index),
                string.char_indices().nth(char_index).map_or(string.len(), |(index, _)| index),
            );
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
        fn test_slice_char_len((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.char_len(), slice.count_chars());
        }

        #[test]
        fn test_slice_line_len((string, range) in string_and_range()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.line_len(), slice.count_line_breaks() + 1);
        }

        #[test]
        fn test_slice_to_char_index((string, range, index) in string_and_range_and_index()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.to_char_index(index), slice[..index].count_chars());
        }

        #[test]
        fn test_slice_to_line_index((string, range, index) in string_and_range_and_index()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(btree_slice.to_line_index(index), slice[..index].count_line_breaks());
        }

        #[test]
        fn test_slice_from_char_index((string, range, char_index) in string_and_range_and_char_index()) {
            let btree_string = BTreeString::from(&string);
            let slice = &string[range.clone()];
            let btree_slice = btree_string.slice(range);
            assert_eq!(
                btree_slice.from_char_index(char_index),
                slice.char_indices().nth(char_index).map_or(slice.len(), |(index, _)| index),
            )
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
                    .chunks()
                    .rev()
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
                btree_slice.bytes().rev().collect::<Vec<_>>(),
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
                btree_slice.chars().rev().collect::<Vec<_>>(),
                slice.chars().rev().collect::<Vec<_>>()
            );
        }
    }
}
