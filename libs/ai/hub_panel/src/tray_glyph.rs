//! The AI Hub's tray glyphs (shared by the app and the makepad WM's "off" slot): the hub glyph of
//! the app icon (a centre with six spokes to its nodes) as a monochrome template, drawn from
//! geometry. Serving: solid; off: thin spokes, hollow centre, faint nodes; problem: ring with a bar;
//! working: grown nodes.

use makepad_widgets::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Serving,
    Working,
    Off,
    Problem,
}

pub const SIZE: u32 = 36;

/// The alpha mask of `glyph`, `SIZE` x `SIZE`.
pub fn image(glyph: Glyph) -> TrayImage {
    let n = SIZE as usize;
    let c = (n as f32 - 1.0) * 0.5;
    let spoke_r = 13.0;
    let (node_r, centre_r, spoke_w, node_a) = match glyph {
        Glyph::Serving => (3.4, 5.6, 1.7, 1.0),
        Glyph::Working => (4.4, 5.6, 1.9, 1.0),
        Glyph::Off => (2.8, 5.0, 1.0, 0.45),
        Glyph::Problem => (3.0, 6.0, 1.3, 0.7),
    };
    let nodes: Vec<(f32, f32)> = (0..6)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::TAU / 6.0;
            (c + spoke_r * a.cos(), c - spoke_r * a.sin())
        })
        .collect();
    let mut alpha = vec![0u8; n * n];
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32, y as f32);
            let mut v: f32 = 0.0;
            // Anti-aliased coverage: 1 inside, falling to 0 over one pixel.
            let cover = |d: f32| (0.5 - d).clamp(0.0, 1.0);
            for &(nx, ny) in &nodes {
                v = v.max(cover(((px - nx).powi(2) + (py - ny).powi(2)).sqrt() - node_r) * node_a);
                // The spoke from the centre's edge to the node.
                let (dx, dy) = (nx - c, ny - c);
                let len2 = dx * dx + dy * dy;
                let t = (((px - c) * dx + (py - c) * dy) / len2).clamp(0.0, 1.0);
                let (qx, qy) = (c + t * dx, c + t * dy);
                let d = ((px - qx).powi(2) + (py - qy).powi(2)).sqrt() - spoke_w * 0.5;
                if t * len2.sqrt() > centre_r {
                    v = v.max(cover(d) * node_a.max(0.6));
                }
            }
            let r = ((px - c).powi(2) + (py - c).powi(2)).sqrt();
            match glyph {
                Glyph::Serving | Glyph::Working => v = v.max(cover(r - centre_r)),
                Glyph::Off => v = v.max(cover((r - (centre_r - 0.9)).abs() - 0.9)),
                Glyph::Problem => {
                    v = v.max(cover((r - (centre_r - 1.0)).abs() - 1.0));
                    // A bar through the ring.
                    let along = (px - c + py - c) / std::f32::consts::SQRT_2;
                    let across = ((px - c) - (py - c)).abs() / std::f32::consts::SQRT_2;
                    if along.abs() <= centre_r {
                        v = v.max(cover(across - 1.1));
                    }
                }
            }
            alpha[y * n + x] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    TrayImage { width: SIZE, height: SIZE, alpha }
}

