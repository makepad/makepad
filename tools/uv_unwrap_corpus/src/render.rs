//! Pictures for the eye: the atlas layout, and the mesh under a checker read
//! through its uvs, where stretched squares and broken lines show.

use crate::{Layout, Mesh};
use jpeg_encoder::{ColorType, Encoder};

/// Calls back with each texel whose centre lies in the triangle.
pub fn for_texels(q: [[f64; 2]; 3], size: usize, mut visit: impl FnMut(usize, usize)) {
    let low_x = q[0][0].min(q[1][0]).min(q[2][0]).floor().max(0.0) as usize;
    let low_y = q[0][1].min(q[1][1]).min(q[2][1]).floor().max(0.0) as usize;
    let high_x = (q[0][0].max(q[1][0]).max(q[2][0]).ceil() as usize).min(size);
    let high_y = (q[0][1].max(q[1][1]).max(q[2][1]).ceil() as usize).min(size);
    let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0]);
    if area == 0.0 {
        return;
    }
    let sign = area.signum();
    for y in low_y..high_y {
        for x in low_x..high_x {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let inside = (0..3).all(|k| {
                let (a, b) = (q[k], q[(k + 1) % 3]);
                ((b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])) * sign >= 0.0
            });
            if inside {
                visit(x, y);
            }
        }
    }
}

fn hue(i: u32) -> [u8; 3] {
    let h = i.wrapping_mul(2654435761);
    [(h >> 24) as u8 | 64, (h >> 16) as u8 | 64, (h >> 8) as u8 | 64]
}

fn save(path: String, pixels: &[u8], w: usize, h: usize) {
    let encoder = Encoder::new_file(&path, 90).unwrap();
    encoder.encode(pixels, w as u16, h as u16, ColorType::Rgb).unwrap();
}

pub fn write(dir: &str, mesh: &Mesh, layout: &Layout, chart_of: &[u32], size: u32) {
    std::fs::create_dir_all(dir).unwrap();
    let s = size as usize;
    let mut atlas = vec![24u8; s * s * 3];
    for t in 0..mesh.indices.len() / 3 {
        let color = hue(chart_of[t]);
        for_texels([layout.uv[t * 3], layout.uv[t * 3 + 1], layout.uv[t * 3 + 2]], s, |x, y| {
            atlas[(y * s + x) * 3..(y * s + x) * 3 + 3].copy_from_slice(&color);
        });
    }
    save(format!("{dir}/{}-atlas.jpg", mesh.name), &atlas, s, s);

    // Three-quarter orthographic view, depth buffered, checker every 16 texels.
    let view = 1024usize;
    let mut low = [f64::INFINITY; 3];
    let mut high = [f64::NEG_INFINITY; 3];
    for p in &mesh.positions {
        for d in 0..3 {
            low[d] = low[d].min(p[d]);
            high[d] = high[d].max(p[d]);
        }
    }
    let centre = [0, 1, 2].map(|d| (low[d] + high[d]) * 0.5);
    let radius = (0..3).map(|d| high[d] - low[d]).fold(1.0e-9f64, f64::max) * 0.9;
    let screen = |p: [f64; 3]| {
        let q = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
        let (x, z) = (q[0] * 0.8 + q[2] * 0.6, -q[0] * 0.6 + q[2] * 0.8);
        let (y, d) = (q[1] * 0.85 + z * 0.53, -q[1] * 0.53 + z * 0.85);
        [(x / radius + 0.5) * view as f64, (0.5 - y / radius) * view as f64, d]
    };
    let mut pixels = vec![32u8; view * view * 3];
    let mut depth = vec![f64::NEG_INFINITY; view * view];
    for t in 0..mesh.indices.len() / 3 {
        let p = [0, 1, 2].map(|k| screen(mesh.positions[mesh.indices[t * 3 + k] as usize]));
        let q = [[p[0][0], p[0][1]], [p[1][0], p[1][1]], [p[2][0], p[2][1]]];
        let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[1][1] - q[0][1]) * (q[2][0] - q[0][0]);
        if area == 0.0 {
            continue;
        }
        let tint = hue(chart_of[t]);
        for_texels(q, view, |x, y| {
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let w1 = ((px - q[0][0]) * (q[2][1] - q[0][1]) - (py - q[0][1]) * (q[2][0] - q[0][0])) / area;
            let w2 = ((q[1][0] - q[0][0]) * (py - q[0][1]) - (q[1][1] - q[0][1]) * (px - q[0][0])) / area;
            let w0 = 1.0 - w1 - w2;
            let d = p[0][2] * w0 + p[1][2] * w1 + p[2][2] * w2;
            if d <= depth[y * view + x] {
                return;
            }
            depth[y * view + x] = d;
            let u = layout.uv[t * 3][0] * w0 + layout.uv[t * 3 + 1][0] * w1 + layout.uv[t * 3 + 2][0] * w2;
            let v = layout.uv[t * 3][1] * w0 + layout.uv[t * 3 + 1][1] * w1 + layout.uv[t * 3 + 2][1] * w2;
            let cell = ((u / 16.0).floor() as i64 + (v / 16.0).floor() as i64) & 1;
            let shade = if cell == 0 { 1.0 } else { 0.45 };
            for c in 0..3 {
                pixels[(y * view + x) * 3 + c] = (tint[c] as f64 * shade) as u8;
            }
        });
    }
    save(format!("{dir}/{}-checker.jpg", mesh.name), &pixels, view, view);
}
