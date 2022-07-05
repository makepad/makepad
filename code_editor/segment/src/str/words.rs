use {super::Cursor, crate::cursor::word};

/// An iterator over the words of a `str`.
/// 
/// This struct is created by the `words` method on `StrExt`.
pub struct Words<'a> {
    string: &'a str,
    cursor: word::Cursor<Cursor<'a>>,
    cursor_back: word::Cursor<Cursor<'a>>,
}

impl<'a> Words<'a> {
    pub(super) fn new(string: &'a str) -> Self {
        use crate::{cursor::char::Cursor as _, str::StrExt};

        Self {
            string,
            cursor: string.cursor_at(0).into_word_cursor(),
            cursor_back: string.cursor_at(string.len()).into_word_cursor(),
        }
    }
}

impl<'a> Iterator for Words<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.position() == self.cursor_back.position() {
            return None;
        }
        let start = self.cursor.position();
        self.cursor.move_next();
        Some(&self.string[start..self.cursor.position()])
    }
}

impl<'a> DoubleEndedIterator for Words<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.cursor.position() == self.cursor_back.position() {
            return None;
        }
        let end = self.cursor_back.position();
        self.cursor_back.move_prev();
        Some(&self.string[self.cursor_back.position()..end])
    }
}
