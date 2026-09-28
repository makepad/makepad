//! Nine-slice scaling for `Image`: the arithmetic behind `ImageFit::Slice`,
//! kept apart from the widget so its rules can be tested without a GPU.
//!
//! A panel, a button skin or a window frame is often drawn from one small
//! texture that has to appear at whatever size the layout hands it.
//! Stretching the whole texture stretches its corners too. Slicing cuts it
//! into nine parts along an inset measured in texels: the corners keep their
//! size, the edges fill their length one way, and the middle fills both ways
//! or is left out.
//!
//! The picture's own pixel shader in `image.rs` does the work, in the one
//! quad the image already draws, so rounding, stroke, opacity and clipping
//! keep working on top of it. The widget resolves the inputs once per draw
//! with the functions below, and the shader's `slice_axis` is [`slice_axis`]
//! here, line for line, so these tests are the tests of what every fragment
//! computes.
//!
//! A tiled part on a texture that carries a mip chain can show a one-pixel
//! line at each tile seam, because the read coordinate jumps there and the
//! sampler picks a coarse level from that jump.

use crate::makepad_draw::*;

/// How a sliced picture's four edges fill their length.
#[derive(Clone, Copy, Debug, Default, PartialEq, Script, ScriptHook)]
pub enum ImageSliceEdge {
    /// The edge's texels drawn once, drawn out to the length.
    #[default]
    Stretch,
    /// The texels repeated at the border's own scale, centred on the edge, so
    /// a pattern keeps its spacing and both ends are cut alike.
    Tile,
    /// The texels repeated a whole number of times, the spacing adjusted a
    /// little so nothing is cut.
    Round,
}

/// How a sliced picture's middle fills.
#[derive(Clone, Copy, Debug, Default, PartialEq, Script, ScriptHook)]
pub enum ImageSliceCenter {
    /// The middle drawn out both ways.
    #[default]
    Stretch,
    /// The middle repeated both ways, with the spacing its edges use.
    Tile,
    /// The middle left out, for a frame around content drawn under it.
    Hidden,
}

/// What `slice_scale` counts one border texel as.
#[derive(Clone, Copy, Debug, Default, PartialEq, Script, ScriptHook)]
pub enum ImageSliceUnits {
    /// Layout points, the unit the picture's natural size already uses.
    #[default]
    Points,
    /// Device pixels, for art that must land on the display's own grid.
    DevicePixels,
}

/// The inset as whole texels that fit inside the window, as
/// `[left, top, right, bottom]`.
///
/// A side that is not a finite number, or is below zero, is no border: a
/// NaN carried into the shader fails every comparison there and draws
/// nothing anywhere, and an infinite side has no proportion to keep when
/// its pair is scaled down. A pair wider than the window is scaled down
/// together, so a panel written for a larger texture keeps the balance
/// between its two sides, and floored, so the pair never reaches past the
/// window and the middle is left with a texel or none, never a negative
/// count.
pub fn slice_texels(inset: Inset, window: Vec2d) -> [f64; 4] {
    fn side(v: f64) -> f64 {
        if v.is_finite() && v > 0.0 {
            v.round()
        } else {
            0.0
        }
    }
    fn fit_pair(start: f64, end: f64, window: f64) -> (f64, f64) {
        if !(window.is_finite() && window > 0.0) {
            return (0.0, 0.0);
        }
        let sum = start + end;
        if sum > window {
            let k = window / sum;
            return ((start * k).floor(), (end * k).floor());
        }
        (start, end)
    }
    let (left, right) = fit_pair(side(inset.left), side(inset.right), window.x);
    let (top, bottom) = fit_pair(side(inset.top), side(inset.bottom), window.y);
    [left, top, right, bottom]
}

/// Layout points one border texel is drawn at, before any shrink.
///
/// A scale that is not a number is the default, and one outside 0.01..64 is
/// held there: at zero every border would vanish into a division by nothing
/// in the shader, and past 64 a single texel is already larger than any box
/// a panel is laid out in. A density that is not a positive number is taken
/// as one, so `DevicePixels` never divides by it.
pub fn slice_points_per_texel(scale: f64, units: ImageSliceUnits, dpi: f64) -> f64 {
    let scale = if scale.is_nan() { 1.0 } else { scale.clamp(0.01, 64.0) };
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 1.0 };
    match units {
        ImageSliceUnits::Points => scale,
        ImageSliceUnits::DevicePixels => scale / dpi,
    }
}

/// The one factor all four borders shrink by when the box cannot hold them.
///
/// One factor rather than one per axis, because a panel squeezed in one
/// direction should get smaller round corners, not oval ones. It never goes
/// above one: borders keep their size in a box that has room for them. An
/// axis with no border has nothing to shrink and is left out. The shader
/// works this out for itself from the same inputs, because the box's size is
/// only final once the quad is placed.
pub fn slice_shrink(size: Vec2d, texels: [f64; 4], points_per_texel: f64) -> f64 {
    let [left, top, right, bottom] = texels;
    let wide = (left + right) * points_per_texel;
    let tall = (top + bottom) * points_per_texel;
    let mut fit = 1.0f64;
    if wide > 0.0 {
        fit = fit.min(size.x.max(0.0) / wide);
    }
    if tall > 0.0 {
        fit = fit.min(size.y.max(0.0) / tall);
    }
    fit
}

/// The two numbers the shader reads, as (edges, middle).
///
/// Edges: 1 stretch, 2 tile, 3 round. Middle: 0 hidden, 1 stretch, 2 tile.
/// A tiled middle between rounded edges is 3, so it takes the edges' rounded
/// spacing and its tiles line up with theirs instead of drifting against
/// them. Zero on the edges means "not sliced", and only the widget writes it.
pub fn slice_modes(edge: ImageSliceEdge, center: ImageSliceCenter) -> Vec2f {
    let edges = match edge {
        ImageSliceEdge::Stretch => 1.0,
        ImageSliceEdge::Tile => 2.0,
        ImageSliceEdge::Round => 3.0,
    };
    let middle = match center {
        ImageSliceCenter::Hidden => 0.0,
        ImageSliceCenter::Stretch => 1.0,
        ImageSliceCenter::Tile if edge == ImageSliceEdge::Round => 3.0,
        ImageSliceCenter::Tile => 2.0,
    };
    vec2(edges, middle)
}

/// Where along one axis a point reads, in texels, and how many points a
/// texel is drawn at there. The shader's `slice_axis` is this function.
///
/// `p` is the point along a box `len` long, `texels` the window's length,
/// `a` and `b` the borders in texels, `s` the points per border texel after
/// any shrink, and `mode` 1 stretch, 2 tile, 3 a whole number of tiles.
/// Every read is held half a texel inside its own part, because a filtered
/// read at a cut blends in the texel on the other side of it, and that texel
/// belongs to a different part. `clamp` and `fract` are written out the way
/// the shader has them: the standard `clamp` panics on a range the wrong way
/// round, and the standard `fract` keeps the sign where the shader's floors.
pub fn slice_axis(p: f64, len: f64, texels: f64, a: f64, b: f64, s: f64, mode: f64) -> (f64, f64) {
    fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
        x.max(lo).min(hi)
    }
    fn fract(x: f64) -> f64 {
        x - x.floor()
    }
    let n = (texels - a - b).max(0.0);
    let start = a * s;
    let end = len - b * s;
    if p < start {
        return (clamp(p / s, 0.5, (a - 0.5).max(0.5)), s);
    }
    if p >= end {
        let first = texels - b;
        return (clamp(first + (p - end) / s, first + 0.5, (texels - 0.5).max(first + 0.5)), s);
    }
    let mid = (end - start).max(0.0001);
    let lo = a + 0.5;
    let hi = (a + n - 0.5).max(lo);
    if mode >= 2.0 {
        let mut tile = (n * s).max(0.0001);
        if mode >= 3.0 {
            tile = mid / (mid / tile + 0.5).floor().max(1.0);
        }
        // A tile's centre sits on the band's centre, so the seams fall the
        // same distance either side of it and both ends are cut alike.
        let t = fract((p - start - mid * 0.5) / tile + 0.5);
        return (clamp(a + t * n, lo, hi), tile / n.max(0.0001));
    }
    (clamp(a + (p - start) / mid * n, lo, hi), mid / n.max(0.0001))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn inset(left: f64, top: f64, right: f64, bottom: f64) -> Inset {
        Inset { left, top, right, bottom }
    }

    /// The cut is a property of the texture, so it is counted in whole
    /// texels of it, and a pair that cannot fit is made to fit on both sides
    /// at once instead of one side losing everything.
    #[test]
    fn an_inset_is_whole_texels_inside_the_window() {
        let square = dvec2(64.0, 64.0);
        assert_eq!(slice_texels(inset(15.6, 16.4, 16.0, 16.0), square), [16.0, 16.0, 16.0, 16.0]);
        assert_eq!(slice_texels(inset(f64::NAN, -3.0, f64::INFINITY, 0.0), square), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(slice_texels(inset(40.0, 40.0, 40.0, 40.0), square), [32.0, 32.0, 32.0, 32.0]);
        assert_eq!(slice_texels(inset(40.0, 40.0, 30.0, 30.0), square), [36.0, 36.0, 27.0, 27.0]);
        assert_eq!(slice_texels(inset(16.0, 16.0, 16.0, 16.0), dvec2(0.0, 0.0)), [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(slice_texels(inset(16.0, 16.0, 16.0, 16.0), dvec2(0.0, 64.0)), [0.0, 16.0, 0.0, 16.0]);
    }

    /// One texel is one point by default, which is what the picture's
    /// natural size already counts; device pixels divide by the density.
    #[test]
    fn a_texel_is_points_or_device_pixels() {
        assert!(close(slice_points_per_texel(1.0, ImageSliceUnits::Points, 1.5), 1.0));
        assert!(close(slice_points_per_texel(1.0, ImageSliceUnits::DevicePixels, 1.5), 2.0 / 3.0));
        assert!(close(slice_points_per_texel(f64::NAN, ImageSliceUnits::Points, 1.0), 1.0));
        assert!(close(slice_points_per_texel(0.0, ImageSliceUnits::Points, 1.0), 0.01));
        assert!(close(slice_points_per_texel(2.0, ImageSliceUnits::DevicePixels, 0.0), 2.0));
        assert!(close(slice_points_per_texel(1000.0, ImageSliceUnits::Points, 1.0), 64.0));
    }

    /// A squeeze in one direction shrinks all four borders by the same
    /// factor, and a box with room to spare never grows them.
    #[test]
    fn the_borders_shrink_together_and_only_when_they_must() {
        let sixteen = [16.0; 4];
        assert!(close(slice_shrink(dvec2(300.0, 200.0), sixteen, 1.0), 1.0));
        assert!(close(slice_shrink(dvec2(20.0, 100.0), sixteen, 1.0), 0.625));
        assert!(close(slice_shrink(dvec2(100.0, 20.0), sixteen, 1.0), 0.625));
        assert!(close(slice_shrink(dvec2(0.0, 0.0), sixteen, 1.0), 0.0));
        assert!(close(slice_shrink(dvec2(20.0, 20.0), [0.0; 4], 1.0), 1.0));
        for size in [32.0, 33.0, 64.0, 1000.0] {
            assert!(slice_shrink(dvec2(size, size), sixteen, 1.0) <= 1.0, "a {size} box grew the borders");
        }
    }

    /// A border is drawn at its own size and reads the texel under it; its
    /// outermost and innermost reads stay half a texel inside the border.
    #[test]
    fn a_border_reads_one_texel_per_its_own_size() {
        assert_eq!(slice_axis(8.0, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0), (8.0, 1.0));
        assert!(close(slice_axis(0.2, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0).0, 0.5));
        assert!(close(slice_axis(284.0, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0).0, 48.5));
        assert!(close(slice_axis(299.5, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0).0, 63.5));
    }

    /// Stretched, the middle's texels are spread over the band, and neither
    /// end of it borrows a texel of the border beside it; a one-texel middle
    /// is one flat colour, not a gradient between its neighbours.
    #[test]
    fn a_stretched_middle_spans_its_texels_and_borrows_nothing() {
        assert!(close(slice_axis(16.0, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0).0, 16.5));
        let (at, per_texel) = slice_axis(150.0, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0);
        assert!(close(at, 32.0));
        assert!(close(per_texel, 268.0 / 32.0));
        assert!(close(slice_axis(283.99, 300.0, 64.0, 16.0, 16.0, 1.0, 1.0).0, 47.5));
        for p in [16.0, 100.0, 150.0, 200.0, 283.0] {
            assert!(
                close(slice_axis(p, 300.0, 33.0, 16.0, 16.0, 1.0, 1.0).0, 16.5),
                "a one-texel middle read a gradient at {p}"
            );
        }
    }

    /// Tiles are copies at the border's own scale, and the seams sit the
    /// same distance either side of the band's centre.
    #[test]
    fn tiles_are_copies_centred_on_the_band() {
        let (centre, per_texel) = slice_axis(150.0, 300.0, 64.0, 16.0, 16.0, 1.0, 2.0);
        assert!(close(centre, 32.0));
        assert!(close(per_texel, 1.0));
        assert!(close(slice_axis(166.0, 300.0, 64.0, 16.0, 16.0, 1.0, 2.0).0, 16.5));
        assert!(close(slice_axis(134.0, 300.0, 64.0, 16.0, 16.0, 1.0, 2.0).0, 16.5));
    }

    /// Round takes the whole number of tiles nearest to what the band holds
    /// at the border's own scale, and never fewer than one.
    #[test]
    fn round_fits_a_whole_number_of_tiles() {
        for (middle, tile) in [(248.0, 31.0), (10.0, 10.0), (47.0, 47.0), (48.0, 24.0)] {
            let len = 32.0 + middle;
            let (_, per_texel) = slice_axis(16.0 + middle * 0.5, len, 64.0, 16.0, 16.0, 1.0, 3.0);
            assert!(
                close(per_texel * 32.0, tile),
                "a {middle}-point middle tiled at {} rather than {tile}",
                per_texel * 32.0
            );
        }
    }

    #[test]
    fn modes_say_what_the_shader_draws() {
        use ImageSliceCenter as C;
        use ImageSliceEdge as E;
        assert_eq!(slice_modes(E::Stretch, C::Stretch), vec2(1.0, 1.0));
        assert_eq!(slice_modes(E::Tile, C::Hidden), vec2(2.0, 0.0));
        assert_eq!(slice_modes(E::Round, C::Tile), vec2(3.0, 3.0));
        assert_eq!(slice_modes(E::Tile, C::Tile), vec2(2.0, 2.0));
        assert_eq!(slice_modes(E::Stretch, C::Tile), vec2(1.0, 2.0));
    }

    /// The shader has no way to report a NaN, only to draw nothing where one
    /// lands, so every corner of the input space has to come back a number.
    #[test]
    fn no_input_reads_a_number_that_is_not_one() {
        for len in [0.0, 1.0, 300.0] {
            for p in [-1.0, 0.0, 0.5, len / 2.0, len, len + 1.0] {
                for texels in [0.0, 1.0, 64.0] {
                    for a in [0.0, 16.0, 40.0] {
                        for b in [0.0, 16.0, 40.0] {
                            for s in [0.0001, 1.0, 3.0] {
                                for mode in [1.0, 2.0, 3.0] {
                                    let (at, per_texel) = slice_axis(p, len, texels, a, b, s, mode);
                                    assert!(
                                        at.is_finite() && per_texel.is_finite(),
                                        "slice_axis({p}, {len}, {texels}, {a}, {b}, {s}, {mode}) gave ({at}, {per_texel})"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
