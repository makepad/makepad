/// The horizontal metrics for a glyph
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HorizontalMetrics {
    pub advance_width: f32,
    pub left_side_bearing: f32,
}
