//! devhub's tray glyph (shared by the app and the makepad WM's "off" slot), drawn from geometry as
//! a monochrome alpha mask (it follows a light or dark bar): the app icon's mark
//! (apps/devhub/resources/icon.svg, tray.svg), an eye with three machines linked into it from
//! above. It reads apart from the AI hub's centre with spokes at menu-bar size. The pupil and a
//! badge carry the state. Quiet: a small solid pupil. Attention (new inbox entries): a bigger
//! pupil and the right machine grown into a badge (on the master: a failed landing or verify).
//! Busy (leases, gates or a verify run): the pupil as an open ring, a spinner's arc. Off (no
//! network identity): a hollow pupil.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Quiet,
    Attention,
    Busy,
    Off,
}

pub const SIZE: u32 = 36;

/// The eye: centre, half-width and half-height in glyph pixels, a lens of two arcs pointed at its
/// corners (tray.svg).
const EYE: (f32, f32, f32, f32) = (17.5, 22.0, 15.0, 8.0);
/// The three machines above it, linked into it.
const NODES: [(f32, f32); 3] = [(6.0, 5.0), (17.5, 3.5), (29.0, 5.0)];

/// Distance from p to the segment a-b.
fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

/// The alpha mask of `glyph`, `SIZE` x `SIZE`, row by row.
pub fn alpha(glyph: Glyph) -> Vec<u8> {
    let n = SIZE as usize;
    let (cx, cy, w, h) = EYE;
    // the lens: the overlap of two circles through both corners
    let big = (w * w + h * h) / (2.0 * h);
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = (x as f32, y as f32);
            // the eye's outline: the signed distance to the lens (negative inside)
            let arc = |oy: f32| ((p.0 - cx).powi(2) + (p.1 - oy).powi(2)).sqrt() - big;
            let lens = arc(cy - (big - h)).max(arc(cy + (big - h)));
            let mut v = cover(lens.abs() - 1.1);
            let r = ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt();
            v = v.max(match glyph {
                Glyph::Quiet => cover(r - 4.0),
                Glyph::Attention => cover(r - 5.0),
                Glyph::Off => cover((r - 3.8).abs() - 0.9),
                // a thick ring with its top-right quarter open
                Glyph::Busy => {
                    if p.0 > cx && p.1 < cy { 0.0 } else { cover((r - 3.6).abs() - 1.3) }
                }
            });
            // the machines and their links, which stop at the eye
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
    alpha
}

#[cfg(test)]
mod tests {
    #[test]
    fn glyphs_are_distinct_full_size_masks() {
        use super::*;
        let all = [Glyph::Quiet, Glyph::Attention, Glyph::Off, Glyph::Busy].map(alpha);
        for g in &all {
            assert_eq!(g.len(), (SIZE * SIZE) as usize);
            assert_eq!(g[0], 0);
        }
        assert_ne!(all[0], all[1]);
        assert_ne!(all[0], all[2]);
        assert_ne!(all[3], all[0]);
        assert_ne!(all[3], all[2]);
    }

    /// The eye's middle is open (the outline and the pupil, not a filled hub), and the links stop
    /// at the outline: what tells it from the AI hub's centre with spokes.
    #[test]
    fn the_eye_is_an_outline_round_its_pupil() {
        use super::*;
        let quiet = alpha(Glyph::Quiet);
        let at = |x: usize, y: usize| quiet[y * SIZE as usize + x];
        let (cx, cy) = (17, 22);
        assert_eq!(at(cx, cy), 255, "the pupil");
        assert_eq!(at(cx + 9, cy), 0, "between the pupil and the outline's corner");
        assert!(at(cx, cy - 8) > 128, "the outline above the pupil");
        assert_eq!(at(cx, cy - 6), 0, "no link inside the eye");
    }
}
