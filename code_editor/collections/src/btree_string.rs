use {crate::{btree, BTree}, std::ops::{AddAssign, SubAssign}};

pub struct BTreeString {
    btree: BTree<Chunk>,
}

impl BTreeString {
    pub fn new() -> Self {
        Self {
            btree: BTree::<Chunk>::new(),
        }
    }
}

#[derive(Clone)]
pub struct Chunk(String);

impl btree::Chunk for Chunk {
    type Info = Info;

    const MAX_LEN: usize = 1024;

    fn new() -> Self {
        unimplemented!()
    }

    fn is_empty(&self) -> bool {
        unimplemented!()
    }
    
    fn len(&self) -> usize {
        unimplemented!()
    }

    fn info(&self) -> Self::Info {
        unimplemented!()
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        unimplemented!()
    }

    fn move_right(&mut self, other: &mut Self, end: usize) {
        unimplemented!()
    }
}

#[derive(Clone, Copy)]
struct Info;

impl btree::Info for Info {
    fn new() -> Self {
        unimplemented!()
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, other: Self) {
        unimplemented!()
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Self) {
        unimplemented!()
    }
}
