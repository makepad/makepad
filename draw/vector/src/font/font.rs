use crate::font::Glyph;
use crate::geometry::Rectangle;

/// A font.
#[derive(Clone, Debug, PartialEq)]
pub struct TTFFont {
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_gap: f64,
    pub bounds: Rectangle,
    pub char_code_to_glyph_index_map: Vec<usize>,
    pub glyphs: Vec<Glyph>,
}


impl TTFFont{
    pub fn get_glyph(&self, c:char)->Option<&Glyph>{
        if c < '\u{10000}' {
            Some(&self.glyphs[self.char_code_to_glyph_index_map[c as usize]])
        } else {
            None
        }
    }
}

