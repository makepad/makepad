//! Far stand-ins for foliage and props: a model rendered from many directions
//! into an atlas pair, drawn as one camera-facing quad from a distance on.
//!
//! [`bake`] rasterises the model's own layers (the packed stream the renderer
//! loads, its embedded base-colour images and cut-out masks) orthographically
//! from [`GRID`] x [`GRID`] directions over the upper hemisphere, laid out
//! hemi-octahedrally: grid point (i, j) maps to u = i / (GRID - 1) * 2 - 1,
//! v = j / (GRID - 1) * 2 - 1, and the direction toward the viewer is
//! normalize((u + v) / 2, 1 - |x| - |z|, (u - v) / 2) — the centre cell is the
//! view from straight above, the rim cells the views along the ground. So a
//! racing camera at a tree's foot and a flight camera above the canopy both
//! find frames made for them.
//!
//! Each frame stores two images: the albedo the model shows from there,
//! times an occlusion term (the vertex AO and a darkening of texels deep
//! behind the silhouette's front, a crown's inside), alpha = coverage, colour
//! bled into the cut-out texels so mips never fringe; and the normal in that
//! view's own frame, x and y in red and green (normal layers are
//! block-compressed to two channels on the GPU; z is rebuilt, the viewer's
//! side). The draw (shaders/impostor.rs) blends the three
//! frames nearest to the view and lights the result with the real sun, so a
//! far forest turns with the camera and shades with the time of day instead
//! of showing one baked picture.
//!
//! The quad carries what the shader needs in its own vertices: positions
//! are the bounding sphere's centre plus the corner times the radius in the
//! model's XY plane, and the uvs are (corner x radius), so the vertex stage
//! recovers centre and radius without a new material field.

use crate::model::StaticModel;
use makepad_draw::ImageBuffer;

/// Views per side of the hemi-octahedral grid. 6 x 6 views 18 degrees
/// apart: the shader blends the three nearest, so the step does not show,
/// and every texel of the atlas is block-compressed on a model's first load
/// (an 8 x 8 grid doubled that encode on a cold start).
pub const GRID: usize = 6;
/// The same, for the shaders (`#(..)` interpolation): the grid size and its
/// last index as floats.
pub const GRID_F: f64 = GRID as f64;
pub const GRID_LAST_F: f64 = (GRID - 1) as f64;
/// The bounding sphere fills this much of a view's cell: the rest is a
/// gutter of the view's own bled colour, so the filtered reads of a far,
/// minified impostor never pull in the neighbouring view (a dark seam
/// through the crown).
pub const FILL: f32 = 0.88;
pub const FILL_F: f64 = FILL as f64;
/// The colour atlas's alpha on texels a leaf card covers (255: the solid
/// core, a trunk); 0 is uncovered. Both count as covered for the draw.
pub const LEAF_ALPHA: u8 = 200;

pub struct ImpostorBake {
    /// size x size RGBA8 (straight alpha): albedo, alpha = coverage.
    pub albedo: Vec<u8>,
    /// size/2 x size/2 RGBA8: the view-space normal's x, y * 0.5 + 0.5
    /// in rg. Half the albedo's resolution: shading varies slower than the
    /// silhouette, and every texel here is block-compressed on first load.
    pub normal: Vec<u8>,
    /// The albedo atlas's edge (the normal atlas's is half of it).
    pub size: usize,
    /// The bounding sphere the frames were taken around (model space).
    pub center: [f32; 3],
    pub radius: f32,
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

/// The direction toward the viewer of grid point (i, j) (shaders/impostor.rs
/// `imp_frame_dir` is the same formula).
pub fn frame_dir(i: usize, j: usize) -> [f32; 3] {
    let k = (GRID - 1) as f32;
    let u = i as f32 / k * 2.0 - 1.0;
    let v = j as f32 / k * 2.0 - 1.0;
    let (x, z) = ((u + v) * 0.5, (u - v) * 0.5);
    let y = 1.0 - x.abs() - z.abs();
    norm([x, y.max(0.0), z])
}

/// A frame's image axes (right, up) for its view direction `f` (toward the
/// viewer). The shader builds the same basis.
pub fn frame_basis(f: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let right = if f[1] > 0.999 { [1.0, 0.0, 0.0] } else { norm(cross([0.0, 1.0, 0.0], f)) };
    (right, cross(f, right))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-12);
    [a[0] / l, a[1] / l, a[2] / l]
}

struct Layer<'a> {
    vertices: &'a [f32],
    indices: &'a [u32],
    uvs: Option<&'a [[f32; 2]]>,
    image: Option<ImageBuffer>,
    cutoff: Option<f32>,
    /// The image's box-filtered mip chain (level 0 = the image): a view
    /// texel reads the level that matches its footprint on the image.
    mips: Vec<(usize, usize, Vec<u32>)>,
}

/// Box-filtered mips of a straight-alpha ARGB image, colour weighted by
/// alpha (no dark fringes where leaves meet the cut-out), down to 1x1.
fn image_mips(img: &ImageBuffer) -> Vec<(usize, usize, Vec<u32>)> {
    let mut out = vec![(img.width, img.height, img.data[..img.width * img.height].to_vec())];
    while let Some((w, h, d)) = out.last().filter(|(w, h, _)| *w > 1 || *h > 1) {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut nd = vec![0u32; nw * nh];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0u32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = d[(2 * y + dy).min(h - 1) * w + (2 * x + dx).min(w - 1)];
                    let a = p >> 24;
                    acc[0] += ((p >> 16) & 0xff) * a;
                    acc[1] += ((p >> 8) & 0xff) * a;
                    acc[2] += (p & 0xff) * a;
                    acc[3] += a;
                }
                let a = acc[3].max(1);
                nd[y * nw + x] = ((acc[3] / 4) << 24) | ((acc[0] / a) << 16) | ((acc[1] / a) << 8) | (acc[2] / a);
            }
        }
        out.push((nw, nh, nd));
    }
    out
}

/// A triangle ready to rasterise into every frame: model-space corners, uvs,
/// vertex tint+AO, and the normal it shades with.
struct Tri<'a> {
    p: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    col: [[f32; 4]; 3],
    n: [f32; 3],
    /// The corners' shading normals (the mesh's own, smooth where the
    /// model is smooth).
    vn: [[f32; 3]; 3],
    layer: &'a Layer<'a>,
}

/// None when the model has nothing to draw (or is itself only an impostor).
/// `cell`: each view's pixels per side (even; the atlas is GRID cells).
pub fn bake(glb: &[u8], cell: usize) -> Option<ImpostorBake> {
    let cell = (cell.max(8) + 1) & !1;
    let model = StaticModel::parse_glb(glb).ok()?;
    let image = |png: &Option<Vec<u8>>| png.as_ref().and_then(|p| ImageBuffer::from_png(p).ok());
    let mut layers = Vec::new();
    if model.draw_layers.is_empty() {
        layers.push(Layer { vertices: &model.vertices, indices: &model.indices, uvs: None, image: image(&model.texture_png), cutoff: None, mips: Vec::new() });
    } else {
        for l in &model.draw_layers {
            let surface = l.pbr.surface.as_ref();
            if surface.is_some_and(|s| s.impostor > 0.0) { continue; }
            let cutoff = surface.filter(|s| s.alpha_mode == 1).map(|s| s.alpha_cutoff);
            layers.push(Layer { vertices: &l.vertices, indices: &l.indices, uvs: Some(&l.uvs), image: image(&l.texture_png), cutoff, mips: Vec::new() });
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
    let center = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5, (lo[2] + hi[2]) * 0.5];
    let mut radius = 0.0f32;
    for l in &layers {
        for v in l.vertices.chunks_exact(F) {
            radius = radius.max(dot([v[0] - center[0], v[1] - center[1], v[2] - center[2]], [v[0] - center[0], v[1] - center[1], v[2] - center[2]]));
        }
    }
    let radius = radius.sqrt().max(1e-3);
    // The crown's centre (where leaves bend their normals away from): the
    // middle of the cut-out layers' bounds, else the model's.
    let (mut clo, mut chi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for l in layers.iter().filter(|l| l.cutoff.is_some()) {
        for v in l.vertices.chunks_exact(F) {
            for d in 0..3 { clo[d] = clo[d].min(v[d]); chi[d] = chi[d].max(v[d]); }
        }
    }
    let crown = if clo[0] <= chi[0] { [(clo[0] + chi[0]) * 0.5, (clo[1] + chi[1]) * 0.5, (clo[2] + chi[2]) * 0.5] } else { center };

    // Triangles once; every frame reuses them.
    for l in &mut layers {
        if let Some(img) = &l.image { l.mips = image_mips(img); }
    }
    let mut tris = Vec::new();
    for l in &layers {
        for tri in l.indices.chunks_exact(3) {
            let mut t = Tri { p: [[0.0; 3]; 3], uv: [[0.0; 2]; 3], col: [[1.0; 4]; 3], n: [0.0; 3], vn: [[0.0; 3]; 3], layer: l };
            for k in 0..3 {
                let i = tri[k] as usize;
                let Some(v) = l.vertices.get(i * F..i * F + F) else { continue };
                t.p[k] = [v[0], v[1], v[2]];
                let (ox, oy) = crate::skin::unpack2f16_pub(v[3]);
                let n = crate::skin::oct_decode_pub(ox, oy);
                t.vn[k] = [n.x, n.y, n.z];
                t.uv[k] = match l.uvs {
                    Some(uvs) => uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                    None => { let b = v[4].to_bits(); [f16_to_f32(b as u16), f16_to_f32((b >> 16) as u16)] }
                };
                let c = v[5].to_bits();
                t.col[k] = [(c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0, ((c >> 24) & 0xff) as f32 / 255.0];
            }
            let e1 = [t.p[1][0] - t.p[0][0], t.p[1][1] - t.p[0][1], t.p[1][2] - t.p[0][2]];
            let e2 = [t.p[2][0] - t.p[0][0], t.p[2][1] - t.p[0][1], t.p[2][2] - t.p[0][2]];
            let n = cross(e1, e2);
            if dot(n, n) < 1e-14 { continue; }
            t.n = norm(n);
            tris.push(t);
        }
    }

    let w = GRID * cell;
    let mut albedo = vec![0u8; w * w * 4];
    let mut normal = vec![0u8; w * w * 4];
    let mut depth = vec![f32::MIN; cell * cell];
    let mut occ = vec![0.0f32; cell * cell];
    for j in 0..GRID {
        for i in 0..GRID {
            let f = frame_dir(i, j);
            let (right, up) = frame_basis(f);
            let (x_off, y_off) = (i * cell, j * cell);
            depth.fill(f32::MIN);
            for t in &tris {
                let mut sp = [[0.0f32; 3]; 3];
                for k in 0..3 {
                    let d = [t.p[k][0] - center[0], t.p[k][1] - center[1], t.p[k][2] - center[2]];
                    sp[k] = [(dot(d, right) / radius * 0.5 * FILL + 0.5) * cell as f32, (0.5 - dot(d, up) / radius * 0.5 * FILL) * cell as f32, dot(d, f)];
                }
                let area = (sp[1][0] - sp[0][0]) * (sp[2][1] - sp[0][1]) - (sp[1][1] - sp[0][1]) * (sp[2][0] - sp[0][0]);
                if area.abs() < 1e-6 { continue; }
                // The side the viewer sees (the face's; per texel the mesh's own
                // interpolated normal on that side, below); leaves bend toward
                // "out of the crown" per texel so a crown shades as a soft volume.
                let n_side = if dot(t.n, f) < 0.0 { [-t.n[0], -t.n[1], -t.n[2]] } else { t.n };
                let leaf = t.layer.cutoff.is_some();
                // The image level whose texels match one view texel's footprint
                // on it: a point read of the full image aliased a leaf card's
                // texture into speckle, which a magnified stand-in shows as
                // blotchy squares.
                let level = t.layer.mips.first().map_or(0, |&(iw, ih, _)| {
                    let (e1, e2) = ([sp[1][0] - sp[0][0], sp[1][1] - sp[0][1]], [sp[2][0] - sp[0][0], sp[2][1] - sp[0][1]]);
                    let (d1, d2) = ([t.uv[1][0] - t.uv[0][0], t.uv[1][1] - t.uv[0][1]], [t.uv[2][0] - t.uv[0][0], t.uv[2][1] - t.uv[0][1]]);
                    let dx = [(d1[0] * e2[1] - d2[0] * e1[1]) / area * iw as f32, (d1[1] * e2[1] - d2[1] * e1[1]) / area * ih as f32];
                    let dy = [(d2[0] * e1[0] - d1[0] * e2[0]) / area * iw as f32, (d2[1] * e1[0] - d1[1] * e2[0]) / area * ih as f32];
                    let fp = (dx[0] * dx[0] + dx[1] * dx[1]).max(dy[0] * dy[0] + dy[1] * dy[1]).sqrt();
                    if fp > 1.0 { (fp.log2().floor() as usize).min(t.layer.mips.len() - 1) } else { 0 }
                });
                let minx = sp.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let maxx = (sp.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cell);
                let miny = sp.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                let maxy = (sp.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cell);
                for py in miny..maxy {
                    for px in minx..maxx {
                        let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                        let w0 = ((sp[1][0] - fx) * (sp[2][1] - fy) - (sp[1][1] - fy) * (sp[2][0] - fx)) / area;
                        let w1 = ((sp[2][0] - fx) * (sp[0][1] - fy) - (sp[2][1] - fy) * (sp[0][0] - fx)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 { continue; }
                        let z = w0 * sp[0][2] + w1 * sp[1][2] + w2 * sp[2][2];
                        let cell = py * cell + px;
                        if z <= depth[cell] { continue; }
                        let u = w0 * t.uv[0][0] + w1 * t.uv[1][0] + w2 * t.uv[2][0];
                        let v = w0 * t.uv[0][1] + w1 * t.uv[1][1] + w2 * t.uv[2][1];
                        let (mut r, mut g, mut bl, mut al) = (1.0f32, 1.0f32, 1.0f32, 1.0f32);
                        if let Some((iw, ih, data)) = t.layer.mips.get(level) {
                            let (iw, ih) = (*iw, *ih);
                            let tx = ((u.rem_euclid(1.0) * iw as f32) as usize).min(iw - 1);
                            let ty = ((v.rem_euclid(1.0) * ih as f32) as usize).min(ih - 1);
                            let p = data[ty * iw + tx];
                            r = ((p >> 16) & 0xff) as f32 / 255.0;
                            g = ((p >> 8) & 0xff) as f32 / 255.0;
                            bl = (p & 0xff) as f32 / 255.0;
                            al = ((p >> 24) & 0xff) as f32 / 255.0;
                        }
                        if let Some(cut) = t.layer.cutoff { if al < cut { continue; } }
                        let c = [0, 1, 2, 3].map(|k| w0 * t.col[0][k] + w1 * t.col[1][k] + w2 * t.col[2][k]);
                        depth[cell] = z;
                        occ[cell] = c[3];
                        let o = ((y_off + py) * w + x_off + px) * 4;
                        albedo[o] = (r * c[0] * 255.0).clamp(0.0, 255.0) as u8;
                        albedo[o + 1] = (g * c[1] * 255.0).clamp(0.0, 255.0) as u8;
                        albedo[o + 2] = (bl * c[2] * 255.0).clamp(0.0, 255.0) as u8;
                        // Alpha tells leaves (LEAF_ALPHA) from the solid core and
                        // trunk (255): the stand-in's shadow lets light through
                        // some leaf texels (shaders/lm_depth.rs).
                        albedo[o + 3] = if t.layer.cutoff.is_some() { LEAF_ALPHA } else { 255 };
                        // A leaf texel's normal leans out from the crown's
                        // middle at its own position: bent once per card
                        // (at its centroid) each card lit flat, and a
                        // magnified stand-in showed the crown in squares.
                        // The interpolated vertex normal (a low-poly crown core
                        // shaded with its face normals showed its facets as
                        // squares on a magnified stand-in).
                        let vi = [0, 1, 2].map(|d| w0 * t.vn[0][d] + w1 * t.vn[1][d] + w2 * t.vn[2][d]);
                        let mut n = if dot(vi, vi) > 1e-6 {
                            let vi = norm(vi);
                            if dot(vi, n_side) < 0.0 { [-vi[0], -vi[1], -vi[2]] } else { vi }
                        } else { n_side };
                        if leaf {
                            let m = [0, 1, 2].map(|d| w0 * t.p[0][d] + w1 * t.p[1][d] + w2 * t.p[2][d] - crown[d]);
                            if dot(m, m) > 1e-8 {
                                let m = norm(m);
                                n = norm([n[0] + m[0] * 1.5, n[1] + m[1] * 1.5, n[2] + m[2] * 1.5]);
                            }
                        }
                        let e = view_encode(n, right, up, f);
                        normal[o] = ((e[0] * 0.5 + 0.5) * 255.0).round() as u8;
                        normal[o + 1] = ((e[1] * 0.5 + 0.5) * 255.0).round() as u8;
                        normal[o + 2] = 128;
                        normal[o + 3] = 255;
                    }
                }
            }
            // Occlusion, multiplied into the albedo (the normal atlas is
            // block-compressed to two channels, see the module docs): the
            // vertex AO times a darkening of texels far behind the
            // silhouette's front (the crown's inside, which a sunlit tree
            // shows darker in direct and ambient light alike).
            let (mut zmin, mut zmax) = (f32::MAX, f32::MIN);
            for &d in &depth { if d > f32::MIN { zmin = zmin.min(d); zmax = zmax.max(d); } }
            let span = (zmax - zmin).max(1e-4);
            for py in 0..cell {
                for px in 0..cell {
                    let cell = py * cell + px;
                    if depth[cell] == f32::MIN { continue; }
                    let k = occ[cell] * (1.0 - 0.4 * ((zmax - depth[cell]) / span));
                    let o = ((y_off + py) * w + x_off + px) * 4;
                    for c in 0..3 { albedo[o + c] = (albedo[o + c] as f32 * k) as u8; }
                }
            }
        }
    }
    bleed(&mut albedo, &mut normal, w);
    let normal = half(&normal, w);
    Some(ImpostorBake { albedo, normal, size: w, center, radius })
}

/// A normal in its view's own frame (right, up, toward the viewer), the
/// viewer's side only: x and y are stored, z is rebuilt. Every texel then
/// lies on one smooth hemisphere, so the GPU's filtering and the half-size
/// average blend neighbours correctly (an octahedral encoding of the
/// model-space normal folds at the back hemisphere, and filtering across the
/// fold drew dark seams through the crowns).
fn view_encode(n: [f32; 3], right: [f32; 3], up: [f32; 3], f: [f32; 3]) -> [f32; 2] {
    let v = norm([dot(n, right), dot(n, up), dot(n, f).max(0.05)]);
    [v[0], v[1]]
}

/// The normal atlas at half resolution: each 2x2 block's view-space normals
/// averaged and encoded again.
fn half(normal: &[u8], w: usize) -> Vec<u8> {
    let h = w / 2;
    let mut out = vec![0u8; h * h * 4];
    let dec = |o: usize| -> [f32; 3] {
        let e = [normal[o] as f32 / 255.0 * 2.0 - 1.0, normal[o + 1] as f32 / 255.0 * 2.0 - 1.0];
        [e[0], e[1], (1.0 - e[0] * e[0] - e[1] * e[1]).max(0.0).sqrt()]
    };
    for y in 0..h {
        for x in 0..h {
            let mut n = [0.0f32; 3];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let d = dec(((2 * y + dy) * w + 2 * x + dx) * 4);
                for c in 0..3 { n[c] += d[c]; }
            }
            let n = norm(n);
            let e = [n[0], n[1]];
            let o = (y * h + x) * 4;
            out[o] = ((e[0] * 0.5 + 0.5) * 255.0).round() as u8;
            out[o + 1] = ((e[1] * 0.5 + 0.5) * 255.0).round() as u8;
            out[o + 2] = 128;
            out[o + 3] = 255;
        }
    }
    out
}

/// Spread each covered texel's colour and normal into its uncovered
/// neighbours (alpha stays 0), so mips and bilinear reads never pull black
/// or a zero normal into the silhouette.
fn bleed(albedo: &mut [u8], normal: &mut [u8], w: usize) {
    let mut filled: Vec<bool> = albedo.chunks_exact(4).map(|p| p[3] != 0).collect();
    let mut front: Vec<usize> = Vec::new();
    for _ in 0..8 {
        front.clear();
        for y in 0..w {
            for x in 0..w {
                let at = y * w + x;
                if filled[at] { continue; }
                let (mut sa, mut sn, mut n) = ([0u32; 3], [0u32; 4], 0u32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= w as i32 { continue; }
                    let p = ny as usize * w + nx as usize;
                    if !filled[p] { continue; }
                    for c in 0..3 { sa[c] += albedo[p * 4 + c] as u32; }
                    for c in 0..4 { sn[c] += normal[p * 4 + c] as u32; }
                    n += 1;
                }
                if n > 0 {
                    for c in 0..3 { albedo[at * 4 + c] = (sa[c] / n) as u8; }
                    for c in 0..4 { normal[at * 4 + c] = (sn[c] / n) as u8; }
                    front.push(at);
                }
            }
        }
        if front.is_empty() { break; }
        for &at in &front { filled[at] = true; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_floats_decode() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xc000), -2.0);
    }

    /// The grid's centre is the view from above, its rim the views along
    /// the ground, and every frame's basis is orthonormal.
    #[test]
    fn hemi_octahedral_frames() {
        let top = frame_dir(GRID / 2, GRID / 2);
        assert!(top[1] > 0.8, "{top:?}");
        let rim = frame_dir(GRID - 1, 0);
        assert!(rim[1].abs() < 1e-5, "{rim:?}");
        for j in 0..GRID {
            for i in 0..GRID {
                let f = frame_dir(i, j);
                let (r, u) = frame_basis(f);
                assert!(dot(r, f).abs() < 1e-4 && dot(u, f).abs() < 1e-4 && dot(r, u).abs() < 1e-4);
                assert!((dot(u, u) - 1.0).abs() < 1e-3 && (dot(r, r) - 1.0).abs() < 1e-3);
            }
        }
    }

    /// The shader's encode (view direction -> grid coordinate) inverts the
    /// bake's frame directions.
    #[test]
    fn frame_lookup_round_trips() {
        for j in 0..GRID {
            for i in 0..GRID {
                let d = frame_dir(i, j);
                let s = d[0].abs() + d[1].abs() + d[2].abs();
                let (x, z) = (d[0] / s, d[2] / s);
                let (u, v) = (x + z, x - z);
                let g = [(u * 0.5 + 0.5) * (GRID - 1) as f32, (v * 0.5 + 0.5) * (GRID - 1) as f32];
                assert!((g[0] - i as f32).abs() < 1e-3 && (g[1] - j as f32).abs() < 1e-3, "{i},{j} -> {g:?}");
            }
        }
    }
}
