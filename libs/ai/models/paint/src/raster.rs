//! Deterministic CPU G-buffer rasterizer for the geometry-conditioning stage.
//!
//! Renders face-index / depth / world-normal / world-position / UV buffers for
//! a mesh under the upstream camera model (orthographic; see [`crate::camera`]).
//! Replaces the upstream `custom_rasterizer` CUDA extension for geometry
//! conditioning: at 512..2048 px and <=30 candidate views a scalar CPU pass is
//! plenty, and byte-deterministic. Interpolation is affine (exact for the
//! orthographic projection used by the paint pipeline).
//!
//! Image convention: row 0 is the top of the image (NDC +Y maps to row 0),
//! pixels sample at their centers. Screen alignment against the upstream
//! rasterizer is re-verified at oracle-parity time.

use crate::camera::{mat4_mul, transform_point, Mat4};
use crate::mesh::TriMesh;
use std::collections::BTreeSet;

#[derive(Clone, Debug)]
pub struct GBuffer {
    pub width: usize,
    pub height: usize,
    /// Triangle index per pixel, -1 for background.
    pub face_index: Vec<i32>,
    /// NDC depth per pixel (smaller = closer); +INF for background.
    pub depth: Vec<f32>,
    /// Interpolated unit world-space normal ("absolute coordinates").
    pub normal: Vec<[f32; 3]>,
    /// Interpolated world-space position.
    pub position: Vec<[f32; 3]>,
    /// Interpolated UV0 (zeros when the mesh has no UVs).
    pub uv: Vec<[f32; 2]>,
}

impl GBuffer {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            face_index: vec![-1; width * height],
            depth: vec![f32::INFINITY; width * height],
            normal: vec![[0.0; 3]; width * height],
            position: vec![[0.0; 3]; width * height],
            uv: vec![[0.0; 2]; width * height],
        }
    }

    /// The set of visible triangle indices (input to bake-view selection).
    pub fn visible_faces(&self) -> BTreeSet<u32> {
        self.face_index
            .iter()
            .filter(|&&f| f >= 0)
            .map(|&f| f as u32)
            .collect()
    }

    /// Fraction of pixels covered by geometry.
    pub fn coverage(&self) -> f64 {
        let covered = self.face_index.iter().filter(|&&f| f >= 0).count();
        let Some(pixels) = self.width.checked_mul(self.height) else {
            return 0.0;
        };
        if pixels == 0 {
            0.0
        } else {
            covered as f64 / pixels as f64
        }
    }
}

/// Render the mesh with world-to-camera `mv` and projection `proj`.
/// Requires per-vertex normals (see [`TriMesh::compute_vertex_normals`]).
pub fn render_gbuffer(mesh: &TriMesh, mv: &Mat4, proj: &Mat4, width: usize, height: usize) -> GBuffer {
    assert_eq!(
        mesh.normals.len(),
        mesh.positions.len(),
        "render_gbuffer requires per-vertex normals"
    );
    let has_uv = mesh.uvs.len() == mesh.positions.len();
    let mvp = mat4_mul(proj, mv);
    let mut gbuf = GBuffer::new(width, height);

    for (face, [i0, i1, i2]) in mesh.indices.iter().enumerate() {
        let idx = [*i0 as usize, *i1 as usize, *i2 as usize];
        let mut screen = [[0.0f32; 3]; 3];
        let mut skip = false;
        for (corner, &vi) in idx.iter().enumerate() {
            let clip = transform_point(&mvp, mesh.positions[vi]);
            if clip.iter().any(|value| !value.is_finite())
                || clip[3].abs() < 1e-12
            {
                skip = true;
                break;
            }
            let inv_w = 1.0 / clip[3];
            let ndc = [clip[0] * inv_w, clip[1] * inv_w, clip[2] * inv_w];
            if ndc.iter().any(|value| !value.is_finite()) {
                skip = true;
                break;
            }
            let projected = [
                (ndc[0] * 0.5 + 0.5) * width as f32,
                (1.0 - (ndc[1] * 0.5 + 0.5)) * height as f32,
                ndc[2],
            ];
            if projected.iter().any(|value| !value.is_finite()) {
                skip = true;
                break;
            }
            screen[corner] = projected;
        }
        if skip {
            continue;
        }
        let area = edge(&screen[0], &screen[1], &screen[2]);
        if !area.is_finite() || area.abs() < 1e-9 {
            continue;
        }
        let min_x = screen.iter().map(|s| s[0]).fold(f32::INFINITY, f32::min);
        let max_x = screen.iter().map(|s| s[0]).fold(f32::NEG_INFINITY, f32::max);
        let min_y = screen.iter().map(|s| s[1]).fold(f32::INFINITY, f32::min);
        let max_y = screen.iter().map(|s| s[1]).fold(f32::NEG_INFINITY, f32::max);
        let clamp_bound = |value: f32, dimension: usize| -> usize {
            value.clamp(0.0, dimension as f32) as usize
        };
        let x0 = clamp_bound((min_x - 0.5).floor(), width);
        let x1 = clamp_bound((max_x + 0.5).ceil(), width);
        let y0 = clamp_bound((min_y - 0.5).floor(), height);
        let y1 = clamp_bound((max_y + 0.5).ceil(), height);
        if x0 >= x1 || y0 >= y1 {
            continue;
        }

        let inv_area = 1.0 / area;
        for py in y0..y1 {
            for px in x0..x1 {
                let p = [px as f32 + 0.5, py as f32 + 0.5, 0.0];
                let w0 = edge(&screen[1], &screen[2], &p) * inv_area;
                let w1 = edge(&screen[2], &screen[0], &p) * inv_area;
                let w2 = edge(&screen[0], &screen[1], &p) * inv_area;
                if !w0.is_finite()
                    || !w1.is_finite()
                    || !w2.is_finite()
                    || w0 < 0.0
                    || w1 < 0.0
                    || w2 < 0.0
                {
                    continue;
                }
                let z = w0 * screen[0][2] + w1 * screen[1][2] + w2 * screen[2][2];
                if !z.is_finite() {
                    continue;
                }
                let pi = py * width + px;
                if z >= gbuf.depth[pi] {
                    continue;
                }
                let mut n = [0.0f32; 3];
                let mut pos = [0.0f32; 3];
                let mut uv = [0.0f32; 2];
                for (&vi, w) in idx.iter().zip([w0, w1, w2]) {
                    let vn = mesh.normals[vi];
                    let vp = mesh.positions[vi];
                    n[0] += vn[0] * w;
                    n[1] += vn[1] * w;
                    n[2] += vn[2] * w;
                    pos[0] += vp[0] * w;
                    pos[1] += vp[1] * w;
                    pos[2] += vp[2] * w;
                    if has_uv {
                        let vt = mesh.uvs[vi];
                        uv[0] += vt[0] * w;
                        uv[1] += vt[1] * w;
                    }
                }
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                if len > 1e-20 {
                    n = [n[0] / len, n[1] / len, n[2] / len];
                }
                if n.iter().any(|value| !value.is_finite())
                    || pos.iter().any(|value| !value.is_finite())
                    || uv.iter().any(|value| !value.is_finite())
                {
                    continue;
                }
                gbuf.depth[pi] = z;
                gbuf.face_index[pi] = face as i32;
                gbuf.normal[pi] = n;
                gbuf.position[pi] = pos;
                gbuf.uv[pi] = uv;
            }
        }
    }
    gbuf
}

fn edge(a: &[f32; 3], b: &[f32; 3], p: &[f32; 3]) -> f32 {
    (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// World-normal conditioning image: rgb = n * 0.5 + 0.5 (the upstream
/// `use_abs_coor=True` world-space normal map).
pub fn normal_map_rgb8(gbuf: &GBuffer, background: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(gbuf.width * gbuf.height * 3);
    for i in 0..gbuf.width * gbuf.height {
        if gbuf.face_index[i] < 0 {
            out.extend_from_slice(&background);
        } else {
            let n = gbuf.normal[i];
            out.push(to_u8(n[0] * 0.5 + 0.5));
            out.push(to_u8(n[1] * 0.5 + 0.5));
            out.push(to_u8(n[2] * 0.5 + 0.5));
        }
    }
    out
}

/// Position conditioning image using the upstream encoding
/// `enc = 0.5 - p / scale_factor` (MeshRender texture-position convention).
pub fn position_map_rgb8(gbuf: &GBuffer, scale_factor: f32, background: [u8; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(gbuf.width * gbuf.height * 3);
    for i in 0..gbuf.width * gbuf.height {
        if gbuf.face_index[i] < 0 {
            out.extend_from_slice(&background);
        } else {
            let p = gbuf.position[i];
            out.push(to_u8(0.5 - p[0] / scale_factor));
            out.push(to_u8(0.5 - p[1] / scale_factor));
            out.push(to_u8(0.5 - p[2] / scale_factor));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{
        default_orthographic, mat4_identity, model_view_matrix, CAMERA_DISTANCE,
    };
    use crate::mesh::TriMesh;

    fn front_gbuffer(size: usize) -> GBuffer {
        let cube = TriMesh::unit_cube();
        let mv = model_view_matrix(0.0, 0.0, CAMERA_DISTANCE, [0.0; 3]);
        let proj = default_orthographic(1.2);
        render_gbuffer(&cube, &mv, &proj, size, size)
    }

    #[test]
    fn front_view_sees_only_plus_y_face() {
        let gbuf = front_gbuffer(64);
        // Cube faces are ordered [+X,-X,+Y,-Y,+Z,-Z]; +Y face is triangles 4 and 5.
        let visible = gbuf.visible_faces();
        assert_eq!(visible, [4u32, 5u32].into_iter().collect());
    }

    #[test]
    fn coverage_matches_ortho_footprint() {
        let gbuf = front_gbuffer(128);
        // Unit cube face (side 1) inside an ortho box of side 1.2:
        // covered fraction = (1/1.2)^2 = 0.6944...
        let expect = (1.0f64 / 1.2) * (1.0 / 1.2);
        assert!(
            (gbuf.coverage() - expect).abs() < 0.03,
            "coverage {} expect {}",
            gbuf.coverage(),
            expect
        );
    }

    #[test]
    fn center_pixel_normal_position_depth() {
        let gbuf = front_gbuffer(64);
        let c = 32 * 64 + 32;
        assert!(gbuf.face_index[c] == 4 || gbuf.face_index[c] == 5);
        let n = gbuf.normal[c];
        assert!((n[0]).abs() < 1e-4 && (n[1] - 1.0).abs() < 1e-4 && (n[2]).abs() < 1e-4);
        let p = gbuf.position[c];
        assert!((p[1] - 0.5).abs() < 1e-4);
        assert!(p[0].abs() < 0.02 && p[2].abs() < 0.02);
        // Camera at distance 1.45 from origin; face plane at y=0.5 -> view z = -0.95;
        // ortho(near 0, far 2): z_ndc = -z_view - 1 = -0.05.
        assert!((gbuf.depth[c] + 0.05).abs() < 1e-3, "depth {}", gbuf.depth[c]);
    }

    #[test]
    fn nearer_triangle_wins_depth_test() {
        let mut mesh = TriMesh::default();
        // Far quad (y = -0.2), then near quad (y = 0.2), both facing +Y.
        for (base_y, _) in [(-0.2f32, 0), (0.2f32, 1)] {
            let base = mesh.positions.len() as u32;
            mesh.positions.extend_from_slice(&[
                [-0.4, base_y, -0.4],
                [0.4, base_y, -0.4],
                [0.4, base_y, 0.4],
                [-0.4, base_y, 0.4],
            ]);
            mesh.normals.extend_from_slice(&[[0.0, 1.0, 0.0]; 4]);
            mesh.uvs.extend_from_slice(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
            mesh.indices.push([base, base + 1, base + 2]);
            mesh.indices.push([base, base + 2, base + 3]);
        }
        let mv = model_view_matrix(0.0, 0.0, CAMERA_DISTANCE, [0.0; 3]);
        let proj = default_orthographic(1.2);
        let gbuf = render_gbuffer(&mesh, &mv, &proj, 64, 64);
        let c = 32 * 64 + 32;
        assert!(gbuf.face_index[c] == 2 || gbuf.face_index[c] == 3, "face {}", gbuf.face_index[c]);
    }

    #[test]
    fn conditioning_maps_encode_expected_values() {
        let gbuf = front_gbuffer(64);
        let nmap = normal_map_rgb8(&gbuf, [0, 0, 0]);
        let pmap = position_map_rgb8(&gbuf, 1.0, [0, 0, 0]);
        let c = 32 * 64 + 32;
        // Normal (0,1,0) -> (128, 255, 128).
        assert_eq!(nmap[c * 3], 128);
        assert_eq!(nmap[c * 3 + 1], 255);
        assert_eq!(nmap[c * 3 + 2], 128);
        // Position (~0, 0.5, ~0) with scale 1 -> enc (0.5, 0.0, 0.5) -> (128, 0, 128).
        assert!((pmap[c * 3] as i32 - 128).abs() <= 3);
        assert_eq!(pmap[c * 3 + 1], 0);
        assert!((pmap[c * 3 + 2] as i32 - 128).abs() <= 3);
        // Background encodes the explicit background color.
        assert_eq!(nmap[0], 0);
    }

    #[test]
    fn deterministic_across_runs() {
        let a = front_gbuffer(96);
        let b = front_gbuffer(96);
        assert_eq!(a.face_index, b.face_index);
        assert_eq!(a.depth, b.depth);
    }

    #[test]
    fn wholly_negative_offscreen_bounds_do_not_wrap_or_panic() {
        let mut mesh = TriMesh::unit_quad();
        for position in &mut mesh.positions {
            position[0] -= 100.0;
            position[1] -= 100.0;
        }
        let result = std::panic::catch_unwind(|| {
            render_gbuffer(&mesh, &mat4_identity(), &mat4_identity(), 64, 64)
        });
        let gbuffer = result.expect("offscreen bounds must be clamped before integer conversion");
        assert_eq!(gbuffer.coverage(), 0.0);
    }

    #[test]
    fn nonfinite_projected_vertices_are_skipped() {
        let mesh = TriMesh::unit_quad();
        let mut projection = mat4_identity();
        projection[0][0] = f32::INFINITY;
        let result = std::panic::catch_unwind(|| {
            render_gbuffer(&mesh, &mat4_identity(), &projection, 64, 64)
        });
        let gbuffer = result.expect("nonfinite screen vertices must be skipped");
        assert_eq!(gbuffer.coverage(), 0.0);
    }
}
