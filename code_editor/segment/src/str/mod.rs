mod cursor;
mod graphemes;
mod grapheme_indices;
mod words;
mod word_indices;

pub use self::{cursor::Cursor, graphemes::Graphemes, grapheme_indices::GraphemeIndices, words::Words, word_indices::WordIndices};

/// Extends `str` with methods to segment strings according to
/// [Unicode Standard Annex #29](http://www.unicode.org/reports/tr29/).
pub trait StrExt {
    /// Returns a cursor at the given position over this `str`.
    fn cursor_at(&self, position: usize) -> Cursor<'_>;

    /// Returns an iterator over the graphemes of this `str`.
    fn graphemes(&self) -> Graphemes<'_>;

    /// Returns an iterator over the graphemes of this `str`, and their positions.
    fn grapheme_indices(&self) -> GraphemeIndices<'_>;

    /// Returns an iterator over the words of this `str`.
    fn words(&self) -> Words<'_>;

    /// Returns an iterator over the words of this `str`, and their positions.
    fn word_indices(&self) -> WordIndices<'_>;
}

impl StrExt for str {
    fn cursor_at(&self, position: usize) -> Cursor<'_> {
        Cursor::new(self, position)
    }

    fn graphemes(&self) -> Graphemes<'_> {
        Graphemes::new(self)
    }

    fn grapheme_indices(&self) -> GraphemeIndices<'_> {
        GraphemeIndices::new(self)
    }

    fn words(&self) -> Words<'_> {
        Words::new(self)
    }

    fn word_indices(&self) -> WordIndices<'_> {
        WordIndices::new(self)
    }
}
