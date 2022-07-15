use {crate::{btree, BTree}, std::{iter::Sum, ops::{AddAssign, SubAssign}}};

pub struct BTreeString {
    btree: BTree<Chunk>,
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
    char_count: usize
}

impl Info {
    fn new() -> Self {
        Info {
            char_count: 0
        }
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

impl Sum for Info {
    fn sum<I>(iter: I) -> Self
    where
        I: Iterator<Item = Self>
    {
        let mut summed_info = Info::new();
        for info in iter {
            summed_info += info;
        }
        summed_info
    }
}