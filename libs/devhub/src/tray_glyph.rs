//! devhub's tray glyph (shared by the app and the makepad WM's "off" slot), drawn from geometry as
//! a monochrome alpha mask (it follows a light or dark bar): the app icon's mark
//! (apps/devhub/resources/icon.svg, tray.svg), a hub with five machines linked around it.
//! Quiet: a solid hub. Attention (new inbox entries): a bigger hub and the right machine grown
//! into a badge. Off (no network identity): a hollow hub.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Quiet,
    Attention,
    Off,
}

pub const SIZE: u32 = 36;

/// The hub's centre and the ring radius the machines sit on, in glyph pixels (tray.svg).
const HUB: (f32, f32) = (18.0, 19.0);
const RING: f32 = 12.5;
/// The machines' angles round the ring, degrees: the top one, then clockwise.
const ANGLES: [f32; 5] = [-90.0, -18.0, 54.0, 126.0, 198.0];

/// Distance from p to the segment a-b.
fn segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

/// The alpha mask of `glyph`, `SIZE` x `SIZE`, row by row.
pub fn alpha(glyph: Glyph) -> Vec<u8> {
    let n = SIZE as usize;
    let (cx, cy) = HUB;
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let nodes: Vec<(f32, f32)> = ANGLES.iter().map(|a| (cx + RING * a.to_radians().cos(), cy + RING * a.to_radians().sin())).collect();
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let p = (x as f32, y as f32);
            let r = ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt();
            let mut v = match glyph {
                Glyph::Quiet => cover(r - 5.2),
                Glyph::Attention => cover(r - 6.0),
                Glyph::Off => cover((r - 4.6).abs() - 0.9),
            };
            // the machines and their links into the hub
            for (i, &node) in nodes.iter().enumerate() {
                let badge = glyph == Glyph::Attention && i == 1;
                let radius = if badge { 4.6 } else { 3.0 };
                v = v.max(cover(((p.0 - node.0).powi(2) + (p.1 - node.1).powi(2)).sqrt() - radius));
                if glyph != Glyph::Off || r > 5.6 {
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
        let all = [Glyph::Quiet, Glyph::Attention, Glyph::Off].map(alpha);
        for g in &all {
            assert_eq!(g.len(), (SIZE * SIZE) as usize);
            assert_eq!(g[0], 0);
        }
        assert_ne!(all[0], all[1]);
        assert_ne!(all[0], all[2]);
    }
}
