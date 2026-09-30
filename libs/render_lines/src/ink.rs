//! The ink rule: how a line's width becomes pixel coverage.
//!
//! Widths are logical pixels; the shader works in physical pixels
//! (`px_scale` = physical / logical, 2 at 4K for a 1080p layout). Coverage
//! is the exact overlap of a one-physical-pixel box filter with the line's
//! cross-section, so the coverage summed across a line equals its physical
//! width: a line keeps its ink at every scale, and is only sharper at 4K
//! (the AA ramp is one physical pixel, half a logical one).
//!
//! A line thinner than [`MIN_PX`] physical pixels is drawn [`MIN_PX`]
//! wide with its alpha scaled by width / [`MIN_PX`]: the same ink, spread
//! over enough pixels not to break into dots (the fade floor). The
//! shaders ([`crate::draw`]) implement exactly these functions; the tests
//! here pin the arithmetic.

/// The fade floor: thinnest drawn width in physical pixels.
pub const MIN_PX: f32 = 0.7;

/// Coverage of a pixel whose centre is `d` physical pixels from the line's
/// centre (across it), for a line `half` physical pixels half-wide.
pub fn coverage(d: f32, half: f32) -> f32 {
    let d = d.abs();
    ((d + 0.5).min(half) - (d - 0.5).max(-half)).clamp(0.0, 1.0)
}

/// The drawn half width (physical px) and alpha scale of a line `width`
/// logical pixels wide at `px_scale`, under the floor `min_px`.
pub fn drawn(width: f32, px_scale: f32, min_px: f32) -> (f32, f32) {
    let w = (width * px_scale).max(0.0);
    let floor = min_px.max(1e-3);
    if w < floor {
        (floor * 0.5, w / floor)
    } else {
        (w * 0.5, 1.0)
    }
}

/// The ink of a line across one physical pixel column, in logical pixels:
/// coverage summed over the pixels across it, times the alpha scale,
/// divided by `px_scale`. Equal to `width` for any placement.
pub fn ink(width: f32, px_scale: f32, min_px: f32, centre_offset: f32) -> f32 {
    let (half, alpha) = drawn(width, px_scale, min_px);
    let reach = half.ceil() as i32 + 2;
    let sum: f32 = (-reach..=reach).map(|k| coverage(k as f32 + centre_offset, half)).sum();
    sum * alpha / px_scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_keeps_its_ink_at_every_scale_and_offset() {
        for &w in &[0.1f32, 0.35, 0.5, 0.7, 1.0, 1.3, 2.0, 3.7, 12.0] {
            for &s in &[1.0f32, 1.5, 2.0, 3.0] {
                for &o in &[0.0f32, 0.25, 0.5, 0.77] {
                    let i = ink(w, s, MIN_PX, o);
                    assert!((i - w).abs() < 1e-4 * w.max(1.0), "width {w} at scale {s}, offset {o}: ink {i}");
                }
            }
        }
    }

    #[test]
    fn thin_lines_draw_at_the_floor_with_less_alpha() {
        assert_eq!(drawn(0.35, 1.0, MIN_PX), (0.35, 0.5));
        assert_eq!(drawn(0.35, 2.0, MIN_PX), (0.35, 1.0));
        let (h, a) = drawn(1.0, 2.0, MIN_PX);
        assert_eq!((h, a), (1.0, 1.0));
    }

    #[test]
    fn a_4k_line_is_sharper_not_thinner() {
        // The same 1-logical-px line: at 2x its full-coverage core is wider
        // in logical terms (two whole physical pixels vs a split pixel).
        let peak1 = coverage(0.0, drawn(1.0, 1.0, MIN_PX).0);
        let peak2 = coverage(0.0, drawn(1.0, 2.0, MIN_PX).0);
        assert_eq!((peak1, peak2), (1.0, 1.0));
        // Straddling a pixel edge, 1x smears it over two half-covered pixels;
        // 2x keeps two fully covered physical pixels.
        assert_eq!(coverage(0.5, 0.5), 0.5);
        assert_eq!(coverage(0.5, 1.0), 1.0);
    }
}
