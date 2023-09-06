use crate::font::Glyph;
use crate::geometry::Rectangle;

/// A font.
#[derive(Clone, Debug)]
pub struct TTFFont {
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_gap: f64,
    pub bounds: Rectangle,
    pub cached_decoded_glyphs: Vec<Option<Box<Glyph>>>,
}

