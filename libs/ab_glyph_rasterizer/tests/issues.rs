use ab_glyph_rasterizer::*;

/// Index oob panic rasterizing "Gauntl" using Bitter-Regular.otf
#[test]
fn rusttype_156_index_panic() {
    let mut r = Rasterizer::new(6, 16);
    r.draw_line(point(5.54, 14.299999), point(3.7399998, 13.799999));
    r.draw_line(point(3.7399998, 13.799999), point(3.7399998, 0.0));
    r.draw_line(point(3.7399998, 0.0), point(0.0, 0.10000038));
}

/// An outline past the image's edge (rounding at a large size, or bounds that miss a point) stays
/// in its own row: past the right edge on the last row it no longer indexes out of the buffer,
/// and on the other rows it no longer wraps into the next row; past the left edge it covers the
/// row from column 0 instead of being dropped or wrapping into the row before.
#[test]
fn an_outline_past_the_edges_stays_in_its_rows() {
    let rows = |r: &Rasterizer| {
        let mut out = vec![[0.0f32; 4]; 3];
        r.for_each_pixel_2d(|x, y, a| out[y as usize][x as usize] = a);
        out
    };
    let mut right = Rasterizer::new(4, 3);
    right.draw_line(point(1.0, 0.0), point(1.0, 3.0));
    right.draw_line(point(6.0, 3.0), point(6.0, 0.0));
    assert_eq!(rows(&right), vec![[0.0, 1.0, 1.0, 1.0]; 3], "a shape from x 1 to 6 covers columns 1-3 of every row");

    let mut left = Rasterizer::new(4, 3);
    left.draw_line(point(-2.0, 0.0), point(-2.0, 3.0));
    left.draw_line(point(2.0, 3.0), point(2.0, 0.0));
    assert_eq!(rows(&left), vec![[1.0, 1.0, 0.0, 0.0]; 3], "a shape from x -2 to 2 covers columns 0-1 of every row");
}
