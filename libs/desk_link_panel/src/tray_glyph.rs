//! Desk Link's tray glyphs, monochrome templates drawn from geometry (they follow a light or
//! dark menu bar, and the WM tints them): a screen on a stand.
//!
//! * `Off`: a thin, faint outline (Desk Host does not serve);
//! * `Serving`: a solid outline (it listens, nobody watches);
//! * `Watched`: the screen filled (someone watches);
//! * `Viewer`: the outline with a play mark inside (the Desk Link viewer).

use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Off,
    Serving,
    Watched,
    Viewer,
}

pub const SIZE: u32 = 36;

/// The alpha mask of `glyph`, `SIZE` x `SIZE`.
pub fn image(glyph: Glyph) -> TrayImage {
    let n = SIZE as usize;
    // the screen: x 4..32, y 6..25 (pixel centres), corners rounded; the stand below it
    let (x0, x1, y0, y1, r) = (4.5f32, 31.5f32, 6.5f32, 25.0f32, 3.0f32);
    let (line, a) = match glyph {
        Glyph::Off => (1.2, 0.5),
        _ => (2.2, 1.0),
    };
    let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32, y as f32);
            // signed distance to the rounded rectangle
            let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
            let (hx, hy) = ((x1 - x0) * 0.5 - r, (y1 - y0) * 0.5 - r);
            let (qx, qy) = ((px - cx).abs() - hx, (py - cy).abs() - hy);
            let sd = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
            let mut v: f32 = match glyph {
                Glyph::Watched => cover(sd),
                _ => cover(sd.abs() - line * 0.5),
            } * a;
            // the stand: a neck and a foot
            let neck = (px - 18.0).abs() <= 1.5 && py > y1 && py <= 28.5;
            let foot = (px - 18.0).abs() <= 7.0 && (py - 29.5).abs() <= 1.0;
            if neck || foot {
                v = v.max(a);
            }
            if glyph == Glyph::Viewer {
                // a play triangle in the screen
                let (tx, ty) = (px - 15.0, py - 15.75);
                if tx >= 0.0 && tx <= 7.0 && ty.abs() <= 4.5 * (1.0 - tx / 7.0) {
                    v = v.max(1.0);
                }
            }
            alpha[y * n + x] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    TrayImage { width: SIZE, height: SIZE, alpha }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_a_distinct_full_size_mask() {
        let all = [Glyph::Off, Glyph::Serving, Glyph::Watched, Glyph::Viewer].map(image);
        for img in &all {
            assert_eq!(img.alpha.len(), (SIZE * SIZE) as usize);
            assert_eq!(img.alpha[0], 0, "the corners stay clear");
        }
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i].alpha, all[j].alpha);
            }
        }
    }
}
