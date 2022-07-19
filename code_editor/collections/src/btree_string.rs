use {
    crate::{btree, BTree},
    std::ops::{AddAssign, SubAssign},
};

pub struct BTreeString {
    btree: BTree<Chunk>,
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

pub struct Cursor<'a> {
    cursor: btree::Cursor<'a, Chunk>,
    chunk: &'a str,
    index: usize,
}

impl<'a> Cursor<'a> {
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
        self.chunk = &self.cursor.chunk().0;
    }

    pub fn move_prev_chunk(&mut self) {
        self.cursor.move_prev_chunk();
        self.chunk = &self.cursor.chunk().0;
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
        while self.chunk.is_char_boundary(self.index) {
            self.index -= 1;
        }
    }
}

#[derive(Clone)]
struct Chunk(String);

impl btree::Chunk for Chunk {
    type Info = Info;

    const MAX_LEN: usize = 1024;

    fn new() -> Self {
        Chunk(String::new())
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn info(&self) -> Self::Info {
        Info {
            char_count: self.0.chars().count(),
        }
    }

    fn can_split_at(&self, index: usize) -> bool {
        self.0.is_char_boundary(index)
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        self.0.push_str(&other.0[..end]);
        other.0.replace_range(..end, "");
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        other.0.replace_range(..0, &self.0[start..]);
        self.0.truncate(start);
    }

    fn truncate_front(&mut self, end: usize) {
        self.0.replace_range(..end, "");
    }

    fn truncate_back(&mut self, start: usize) {
        self.0.truncate(start);
    }
}

#[derive(Clone, Copy)]
struct Info {
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