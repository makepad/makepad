//! The tray glyph, drawn from geometry as a monochrome template (it follows a light or dark bar,
//! and is the node's icon in the Linux WM's bar too): the app icon's mark
//! (apps/devhub/resources/icon.svg), an eye with three nodes linked into it. Quiet: a small
//! pupil. Attention (new inbox entries): a solid larger pupil and the right node grown into a
//! badge. Off (no network identity): a hollow pupil.

use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Quiet,
    Attention,
    Off,
}

pub const SIZE: u32 = 36;

/// The eye (centre, half-width, half-height: a lens of two arcs, pointed at the corners) and the
/// three nodes linked into it, in glyph pixels.
const EYE: (f32, f32, f32, f32) = (17.5, 22.0, 15.0, 8.0);
const NODES: [(f32, f32); 3] = [(6.0, 5.0), (17.5, 3.5), (29.0, 5.0)];

/// Distance from p to the segment a-b.
fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

pub fn image(glyph: Glyph) -> TrayImage {
    let n = SIZE as usize;
    let (cx, cy, w, h) = EYE;
    // the lens = the overlap of two circles through both corners
    let big = (w * w + h * h) / (2.0 * h);
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = (x as f32, y as f32);
            // the eye's outline: signed distance to the lens (negative inside)
            let arc = |oy: f32| ((p.0 - cx).powi(2) + (p.1 - oy).powi(2)).sqrt() - big;
            let lens = arc(cy - (big - h)).max(arc(cy + (big - h)));
            let mut v = cover(lens.abs() - 1.1);
            let r = ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt();
            v = v.max(match glyph {
                Glyph::Quiet => cover(r - 4.0),
                Glyph::Attention => cover(r - 5.0),
                Glyph::Off => cover((r - 3.8).abs() - 0.9),
            });
            // the nodes and their links, which stop at the eye
            for (i, &node) in NODES.iter().enumerate() {
                let badge = glyph == Glyph::Attention && i == 2;
                let radius = if badge { 4.6 } else { 2.9 };
                v = v.max(cover(((p.0 - node.0).powi(2) + (p.1 - node.1).powi(2)).sqrt() - radius));
                if lens > 0.0 {
                    v = v.max(cover(segment(p, node, (cx, cy)) - 0.9));
                }
            }
            alpha[y * n + x] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    TrayImage { width: SIZE, height: SIZE, alpha }
}

#[cfg(test)]
mod tests {
    #[test]
    fn glyphs_are_distinct_full_size_masks() {
        use super::*;
        let all = [Glyph::Quiet, Glyph::Attention, Glyph::Off].map(image);
        for g in &all {
            assert_eq!(g.alpha.len(), (SIZE * SIZE) as usize);
            assert_eq!(g.alpha[0], 0);
        }
        assert_ne!(all[0].alpha, all[1].alpha);
        assert_ne!(all[0].alpha, all[2].alpha);
    }
}
