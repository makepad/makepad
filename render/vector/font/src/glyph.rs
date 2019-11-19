use crate::{HorizontalMetrics, Outline};
use geometry::Rectangle;

/// A glyph in a font.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub horizontal_metrics: HorizontalMetrics,
    pub bounds: Rectangle,
    pub outline: Outline,
}
