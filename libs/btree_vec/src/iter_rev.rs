use crate::{Cursor, Metric};

#[derive(Clone)]
pub struct IterRev<'a, T, M>
where
    M: Metric<T>,
{
    cursor: Cursor<'a, T, M>,
}

impl<'a, T, M> IterRev<'a, T, M>
where
    M: Metric<T>,
{
    pub(super) fn new(cursor: Cursor<'a, T, M>) -> Self {
        Self { cursor }
    }
}

impl<'a, T, M> Iterator for IterRev<'a, T, M>
where
    M: Metric<T>,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.move_prev() {
            self.cursor.current()
        } else {
            None
        }
    }
}
