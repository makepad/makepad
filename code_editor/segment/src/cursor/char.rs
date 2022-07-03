use crate::cursor::{grapheme, word};

/// A cursor over the `char`s in a text.
///
/// A `Cursor` is like an iterator, except that it can freely seek back-and-forth.
pub trait Cursor {
    /// Returns `true` if this `Cursor` is at the start of the text.
    fn is_at_start(&self) -> bool;

    /// Returns `true` if this `Cursor` is at the end of the text.
    fn is_at_end(&self) -> bool;

    /// Returns `true` if this `Cursor` is at a `char` boundary.
    fn is_at_boundary(&self) -> bool;

    /// Returns the position of this `Cursor`.
    fn position(&self) -> usize;

    /// Returns the `char` that this `Cursor` is pointing to.
    ///
    /// # Panics
    ///
    /// Panics if this cursor is not at a `char` boundary.
    fn current(&self) -> char;

    /// Moves this `Cursor` to the next `char` boundary.
    ///
    /// # Panics
    ///
    /// Panics if this `Cursor` is at the end of the text.
    fn move_next(&mut self);

    /// Moves this `Cursor` to the previous `char` boundary.
    ///
    /// # Panics
    ///
    /// Panics if this `Cursor` is at the start of the text.
    fn move_prev(&mut self);

    /// Sets the `position` of this `Cursor`.
    ///
    /// # Panics
    ///
    /// Panics if `position` is out of bounds.
    fn set_position(&mut self, position: usize);

    /// Converts this `Cursor` into a cursor over the graphemes in a text.
    fn into_grapheme_cursor(self) -> grapheme::Cursor<Self>
    where
        Self: Sized,
    {
        grapheme::Cursor::new(self)
    }

    /// Converts this `Cursor` into a cursor over the words in a text.
    fn into_word_cursor(self) -> word::Cursor<Self>
    where
        Self: Sized,
    {
        word::Cursor::new(self)
    }
}
