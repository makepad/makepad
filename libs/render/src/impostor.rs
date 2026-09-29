//! Far stand-ins for foliage and props: a model rendered from the side into
//! a small atlas, drawn as two crossed, alpha-cut quads from a distance on.
//!
//! [`bake`] rasterises the model's own layers (the packed stream the renderer
//! loads, its embedded base-colour images and cut-out masks) orthographically
//! from two horizontal directions 90 degrees apart into one RGBA atlas: view
//! 0 fills the left half, view 1 the right half. What it stores is the
//! albedo the model shows from there, darkened a little toward the crown's
//! inside and underside (a far tree has no light of its own to shade it),
//! with the colour bled into the cut-out texels so a mip never fringes.
//!
//! The model kit turns the atlas into an ordinary object (two double-sided
//! quads, one image layer, `makepadShading.impostor` = the distance), so it
//! loads, compresses, batches and cuts out like any other layer; the model
//! lane draws that layer from the distance on and every other layer before
//! it (renderer/draw_models.rs). A forest of hundreds of trees far away is
//! then one small draw of quads per species.

use crate::model::StaticModel;
use makepad_draw::ImageBuffer;

/// One view's cell: square, pixels per side.
pub const CELL: usize = 256;

pub struct ImpostorBake {
    /// 2 * CELL wide, CELL high, RGBA8 (straight alpha).
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    /// The quads: centred on (cx, cz), `half` metres to each side, from
    /// y0 to y1.
    pub cx: f32,
    pub cz: f32,
    pub half: f32,
    pub y0: f32,
    pub y1: f32,
}

fn f16_to_f32(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    s * match e {
        0 => m * 2f32.powi(-24),
        31 => f32::INFINITY,
        _ => (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

struct Layer<'a> {
    vertices: &'a [f32],
    indices: &'a [u32],
    uvs: Option<&'a [[f32; 2]]>,
    image: Option<ImageBuffer>,
    cutoff: Option<f32>,
}

/// None when the model has nothing to draw (or is itself only an impostor).
pub fn bake(glb: &[u8]) -> Option<ImpostorBake> {
    let model = StaticModel::parse_glb(glb).ok()?;
    let image = |png: &Option<Vec<u8>>| png.as_ref().and_then(|p| ImageBuffer::from_png(p).ok());
    let mut layers = Vec::new();
    if model.draw_layers.is_empty() {
        layers.push(Layer { vertices: &model.vertices, indices: &model.indices, uvs: None, image: image(&model.texture_png), cutoff: None });
    } else {
        for l in &model.draw_layers {
            let surface = l.pbr.surface.as_ref();
            if surface.is_some_and(|s| s.impostor > 0.0) { continue; }
            let cutoff = surface.filter(|s| s.alpha_mode == 1).map(|s| s.alpha_cutoff);
            layers.push(Layer { vertices: &l.vertices, indices: &l.indices, uvs: Some(&l.uvs), image: image(&l.texture_png), cutoff });
        }
    }
    const F: usize = crate::model::MODEL_VERTEX_FLOATS;
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for l in &layers {
        for v in l.vertices.chunks_exact(F) {
            for d in 0..3 { lo[d] = lo[d].min(v[d]); hi[d] = hi[d].max(v[d]); }
        }
    }
    if lo[0] > hi[0] || hi[1] - lo[1] < 1e-3 { return None; }
    let (cx, cz) = ((lo[0] + hi[0]) * 0.5, (lo[2] + hi[2]) * 0.5);
    let mut half = 0.0f32;
    for l in &layers {
        for v in l.vertices.chunks_exact(F) { half = half.max((v[0] - cx).abs()).max((v[2] - cz).abs()); }
    }
    let (y0, y1) = (lo[1], hi[1]);
    let span = (2.0 * half).max(y1 - y0);
    let half = span * 0.5;
    // Square cells: the quads are `span` on each side, floor at y0.
    let (w, h) = (CELL * 2, CELL);
    let mut rgba = vec![0u8; w * h * 4];
    let mut depth = vec![f32::MAX; w * h];
    for view in 0..2 {
        // View 0 looks along -Z (right = +X), view 1 along -X (right = -Z).
        let (rx, rz, dx, dz) = if view == 0 { (1.0, 0.0, 0.0, -1.0) } else { (0.0, -1.0, -1.0, 0.0) };
        let x_off = view * CELL;
        for l in &layers {
            for tri in l.indices.chunks_exact(3) {
                let mut sp = [[0.0f32; 3]; 3];
                let mut uv = [[0.0f32; 2]; 3];
                let mut col = [[1.0f32; 4]; 3];
                let mut world = [[0.0f32; 3]; 3];
                for k in 0..3 {
                    let i = tri[k] as usize;
                    let v = &l.vertices[i * F..i * F + F];
                    world[k] = [v[0], v[1], v[2]];
                    let (px, pz) = (v[0] - cx, v[2] - cz);
                    let u = (px * rx + pz * rz) / span + 0.5;
                    let t = 1.0 - (v[1] - y0) / span;
                    sp[k] = [u * CELL as f32, t * CELL as f32, -(px * dx + pz * dz)];
                    uv[k] = match l.uvs {
                        Some(uvs) => uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                        None => { let b = v[4].to_bits(); [f16_to_f32(b as u16), f16_to_f32((b >> 16) as u16)] }
                    };
                    let c = v[5].to_bits();
                    col[k] = [(c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0, ((c >> 24) & 0xff) as f32 / 255.0];
                }
                // Facing: how much this triangle looks up (lit crowns, dark
                // undersides).
                let (a, b, c) = (world[0], world[1], world[2]);
                let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let nl = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                let up = (n[1] / nl).abs();
                let area = (sp[1][0] - sp[0][0]) * (sp[2][1] - sp[0][1]) - (sp[1][1] - sp[0][1]) * (sp[2][0] - sp[0][0]);
                if area.abs() < 1e-6 { continue; }
                let minx = sp.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let maxx = (sp.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(CELL);
                let miny = sp.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let maxy = (sp.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(CELL);
                for py in miny..maxy {
                    for px in minx..maxx {
                        let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                        let w0 = ((sp[1][0] - fx) * (sp[2][1] - fy) - (sp[1][1] - fy) * (sp[2][0] - fx)) / area;
                        let w1 = ((sp[2][0] - fx) * (sp[0][1] - fy) - (sp[2][1] - fy) * (sp[0][0] - fx)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                        let z = w0 * sp[0][2] + w1 * sp[1][2] + w2 * sp[2][2];
                        let at = py * w + x_off + px;
                        if z >= depth[at] { continue; }
                        let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
                        let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
                        let (mut r, mut g, mut bl, mut al) = (1.0f32, 1.0f32, 1.0f32, 1.0f32);
                        if let Some(img) = &l.image {
                            let tx = ((u.rem_euclid(1.0) * img.width as f32) as usize).min(img.width - 1);
                            let ty = ((v.rem_euclid(1.0) * img.height as f32) as usize).min(img.height - 1);
                            let p = img.data[ty * img.width + tx];
                            r = ((p >> 16) & 0xff) as f32 / 255.0;
                            g = ((p >> 8) & 0xff) as f32 / 255.0;
                            bl = (p & 0xff) as f32 / 255.0;
                            al = ((p >> 24) & 0xff) as f32 / 255.0;
                        }
                        if let Some(cut) = l.cutoff { if al < cut { continue; } }
                        let tint = [0, 1, 2].map(|k| w0 * col[0][k] + w1 * col[1][k] + w2 * col[2][k]);
                        depth[at] = z;
                        // Up-facing parts a little lighter, down-facing darker.
                        let shade = 0.72 + 0.28 * up * if n[1] >= 0.0 { 1.0 } else { -0.6 };
                        let o = at * 4;
                        rgba[o] = (r * tint[0] * shade * 255.0).clamp(0.0, 255.0) as u8;
                        rgba[o + 1] = (g * tint[1] * shade * 255.0).clamp(0.0, 255.0) as u8;
                        rgba[o + 2] = (bl * tint[2] * shade * 255.0).clamp(0.0, 255.0) as u8;
                        rgba[o + 3] = 255;
                    }
                }
            }
        }
        // Depth darkening: texels far behind the silhouette's front are the
        // crown's inside, which a sunlit tree shows darker.
        let (mut zmin, mut zmax) = (f32::MAX, f32::MIN);
        for y in 0..CELL { for x in 0..CELL { let d = depth[y * w + x_off + x]; if d < f32::MAX { zmin = zmin.min(d); zmax = zmax.max(d); } } }
        if zmax > zmin {
            for y in 0..CELL {
                for x in 0..CELL {
                    let at = y * w + x_off + x;
                    let d = depth[at];
                    if d == f32::MAX { continue; }
                    let k = 1.0 - 0.35 * ((d - zmin) / (zmax - zmin));
                    for c in 0..3 { rgba[at * 4 + c] = (rgba[at * 4 + c] as f32 * k) as u8; }
                }
            }
        }
    }
    bleed(&mut rgba, w, h);
    Some(ImpostorBake { rgba, width: w, height: h, cx, cz, half, y0, y1: y0 + span })
}

/// Spread each opaque texel's colour into its cut-out neighbours (alpha
/// stays 0), so mips and bilinear reads never pull black into the edge.
fn bleed(rgba: &mut [u8], w: usize, h: usize) {
    for _ in 0..8 {
        let src = rgba.to_vec();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let o = (y * w + x) * 4;
                if src[o + 3] != 0 || src[o] != 0 || src[o + 1] != 0 || src[o + 2] != 0 { continue; }
                let (mut sum, mut n) = ([0u32; 3], 0u32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 { continue; }
                    let p = (ny as usize * w + nx as usize) * 4;
                    if src[p + 3] == 0 && src[p] == 0 && src[p + 1] == 0 && src[p + 2] == 0 { continue; }
                    for c in 0..3 { sum[c] += src[p + c] as u32; }
                    n += 1;
                }
                if n > 0 {
                    for c in 0..3 { rgba[o + c] = (sum[c] / n).max(1) as u8; }
                    changed = true;
                }
            }
        }
        if !changed { break; }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn half_floats_decode() {
        assert_eq!(super::f16_to_f32(0x3c00), 1.0);
        assert_eq!(super::f16_to_f32(0x3800), 0.5);
        assert_eq!(super::f16_to_f32(0xc000), -2.0);
    }
}
