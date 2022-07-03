mod cursor;
mod graphemes;
mod words;

pub use self::{cursor::Cursor, graphemes::Graphemes, words::Words};

pub trait StrExt {
    fn cursor_at(&self, position: usize) -> Cursor<'_>;
    fn graphemes(&self) -> Graphemes<'_>;
    fn words(&self) -> Words<'_>;
}

impl StrExt for str {
    fn cursor_at(&self, position: usize) -> Cursor<'_> {
        Cursor::new(self, position)
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes::new(self)
    }

    fn words(&self) -> Words<'_> {
        Words::new(self)
    }
}
