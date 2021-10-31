use crate::Glyph;
use makepad_geometry::Rectangle;

/// A font.
#[derive(Clone, Debug, PartialEq)]
pub struct TTFFont {
    pub units_per_em: f32,
    pub ascender: f32,
    pub descender: f32,
    pub line_gap: f32,
    pub bounds: Rectangle,
    pub char_code_to_glyph_index_map: Vec<usize>,
    pub glyphs: Vec<Glyph>,
}
