use {
    crate::{Chunks, Iter, IterRev, Leaf, Metric},
    btree,
};

#[derive(Clone)]
pub struct Cursor<'a, T, M>
where
    M: Metric<T>,
{
    cursor: btree::Cursor<'a, Leaf<T, M>>,
}

impl<'a, T, M> Cursor<'a, T, M>
where
    M: Metric<T>,
{
    pub(super) fn new(cursor: btree::Cursor<'a, Leaf<T, M>>) -> Self {
        Self { cursor }
    }

    pub fn into_chunks(self) -> Chunks<'a, T, M> {
        Chunks::new(self)
    }

    pub fn into_iter(self) -> Iter<'a, T, M> {
        Iter::new(self)
    }

    pub fn into_iter_rev(self) -> IterRev<'a, T, M> {
        IterRev::new(self)
    }

    pub fn is_start(&self) -> bool {
        self.cursor.is_start()
    }

    pub fn is_end(&self) -> bool {
        self.cursor.is_end()
    }

    pub fn index(&self) -> usize {
        self.cursor.index()
    }

    pub fn current_chunk(&self) -> &'a [T] {
        let (slice, start, end) = self.current_inner();
        &slice[start..end]
    }

    pub fn current(&self) -> Option<&'a T> {
        let (slice, _, end) = self.current_inner();
        slice[..end].get(self.cursor.offset())
    }

    pub fn move_next_chunk(&mut self) -> bool {
        self.cursor.move_next_chunk()
    }

    pub fn move_next(&mut self) -> bool {
        self.cursor.move_next()
    }

    pub fn move_prev_chunk(&mut self) -> bool {
        self.cursor.move_prev_chunk()
    }

    pub fn move_prev(&mut self) -> bool {
        self.cursor.move_prev()
    }

    fn current_inner(&self) -> (&'a [T], usize, usize) {
        let (leaf, start, end) = self.cursor.current();
        (leaf.map_or(&[], |leaf| leaf.as_slice()), start, end)
    }
}
