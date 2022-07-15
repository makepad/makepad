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
            btree: BTree::new()
        }
    }
    
    pub fn prepend(&mut self, other: Self) {
        self.btree.prepend(other.btree);
    }

    pub fn append(&mut self, other: Self) {
        self.btree.append(other.btree);
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

    fn move_left(&mut self, other: &mut Self, end: usize) {
        self.0.push_str(&other.0[..end]);
        other.0.replace_range(..end, "");
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        other.0.replace_range(..0, &self.0[start..]);
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
