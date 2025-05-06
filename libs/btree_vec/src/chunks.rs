use crate::{Cursor, Metric};

#[derive(Clone)]
pub struct Chunks<'a, T, M>
where
    M: Metric<T>,
{
    cursor: Cursor<'a, T, M>,
}

impl<'a, T, M> Chunks<'a, T, M>
where
    M: Metric<T>,
{
    pub(super) fn new(cursor: Cursor<'a, T, M>) -> Self {
        Self { cursor }
    }
}

impl<'a, T, M> Iterator for Chunks<'a, T, M>
where
    M: Metric<T>,
{
    type Item = &'a [T];

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_end() {
            return None;
        }
        let current = self.cursor.current_chunk();
        self.cursor.move_next_chunk();
        Some(current)
    }
}
