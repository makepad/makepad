use crate::{Cursor, Metric};

#[derive(Clone)]
pub struct Iter<'a, T, M>
where
    M: Metric<T>,
{
    cursor: Cursor<'a, T, M>,
}

impl<'a, T, M> Iter<'a, T, M>
where
    M: Metric<T>,
{
    pub(super) fn new(cursor: Cursor<'a, T, M>) -> Self {
        Self { cursor }
    }
}

impl<'a, T, M> Iterator for Iter<'a, T, M>
where
    M: Metric<T>,
{
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        let current = self.cursor.current();
        self.cursor.move_next();
        current
    }
}
