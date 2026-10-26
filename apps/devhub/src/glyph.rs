//! The tray glyph, drawn from geometry as a monochrome template (it follows a light or dark bar):
//! an eye — a ring with a pupil. Quiet: thin ring, small pupil. Attention (new inbox entries):
//! a solid pupil and a badge dot at the top right. Off (no network identity): a hollow pupil.

use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Quiet,
    Attention,
    Off,
}

pub const SIZE: u32 = 36;

pub fn image(glyph: Glyph) -> TrayImage {
    let n = SIZE as usize;
    let c = (n as f32 - 1.0) * 0.5;
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32, y as f32);
            // the eye's outline: two arcs meeting at the corners, as an ellipse ring
            let (ex, ey) = ((px - c) / 15.0, (py - c) / 9.0);
            let e = (ex * ex + ey * ey).sqrt();
            let mut v = cover(((e - 1.0) * 9.0).abs() - 1.0);
            let r = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            v = v.max(match glyph {
                Glyph::Quiet => cover(r - 4.2),
                Glyph::Attention => cover(r - 5.6),
                Glyph::Off => cover((r - 4.2).abs() - 0.9),
            });
            if glyph == Glyph::Attention {
                let b = ((px - 30.0).powi(2) + (py - 6.0).powi(2)).sqrt();
                v = v.max(cover(b - 4.5));
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
