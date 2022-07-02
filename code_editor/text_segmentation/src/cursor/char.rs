use crate::cursor::{grapheme, word};

pub trait Cursor {
    fn is_at_start(&self) -> bool;
    fn is_at_end(&self) -> bool;
    fn is_at_boundary(&self) -> bool;
    fn position(&self) -> usize;
    fn current(&self) -> char;
    fn move_next(&mut self);
    fn move_prev(&mut self);
    fn set_position(&mut self, position: usize);

    fn into_grapheme_cursor(self) -> grapheme::Cursor<Self>
    where
        Self: Sized,
    {
        grapheme::Cursor::new(self)
    }

    fn into_word_cursor(self) -> word::Cursor<Self>
    where
        Self: Sized,
    {
        word::Cursor::new(self)
    }
}
