use super::Chunk;

#[derive(Clone)]
pub struct Leaf<T> {
    chunk: T,
}

impl<T: Chunk> Leaf<T> {
    const MAX_LEN: usize = T::MAX_LEN;

    pub fn new() -> Self {
        Self {
            chunk: Chunk::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chunk.is_empty()
    }

    pub fn len(&self) -> usize {
        self.chunk.len()
    }

    pub fn info(&self) -> T::Info {
        self.chunk.info()
    }

    pub fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            other.move_right(self, self.len());
            return None;
        }
        other.distribute(self);
        Some(other)
    }

    pub fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            let other_len = other.len();
            self.move_left(&mut other, other_len);
            return None;
        }
        self.distribute(&mut other);
        Some(other)
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => self.move_right(other, (other.len() - self.len()) / 2),
            Ordering::Greater => self.move_left(other, (self.len() + other.len()) / 2),
            _ => {}
        }
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        self.chunk.move_left(&mut other.chunk, end);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        self.chunk.move_right(&mut other.chunk, start);
    }
}
