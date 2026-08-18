//! UV-atlas back-projection baking: project per-view generated images onto the
//! mesh's UV texture, weighted by `view_weight * cos(view angle)^bake_exp`
//! with per-view depth visibility — the native counterpart of the upstream
//! `back_sample` bake + "fast" merge (weighted average over views), followed
//! by texel dilation inpainting. Exact upstream merge semantics are re-checked
//! at oracle-parity time; the weighting law and constants (bake_exp = 4,
//! trust threshold 1e-8) match `pipeline_utils.py` / `Hunyuan3DPaintConfig`.

use crate::camera::{mat4_mul, transform_point, Mat4};
use crate::mesh::TriMesh;
use crate::raster::render_gbuffer;

/// Upstream trust threshold: texels with accumulated weight above this count
/// as baked; everything else is inpainted.
pub const TRUST_EPS: f32 = 1e-8;
/// Upstream `bake_exp`.
pub const BAKE_EXP: f32 = 4.0;

pub struct BakeView<'a> {
    /// Linear RGB, interleaved, `width * height * 3`.
    pub rgb: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub mv: Mat4,
    pub proj: Mat4,
    pub weight: f32,
}

/// Per-texel mesh geometry rasterized in UV space.
pub struct TexelGeometry {
    pub size: usize,
    pub valid: Vec<bool>,
    pub position: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
    /// Source triangle index per texel (-1 where invalid).
    pub tri: Vec<i32>,
}

/// Rasterize the mesh's UV layout at `size`^2, interpolating world position and
/// normal per texel. Requires per-vertex normals and UVs; the atlas must be
/// non-overlapping (overlaps are last-write-wins and violate the contract).
pub fn rasterize_uv_geometry(mesh: &TriMesh, size: usize) -> TexelGeometry {
    assert_eq!(mesh.uvs.len(), mesh.positions.len(), "mesh must carry UV0");
    assert_eq!(mesh.normals.len(), mesh.positions.len(), "mesh must carry normals");
    let mut geo = TexelGeometry {
        size,
        valid: vec![false; size * size],
        position: vec![[0.0; 3]; size * size],
        normal: vec![[0.0; 3]; size * size],
        tri: vec![-1; size * size],
    };
    let s = size as f32;
    for (tri_index, [i0, i1, i2]) in mesh.indices.iter().enumerate() {
        let idx = [*i0 as usize, *i1 as usize, *i2 as usize];
        let t = [
            [mesh.uvs[idx[0]][0] * s, mesh.uvs[idx[0]][1] * s],
            [mesh.uvs[idx[1]][0] * s, mesh.uvs[idx[1]][1] * s],
            [mesh.uvs[idx[2]][0] * s, mesh.uvs[idx[2]][1] * s],
        ];
        // Signed area paired with the edge functions below (same convention as
        // raster.rs: area = cross(t2-t0, t1-t0), edges = cross(p-a, b-a)).
        let area = (t[2][0] - t[0][0]) * (t[1][1] - t[0][1]) - (t[2][1] - t[0][1]) * (t[1][0] - t[0][0]);
        if area.abs() < 1e-12 {
            continue;
        }
        let inv_area = 1.0 / area;
        let min_x = t.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let max_x = t.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        let min_y = t.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let max_y = t.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
        let x0 = (min_x - 0.5).floor().max(0.0) as usize;
        let x1 = ((max_x + 0.5).ceil() as isize).min(size as isize).max(0) as usize;
        let y0 = (min_y - 0.5).floor().max(0.0) as usize;
        let y1 = ((max_y + 0.5).ceil() as isize).min(size as isize).max(0) as usize;
        for py in y0..y1 {
            for px in x0..x1 {
                let p = [px as f32 + 0.5, py as f32 + 0.5];
                let e0 = (p[0] - t[1][0]) * (t[2][1] - t[1][1]) - (p[1] - t[1][1]) * (t[2][0] - t[1][0]);
                let e1 = (p[0] - t[2][0]) * (t[0][1] - t[2][1]) - (p[1] - t[2][1]) * (t[0][0] - t[2][0]);
                let e2 = (p[0] - t[0][0]) * (t[1][1] - t[0][1]) - (p[1] - t[0][1]) * (t[1][0] - t[0][0]);
                let w0 = e0 * inv_area;
                let w1 = e1 * inv_area;
                let w2 = e2 * inv_area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let ti = py * size + px;
                let mut pos = [0.0f32; 3];
                let mut n = [0.0f32; 3];
                for (&vi, w) in idx.iter().zip([w0, w1, w2]) {
                    let vp = mesh.positions[vi];
                    let vn = mesh.normals[vi];
                    pos[0] += vp[0] * w;
                    pos[1] += vp[1] * w;
                    pos[2] += vp[2] * w;
                    n[0] += vn[0] * w;
                    n[1] += vn[1] * w;
                    n[2] += vn[2] * w;
                }
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len > 1e-20 {
                    n = [n[0] / len, n[1] / len, n[2] / len];
                }
                geo.valid[ti] = true;
                geo.position[ti] = pos;
                geo.normal[ti] = n;
                geo.tri[ti] = tri_index as i32;
            }
        }
    }
    geo
}

pub struct BakeResult {
    pub size: usize,
    /// Linear RGB, weighted mean over contributing views (zeros where trust = 0).
    pub rgb: Vec<f32>,
    /// Accumulated weight per texel; `> TRUST_EPS` marks a baked texel.
    pub trust: Vec<f32>,
}

fn bilinear(rgb: &[f32], width: usize, height: usize, sx: f32, sy: f32) -> [f32; 3] {
    let fx = sx - 0.5;
    let fy = sy - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let clamp = |v: f32, hi: usize| (v.max(0.0) as usize).min(hi - 1);
    let x0i = clamp(x0, width);
    let x1i = clamp(x0 + 1.0, width);
    let y0i = clamp(y0, height);
    let y1i = clamp(y0 + 1.0, height);
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        let v00 = rgb[(y0i * width + x0i) * 3 + c];
        let v10 = rgb[(y0i * width + x1i) * 3 + c];
        let v01 = rgb[(y1i * width + x0i) * 3 + c];
        let v11 = rgb[(y1i * width + x1i) * 3 + c];
        out[c] = v00 * (1.0 - tx) * (1.0 - ty) + v10 * tx * (1.0 - ty) + v01 * (1.0 - tx) * ty + v11 * tx * ty;
    }
    out
}

/// Bake all views into a `texture_size`^2 atlas. `depth_size` is the per-view
/// visibility z-buffer resolution (upstream renders at `render_size`);
/// `depth_bias` absorbs rasterization/interpolation depth noise (NDC units).
pub fn bake_from_views(
    mesh: &TriMesh,
    views: &[BakeView],
    texture_size: usize,
    depth_size: usize,
    cos_exp: f32,
    depth_bias: f32,
) -> BakeResult {
    let geo = rasterize_uv_geometry(mesh, texture_size);
    let mut rgb_acc = vec![0.0f32; texture_size * texture_size * 3];
    let mut trust = vec![0.0f32; texture_size * texture_size];

    for view in views {
        assert_eq!(view.rgb.len(), view.width * view.height * 3, "view image size mismatch");
        let gbuf = render_gbuffer(mesh, &view.mv, &view.proj, depth_size, depth_size);
        let mvp = mat4_mul(&view.proj, &view.mv);
        // w2c row 2 is -lookat: the world-space direction from the scene toward
        // the camera (constant for the orthographic paint cameras).
        let to_cam = [view.mv[2][0], view.mv[2][1], view.mv[2][2]];

        for ti in 0..texture_size * texture_size {
            if !geo.valid[ti] {
                continue;
            }
            let n = geo.normal[ti];
            let cosv = n[0] * to_cam[0] + n[1] * to_cam[1] + n[2] * to_cam[2];
            if cosv <= 0.0 {
                continue;
            }
            let clip = transform_point(&mvp, geo.position[ti]);
            if clip[3].abs() < 1e-12 {
                continue;
            }
            let inv_w = 1.0 / clip[3];
            let ndc = [clip[0] * inv_w, clip[1] * inv_w, clip[2] * inv_w];
            let sx = (ndc[0] * 0.5 + 0.5) * view.width as f32;
            let sy = (1.0 - (ndc[1] * 0.5 + 0.5)) * view.height as f32;
            if sx < 0.0 || sy < 0.0 || sx >= view.width as f32 || sy >= view.height as f32 {
                continue;
            }
            // Visibility against the view depth buffer (nearest texel).
            let dx = ((sx / view.width as f32) * gbuf.width as f32) as usize;
            let dy = ((sy / view.height as f32) * gbuf.height as f32) as usize;
            let dx = dx.min(gbuf.width - 1);
            let dy = dy.min(gbuf.height - 1);
            let zbuf = gbuf.depth[dy * gbuf.width + dx];
            if !zbuf.is_finite() || ndc[2] > zbuf + depth_bias {
                continue;
            }
            let w = view.weight * cosv.powf(cos_exp);
            if w <= 0.0 {
                continue;
            }
            let c = bilinear(view.rgb, view.width, view.height, sx, sy);
            rgb_acc[ti * 3] += c[0] * w;
            rgb_acc[ti * 3 + 1] += c[1] * w;
            rgb_acc[ti * 3 + 2] += c[2] * w;
            trust[ti] += w;
        }
    }

    for ti in 0..texture_size * texture_size {
        if trust[ti] > TRUST_EPS {
            let inv = 1.0 / trust[ti];
            rgb_acc[ti * 3] *= inv;
            rgb_acc[ti * 3 + 1] *= inv;
            rgb_acc[ti * 3 + 2] *= inv;
        }
    }
    BakeResult {
        size: texture_size,
        rgb: rgb_acc,
        trust,
    }
}

/// Iterative 8-neighbor dilation inpaint of un-baked texels. Returns the
/// number of passes executed (stops early when nothing changes).
pub fn dilate_inpaint(rgb: &mut [f32], valid: &mut [bool], size: usize, max_passes: usize) -> usize {
    assert_eq!(rgb.len(), size * size * 3);
    assert_eq!(valid.len(), size * size);
    for pass in 0..max_passes {
        let snapshot = valid.to_vec();
        let src = rgb.to_vec();
        let mut changed = false;
        for y in 0..size {
            for x in 0..size {
                let ti = y * size + x;
                if snapshot[ti] {
                    continue;
                }
                let mut acc = [0.0f32; 3];
                let mut count = 0u32;
                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let nx = x as i32 + dx;
                        let ny = y as i32 + dy;
                        if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                            continue;
                        }
                        let ni = ny as usize * size + nx as usize;
                        if snapshot[ni] {
                            acc[0] += src[ni * 3];
                            acc[1] += src[ni * 3 + 1];
                            acc[2] += src[ni * 3 + 2];
                            count += 1;
                        }
                    }
                }
                if count > 0 {
                    let inv = 1.0 / count as f32;
                    rgb[ti * 3] = acc[0] * inv;
                    rgb[ti * 3 + 1] = acc[1] * inv;
                    rgb[ti * 3 + 2] = acc[2] * inv;
                    valid[ti] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return pass;
        }
    }
    max_passes
}

/// Bake a tangent-space normal map (+Y up, OpenGL/glTF convention) of the
/// mesh's interpolated vertex normals into its UV atlas. The tangent frame is
/// built per source triangle from its UV mapping around the *geometric* face
/// normal, so the encoded texel is the smoothed shading normal expressed in
/// that frame: hard-faced meshes bake flat (128,128,255); smoothed meshes bake
/// their shading variation. This channel is GeometryDerived — it never comes
/// from a generative model. Invalid texels get the flat neutral normal.
pub fn bake_tangent_normal_map(mesh: &TriMesh, size: usize) -> Vec<u8> {
    let geo = rasterize_uv_geometry(mesh, size);
    let mut out = vec![0u8; size * size * 3];
    let enc = |v: f32| -> u8 { ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8 };
    for ti in 0..size * size {
        let px = &mut out[ti * 3..ti * 3 + 3];
        if !geo.valid[ti] {
            px.copy_from_slice(&[128, 128, 255]);
            continue;
        }
        let [i0, i1, i2] = mesh.indices[geo.tri[ti] as usize];
        let p0 = mesh.positions[i0 as usize];
        let p1 = mesh.positions[i1 as usize];
        let p2 = mesh.positions[i2 as usize];
        let u0 = mesh.uvs[i0 as usize];
        let u1 = mesh.uvs[i1 as usize];
        let u2 = mesh.uvs[i2 as usize];
        let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
        let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
        let d1 = [u1[0] - u0[0], u1[1] - u0[1]];
        let d2 = [u2[0] - u0[0], u2[1] - u0[1]];
        // Geometric face normal is the frame's Z axis.
        let mut nf = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let nf_len = (nf[0] * nf[0] + nf[1] * nf[1] + nf[2] * nf[2]).sqrt();
        if nf_len < 1e-20 {
            px.copy_from_slice(&[128, 128, 255]);
            continue;
        }
        for c in &mut nf {
            *c /= nf_len;
        }
        let det = d1[0] * d2[1] - d2[0] * d1[1];
        let mut t = if det.abs() < 1e-12 {
            // Degenerate UV mapping: any tangent perpendicular to the normal.
            if nf[0].abs() < 0.9 {
                [1.0, 0.0, 0.0]
            } else {
                [0.0, 1.0, 0.0]
            }
        } else {
            let r = 1.0 / det;
            [
                (e1[0] * d2[1] - e2[0] * d1[1]) * r,
                (e1[1] * d2[1] - e2[1] * d1[1]) * r,
                (e1[2] * d2[1] - e2[2] * d1[1]) * r,
            ]
        };
        // Gram-Schmidt against the face normal.
        let dot_nt = nf[0] * t[0] + nf[1] * t[1] + nf[2] * t[2];
        for c in 0..3 {
            t[c] -= nf[c] * dot_nt;
        }
        let t_len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
        if t_len < 1e-12 {
            px.copy_from_slice(&[128, 128, 255]);
            continue;
        }
        for c in &mut t {
            *c /= t_len;
        }
        // +Y-up bitangent.
        let b = [
            nf[1] * t[2] - nf[2] * t[1],
            nf[2] * t[0] - nf[0] * t[2],
            nf[0] * t[1] - nf[1] * t[0],
        ];
        let n = geo.normal[ti];
        let ts = [
            n[0] * t[0] + n[1] * t[1] + n[2] * t[2],
            n[0] * b[0] + n[1] * b[1] + n[2] * b[2],
            n[0] * nf[0] + n[1] * nf[1] + n[2] * nf[2],
        ];
        px[0] = enc(ts[0]);
        px[1] = enc(ts[1]);
        px[2] = enc(ts[2]);
    }
    out
}

/// Deterministic multi-source BFS fill: every un-baked texel takes the color
/// of its nearest baked texel (FIFO fronts seeded in index order). Used after
/// a bounded [`dilate_inpaint`] to fill arbitrarily large unseen regions in
/// O(texels) instead of O(texels * passes).
pub fn nearest_fill(rgb: &mut [f32], valid: &mut [bool], size: usize) {
    assert_eq!(rgb.len(), size * size * 3);
    assert_eq!(valid.len(), size * size);
    let mut queue = std::collections::VecDeque::new();
    for (i, v) in valid.iter().enumerate() {
        if *v {
            queue.push_back(i);
        }
    }
    if queue.is_empty() {
        return;
    }
    while let Some(i) = queue.pop_front() {
        let x = (i % size) as i32;
        let y = (i / size) as i32;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0 || nx >= size as i32 || ny >= size as i32 {
                continue;
            }
            let ni = ny as usize * size + nx as usize;
            if !valid[ni] {
                valid[ni] = true;
                rgb[ni * 3] = rgb[i * 3];
                rgb[ni * 3 + 1] = rgb[i * 3 + 1];
                rgb[ni * 3 + 2] = rgb[i * 3 + 2];
                queue.push_back(ni);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{default_orthographic, model_view_matrix, CAMERA_DISTANCE};
    use crate::mesh::TriMesh;

    fn solid(width: usize, height: usize, color: [f32; 3]) -> Vec<f32> {
        let mut v = Vec::with_capacity(width * height * 3);
        for _ in 0..width * height {
            v.extend_from_slice(&color);
        }
        v
    }

    fn face_cell_center(face: usize, size: usize) -> usize {
        let col = face % 3;
        let row = face / 3;
        let x = col * size / 3 + size / 6;
        let y = row * size / 2 + size / 4;
        y * size + x
    }

    #[test]
    fn uv_geometry_covers_atlas_cells() {
        let cube = TriMesh::unit_cube_atlas();
        let geo = rasterize_uv_geometry(&cube, 48);
        for face in 0..6 {
            let ti = face_cell_center(face, 48);
            assert!(geo.valid[ti], "face {face} atlas cell center not rasterized");
        }
        // Cell centers carry the face's world geometry: +Y face has normal (0,1,0).
        let ti = face_cell_center(2, 48);
        let n = geo.normal[ti];
        assert!((n[1] - 1.0).abs() < 1e-4);
        assert!(geo.tri[ti] == 4 || geo.tri[ti] == 5, "tri id {}", geo.tri[ti]);
    }

    #[test]
    fn tangent_normal_bake_is_flat_for_hard_faces() {
        let cube = TriMesh::unit_cube_atlas();
        let a = bake_tangent_normal_map(&cube, 32);
        let b = bake_tangent_normal_map(&cube, 32);
        assert_eq!(a, b, "deterministic");
        // Hard-faced cube: shading normal == geometric normal everywhere,
        // so every texel (valid or filled) encodes flat (128,128,255).
        for px in a.chunks_exact(3) {
            assert_eq!(px, &[128, 128, 255]);
        }
    }

    #[test]
    fn single_front_view_bakes_only_facing_cell() {
        let cube = TriMesh::unit_cube_atlas();
        let img = solid(64, 64, [1.0, 0.0, 0.0]);
        let views = [BakeView {
            rgb: &img,
            width: 64,
            height: 64,
            mv: model_view_matrix(0.0, 0.0, CAMERA_DISTANCE, [0.0; 3]),
            proj: default_orthographic(1.2),
            weight: 1.0,
        }];
        let baked = bake_from_views(&cube, &views, 48, 128, BAKE_EXP, 1e-3);
        let front = face_cell_center(2, 48); // +Y face
        let back = face_cell_center(3, 48); // -Y face
        assert!(baked.trust[front] > TRUST_EPS, "front cell must be baked");
        assert!(baked.trust[back] <= TRUST_EPS, "back cell must stay empty");
        assert!((baked.rgb[front * 3] - 1.0).abs() < 1e-4);
        assert!(baked.rgb[front * 3 + 1].abs() < 1e-4);
    }

    #[test]
    fn opposing_views_bake_their_own_cells() {
        let cube = TriMesh::unit_cube_atlas();
        let red = solid(64, 64, [1.0, 0.0, 0.0]);
        let blue = solid(64, 64, [0.0, 0.0, 1.0]);
        let proj = default_orthographic(1.2);
        let views = [
            BakeView {
                rgb: &red,
                width: 64,
                height: 64,
                mv: model_view_matrix(0.0, 0.0, CAMERA_DISTANCE, [0.0; 3]),
                proj,
                weight: 1.0,
            },
            BakeView {
                rgb: &blue,
                width: 64,
                height: 64,
                mv: model_view_matrix(0.0, 180.0, CAMERA_DISTANCE, [0.0; 3]),
                proj,
                weight: 0.5,
            },
        ];
        let baked = bake_from_views(&cube, &views, 48, 128, BAKE_EXP, 1e-3);
        let front = face_cell_center(2, 48);
        let back = face_cell_center(3, 48);
        assert!((baked.rgb[front * 3] - 1.0).abs() < 1e-4, "front is red");
        assert!(baked.rgb[front * 3 + 2].abs() < 1e-4);
        assert!((baked.rgb[back * 3 + 2] - 1.0).abs() < 1e-4, "back is blue");
        assert!(baked.rgb[back * 3].abs() < 1e-4);
    }

    #[test]
    fn dilate_fills_neighbors_deterministically() {
        let size = 16;
        let mut rgb = vec![0.0f32; size * size * 3];
        let mut valid = vec![false; size * size];
        let center = 8 * size + 8;
        valid[center] = true;
        rgb[center * 3] = 1.0;
        let mut rgb2 = rgb.clone();
        let mut valid2 = valid.clone();
        let passes = dilate_inpaint(&mut rgb, &mut valid, size, 4);
        assert_eq!(passes, 4);
        assert!(valid[center - 1] && valid[center + 1] && valid[center - size]);
        assert!((rgb[(center - 1) * 3] - 1.0).abs() < 1e-6);
        let far = 8 * size + 8 - 4; // 4 texels away: reached on pass 4
        assert!(valid[far]);
        dilate_inpaint(&mut rgb2, &mut valid2, size, 4);
        assert_eq!(rgb, rgb2);
        assert_eq!(valid, valid2);
    }

    #[test]
    fn nearest_fill_completes_and_is_deterministic() {
        let size = 8;
        let mut rgb = vec![0.0f32; size * size * 3];
        let mut valid = vec![false; size * size];
        for y in 0..size {
            // Left column red, right column blue.
            valid[y * size] = true;
            rgb[(y * size) * 3] = 1.0;
            valid[y * size + size - 1] = true;
            rgb[(y * size + size - 1) * 3 + 2] = 1.0;
        }
        let mut rgb2 = rgb.clone();
        let mut valid2 = valid.clone();
        nearest_fill(&mut rgb, &mut valid, size);
        nearest_fill(&mut rgb2, &mut valid2, size);
        assert!(valid.iter().all(|v| *v), "every texel filled");
        assert_eq!(rgb, rgb2, "fill must be deterministic");
        // Texels adjacent to each source keep that source's color.
        assert!((rgb[(3 * size + 1) * 3] - 1.0).abs() < 1e-6, "near-left is red");
        assert!((rgb[(3 * size + size - 2) * 3 + 2] - 1.0).abs() < 1e-6, "near-right is blue");
    }

    #[test]
    fn bake_is_deterministic() {
        let cube = TriMesh::unit_cube_atlas();
        let img = solid(32, 32, [0.3, 0.6, 0.9]);
        let views = [BakeView {
            rgb: &img,
            width: 32,
            height: 32,
            mv: model_view_matrix(20.0, 30.0, CAMERA_DISTANCE, [0.0; 3]),
            proj: default_orthographic(1.2),
            weight: 1.0,
        }];
        let a = bake_from_views(&cube, &views, 32, 64, BAKE_EXP, 1e-3);
        let b = bake_from_views(&cube, &views, 32, 64, BAKE_EXP, 1e-3);
        assert_eq!(a.rgb, b.rgb);
        assert_eq!(a.trust, b.trust);
    }
}
