use crate::font::Glyph;
use crate::geometry::Rectangle;
use resvg::usvg::Tree;
use std::rc::Rc;

/// A font.
#[derive(Clone, Debug)]
pub struct TTFFont {
    pub units_per_em: f64,
    pub ascender: f64,
    pub descender: f64,
    pub line_gap: f64,
    pub bounds: Rectangle,
    pub cached_decoded_glyphs: Vec<Option<Box<Glyph>>>,
    pub cached_svg_images: Vec<Option<Option<Rc<Tree>>>>,
}

