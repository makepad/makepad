use {super::Cursor, crate::cursor::grapheme};

/// An iterator over the graphemes of a `str`.
///
/// This struct is created by the `graphemes` method on `StrExt`.

pub struct Graphemes<'a> {
    string: &'a str,
    cursor: grapheme::Cursor<Cursor<'a>>,
    cursor_back: grapheme::Cursor<Cursor<'a>>,
}

impl<'a> Graphemes<'a> {
    pub(super) fn new(string: &'a str) -> Self {
        use crate::cursor::char::Cursor as _;

        Self {
            string,
            cursor: Cursor::new(string, 0).into_grapheme_cursor(),
            cursor_back: Cursor::new(string, string.len()).into_grapheme_cursor(),
        }
    }
}

impl<'a> Iterator for Graphemes<'a> {
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

impl<'a> DoubleEndedIterator for Graphemes<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.cursor.position() == self.cursor_back.position() {
            return None;
        }
        let end = self.cursor_back.position();
        self.cursor_back.move_prev();
        Some(&self.string[self.cursor_back.position()..end])
    }
}
