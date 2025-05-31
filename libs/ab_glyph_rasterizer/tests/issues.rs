use ab_glyph_rasterizer::*;

/// Index oob panic rasterizing "Gauntl" using Bitter-Regular.otf
#[test]
fn rusttype_156_index_panic() {
    let mut r = Rasterizer::new(6, 16);
    r.draw_line(point(5.54, 14.299999), point(3.7399998, 13.799999));
    r.draw_line(point(3.7399998, 13.799999), point(3.7399998, 0.0));
    r.draw_line(point(3.7399998, 0.0), point(0.0, 0.10000038));
}
