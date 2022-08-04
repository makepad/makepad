use crate::{Cursor, Slice};

#[derive(Clone, Debug)]
pub struct Chunks<'a> {
    is_at_end: bool,
    cursor: Cursor<'a>,
}

impl<'a> Chunks<'a> {
    pub(crate) fn new(slice: Slice<'a>) -> Self {
        Self {
            is_at_end: false,
            cursor: slice.cursor_front(),
        }
    }
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.is_at_end {
            return None;
        }
        let chunk = self.cursor.current();
        if self.cursor.is_at_back() {
            self.is_at_end = true;
        } else {
            self.cursor.move_next();
        }
        Some(chunk)
    }
}
