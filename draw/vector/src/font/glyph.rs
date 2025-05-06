use crate::font::HorizontalMetrics;
use crate::geometry::Rectangle;
use crate::path::PathCommand;
use resvg::usvg::Tree;
use std::rc::Rc;

/// A glyph in a font.
#[derive(Clone, Debug)]
pub struct Glyph {
    pub horizontal_metrics: HorizontalMetrics,
    pub bounds: Rectangle,
    pub outline: Vec<PathCommand>,
    pub svg_image: Option<Rc<Tree>>,
}
