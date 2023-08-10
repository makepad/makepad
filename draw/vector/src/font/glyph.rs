use crate::font::HorizontalMetrics;
use crate::geometry::Rectangle;
use crate::path::PathCommand;

/// A glyph in a font.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub horizontal_metrics: HorizontalMetrics,
    pub bounds: Rectangle,
    pub outline: Vec<PathCommand>,
}
