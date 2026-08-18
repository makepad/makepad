//! Flexible-dual-grid mesh extraction (o_voxel convert/flexible_dual_grid.py,
//! inference path, op for op): one mesh vertex per active voxel at
//! (coord + dual_vertex) * voxel_size + aabb_min; for every voxel edge flagged
//! intersected (3 axis flags per voxel) the 4 edge-adjacent voxels form a
//! quad, split into 2 triangles by the split-weight products
//! (w0*w2 > w1*w3 -> [0,1,2]+[0,2,3], else [0,1,3]+[3,1,2]).
//!
//! Decoder head activations (shape decoder, 7 channels):
//!   dual_vertex = 2*sigmoid(f[0..3]) - 0.5   (voxel_margin 0.5)
//!   intersected = f[3..6] > 0
//!   split_weight = softplus(f[6])

use crate::trellis_slat::CoordMap;
use crate::{DiffusionError, Result};

/// Per-axis quad neighbor offsets (x, y, z edge orientation).
const EDGE_NEIGHBOR_OFFSETS: [[[i32; 3]; 4]; 3] = [
    [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]], // x-axis edge
    [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]], // y-axis edge
    [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]], // z-axis edge
];

pub struct T2MeshRaw {
    /// One vertex per voxel, voxel order (unreferenced voxels included, like
    /// the reference).
    pub vertices: Vec<[f32; 3]>,
    pub faces: Vec<[u32; 3]>,
}

pub struct T2FdgFields {
    pub dual_vertices: Vec<[f32; 3]>,
    pub intersected: Vec<[bool; 3]>,
    pub split_weight: Vec<f32>,
}

/// Decoder feats (N, 7) -> activated dual-grid fields.
pub fn t2_fdg_fields(feats: &[f32]) -> Result<T2FdgFields> {
    if feats.len() % 7 != 0 {
        return Err(DiffusionError::workflow("fdg feats must be (N, 7)"));
    }
    let n = feats.len() / 7;
    let sigmoid = |x: f32| 1.0 / (1.0 + (-x).exp());
    let softplus = |x: f32| {
        // torch F.softplus default beta 1, threshold 20: x > 20 -> x.
        if x > 20.0 {
            x
        } else {
            (1.0 + x.exp()).ln()
        }
    };
    // exp/ln over N x 7 at 5M+ voxels: chunked across threads, chunk outputs
    // concatenated in order.
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8)
        .clamp(1, 16);
    let chunk = n.div_ceil(threads).max(1);
    let parts: Vec<(Vec<[f32; 3]>, Vec<[bool; 3]>, Vec<f32>)> = std::thread::scope(|scope| {
        feats
            .chunks(chunk * 7)
            .map(|rows| {
                scope.spawn(move || {
                    let mut dual = Vec::with_capacity(rows.len() / 7);
                    let mut inter = Vec::with_capacity(rows.len() / 7);
                    let mut split = Vec::with_capacity(rows.len() / 7);
                    for row in rows.chunks_exact(7) {
                        dual.push([
                            2.0 * sigmoid(row[0]) - 0.5,
                            2.0 * sigmoid(row[1]) - 0.5,
                            2.0 * sigmoid(row[2]) - 0.5,
                        ]);
                        inter.push([row[3] > 0.0, row[4] > 0.0, row[5] > 0.0]);
                        split.push(softplus(row[6]));
                    }
                    (dual, inter, split)
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("fdg field thread"))
            .collect()
    });
    let mut dual_vertices = Vec::with_capacity(n);
    let mut intersected = Vec::with_capacity(n);
    let mut split_weight = Vec::with_capacity(n);
    for (dual, inter, split) in parts {
        dual_vertices.extend_from_slice(&dual);
        intersected.extend_from_slice(&inter);
        split_weight.extend_from_slice(&split);
    }
    Ok(T2FdgFields {
        dual_vertices,
        intersected,
        split_weight,
    })
}

/// The dual-grid extraction. `resolution` is the voxel grid size; the aabb is
/// the TRELLIS canonical [-0.5, 0.5]^3.
pub fn t2_dual_grid_to_mesh(
    coords: &[[i32; 3]],
    fields: &T2FdgFields,
    resolution: usize,
) -> Result<T2MeshRaw> {
    let n = coords.len();
    if fields.dual_vertices.len() != n || fields.intersected.len() != n {
        return Err(DiffusionError::workflow("fdg field length mismatch"));
    }
    let voxel_size = 1.0f32 / resolution as f32;
    let mut vertices = Vec::with_capacity(n);
    for (coord, dual) in coords.iter().zip(fields.dual_vertices.iter()) {
        vertices.push([
            (coord[0] as f32 + dual[0]) * voxel_size - 0.5,
            (coord[1] as f32 + dual[1]) * voxel_size - 0.5,
            (coord[2] as f32 + dual[2]) * voxel_size - 0.5,
        ]);
    }
    let map = CoordMap::build(coords);
    // Quads in (voxel, axis) row-major order, exactly the reference's
    // boolean-mask flattening. Contiguous voxel chunks across threads keep
    // that order after in-order concatenation.
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8)
        .clamp(1, 16);
    let chunk = n.div_ceil(threads).max(1);
    let face_parts: Vec<Vec<[u32; 3]>> = std::thread::scope(|scope| {
        (0..n)
            .step_by(chunk)
            .map(|start| {
                let map = &map;
                let end = (start + chunk).min(n);
                scope.spawn(move || {
                    let mut faces = Vec::new();
                    for i in start..end {
                        let coord = coords[i];
                        for axis in 0..3 {
                            if !fields.intersected[i][axis] {
                                continue;
                            }
                            let mut quad = [0u32; 4];
                            let mut valid = true;
                            for (slot, offset) in
                                EDGE_NEIGHBOR_OFFSETS[axis].iter().enumerate()
                            {
                                let index = map.get([
                                    coord[0] + offset[0],
                                    coord[1] + offset[1],
                                    coord[2] + offset[2],
                                ]);
                                if index == u32::MAX {
                                    valid = false;
                                    break;
                                }
                                quad[slot] = index;
                            }
                            if !valid {
                                continue;
                            }
                            let w = &fields.split_weight;
                            let w02 = w[quad[0] as usize] * w[quad[2] as usize];
                            let w13 = w[quad[1] as usize] * w[quad[3] as usize];
                            if w02 > w13 {
                                faces.push([quad[0], quad[1], quad[2]]);
                                faces.push([quad[0], quad[2], quad[3]]);
                            } else {
                                faces.push([quad[0], quad[1], quad[3]]);
                                faces.push([quad[3], quad[1], quad[2]]);
                            }
                        }
                    }
                    faces
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().expect("dual grid face thread"))
            .collect()
    });
    let mut faces = Vec::with_capacity(face_parts.iter().map(Vec::len).sum());
    for part in face_parts {
        faces.extend_from_slice(&part);
    }
    Ok(T2MeshRaw { vertices, faces })
}

/// TRELLIS space is Z-up; glTF is Y-up. The reference converts at export
/// ((x, y, z) -> (x, z, -y), a proper rotation — winding preserved); every
/// GLB we hand to consumers must do the same or the model lies pitched
/// forward. Keep the pipeline in TRELLIS space internally and rotate once,
/// at GLB write.
#[inline]
pub fn t2_yup(p: [f32; 3]) -> [f32; 3] {
    [p[0], p[2], -p[1]]
}

/// Compact to referenced vertices and serialize as an untextured GLB
/// (rotated to glTF Y-up).
pub fn t2_mesh_to_glb(mesh: &T2MeshRaw) -> Vec<u8> {
    t2_mesh_to_glb_colored(mesh, None)
}

/// [`t2_mesh_to_glb`] with optional per-VOXEL linear RGB (row i pairs with
/// mesh vertex i — the dual grid emits exactly one vertex per voxel), carried
/// through the compaction into COLOR_0.
pub fn t2_mesh_to_glb_colored(mesh: &T2MeshRaw, voxel_rgb: Option<&[[f32; 3]]>) -> Vec<u8> {
    let mut remap = vec![u32::MAX; mesh.vertices.len()];
    let mut positions = Vec::new();
    let mut colors = Vec::new();
    let mut indices = Vec::with_capacity(mesh.faces.len() * 3);
    for face in &mesh.faces {
        for &v in face {
            if remap[v as usize] == u32::MAX {
                remap[v as usize] = positions.len() as u32;
                positions.push(t2_yup(mesh.vertices[v as usize]));
                if let Some(rgb) = voxel_rgb {
                    colors.push(rgb[v as usize]);
                }
            }
            indices.push(remap[v as usize]);
        }
    }
    match voxel_rgb {
        Some(_) => makepad_gltf::write_glb_mesh_colored(&positions, &indices, Some(&colors)),
        None => makepad_gltf::write_glb_mesh(&positions, &indices),
    }
}

/// Weight-normalized trilinear point sampler over a sparse per-voxel
/// attribute volume in TRELLIS space (the reference bake's grid_sample_3d,
/// with two deliberate differences: the weights renormalize over ACTIVE
/// voxels so sparse boundaries don't darken, and positions whose whole
/// 8-neighborhood is empty fall back to the nearest active voxel in an
/// expanding shell search — bounded by `radius`).
///
/// `feats` is (N, channels) matching `coords`; voxel i covers
/// [i/res - 0.5, (i+1)/res - 0.5), center (i + 0.5)/res - 0.5.
pub struct T2VoxelSampler<'a> {
    map: CoordMap,
    feats: &'a [f32],
    channels: usize,
    res: f32,
}

impl<'a> T2VoxelSampler<'a> {
    pub fn new(
        coords: &[[i32; 3]],
        feats: &'a [f32],
        channels: usize,
        resolution: usize,
    ) -> Result<Self> {
        if coords.len() * channels != feats.len() {
            return Err(DiffusionError::workflow("voxel attr shape mismatch"));
        }
        Ok(Self {
            map: CoordMap::build(coords),
            feats,
            channels,
            res: resolution as f32,
        })
    }

    /// Sample into `out` (len == channels). Returns false when nothing was
    /// found within `radius` shells (out untouched).
    pub fn sample_into(&self, p: [f32; 3], radius: i32, out: &mut [f32]) -> bool {
        let channels = self.channels;
        // Voxel-center space: g = (p + 0.5) * res - 0.5 puts voxel i's
        // center at g = i.
        let g = [
            (p[0] + 0.5) * self.res - 0.5,
            (p[1] + 0.5) * self.res - 0.5,
            (p[2] + 0.5) * self.res - 0.5,
        ];
        let base = [
            g[0].floor() as i32,
            g[1].floor() as i32,
            g[2].floor() as i32,
        ];
        let frac = [
            g[0] - base[0] as f32,
            g[1] - base[1] as f32,
            g[2] - base[2] as f32,
        ];
        for value in out.iter_mut() {
            *value = 0.0;
        }
        let mut total = 0.0f32;
        for corner in 0..8 {
            let (dx, dy, dz) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let row = self.map.get([
                base[0] + dx as i32,
                base[1] + dy as i32,
                base[2] + dz as i32,
            ]);
            if row == u32::MAX {
                continue;
            }
            let w = (if dx == 1 { frac[0] } else { 1.0 - frac[0] })
                * (if dy == 1 { frac[1] } else { 1.0 - frac[1] })
                * (if dz == 1 { frac[2] } else { 1.0 - frac[2] });
            if w <= 0.0 {
                continue;
            }
            total += w;
            let src = &self.feats[row as usize * channels..(row as usize + 1) * channels];
            for (dst, value) in out.iter_mut().zip(src) {
                *dst += w * value;
            }
        }
        if total > 1e-8 {
            for value in out.iter_mut() {
                *value /= total;
            }
            return true;
        }
        // Empty 8-neighborhood: expanding shell search for the nearest
        // active voxel (surface points sit within a voxel or two of the
        // shell, so this triggers rarely and terminates fast).
        let center = [g[0].round() as i32, g[1].round() as i32, g[2].round() as i32];
        for r in 1..=radius {
            let mut best = u32::MAX;
            let mut best_d2 = f32::MAX;
            for dx in -r..=r {
                for dy in -r..=r {
                    for dz in -r..=r {
                        if dx.abs().max(dy.abs()).max(dz.abs()) != r {
                            continue;
                        }
                        let c = [center[0] + dx, center[1] + dy, center[2] + dz];
                        let row = self.map.get(c);
                        if row == u32::MAX {
                            continue;
                        }
                        let d = [
                            c[0] as f32 - g[0],
                            c[1] as f32 - g[1],
                            c[2] as f32 - g[2],
                        ];
                        let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                        if d2 < best_d2 {
                            best_d2 = d2;
                            best = row;
                        }
                    }
                }
            }
            if best != u32::MAX {
                let src =
                    &self.feats[best as usize * channels..(best as usize + 1) * channels];
                out.copy_from_slice(src);
                return true;
            }
        }
        false
    }
}

/// Batch [`T2VoxelSampler`] over many positions (threaded); positions whose
/// bounded search finds nothing get 0.5 in every channel (neutral gray, only
/// reachable far off the shell). Returns (positions.len(), channels).
pub fn t2_sample_voxel_attrs(
    coords: &[[i32; 3]],
    feats: &[f32],
    channels: usize,
    resolution: usize,
    positions: &[[f32; 3]],
) -> Result<Vec<f32>> {
    let sampler = T2VoxelSampler::new(coords, feats, channels, resolution)?;
    let n = positions.len();
    let mut out = vec![0.0f32; n * channels];
    let threads = std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8)
        .clamp(1, 16);
    let chunk = n.div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for (rows, values) in positions.chunks(chunk).zip(out.chunks_mut(chunk * channels)) {
            let sampler = &sampler;
            scope.spawn(move || {
                for (p, dst) in rows.iter().zip(values.chunks_exact_mut(channels)) {
                    if !sampler.sample_into(*p, 8, dst) {
                        for value in dst.iter_mut() {
                            *value = 0.5;
                        }
                    }
                }
            });
        }
    });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_split_and_vertex_math() {
        // A 2x2 plate of voxels in the y/z plane sharing an x-axis edge at
        // (0,0,0): coords (0,0,0), (0,0,1), (0,1,1), (0,1,0).
        let coords = vec![[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]];
        let fields = T2FdgFields {
            dual_vertices: vec![[0.5; 3]; 4],
            intersected: vec![[true, false, false], [false; 3], [false; 3], [false; 3]],
            split_weight: vec![1.0, 2.0, 1.0, 2.0],
        };
        let mesh = t2_dual_grid_to_mesh(&coords, &fields, 4).unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        // (0 + 0.5) * 0.25 - 0.5 = -0.375
        assert!((mesh.vertices[0][0] + 0.375).abs() < 1e-6);
        // w02 = 1, w13 = 4 -> split 2: [0,1,3], [3,1,2]
        assert_eq!(mesh.faces, vec![[0, 1, 3], [3, 1, 2]]);
        // Missing neighbor -> no faces.
        let fields2 = T2FdgFields {
            dual_vertices: vec![[0.5; 3]; 3],
            intersected: vec![[true, false, false], [false; 3], [false; 3]],
            split_weight: vec![1.0; 3],
        };
        let mesh2 = t2_dual_grid_to_mesh(&coords[..3], &fields2, 4).unwrap();
        assert!(mesh2.faces.is_empty());
    }

    #[test]
    fn glb_compacts_vertices() {
        let mesh = T2MeshRaw {
            vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [9.0; 3]],
            faces: vec![[0, 1, 2]],
        };
        let glb = t2_mesh_to_glb(&mesh);
        assert_eq!(&glb[0..4], b"glTF");
    }

    #[test]
    fn yup_rotation_roundtrip() {
        let p = [0.25f32, -0.5, 0.75];
        let r = t2_yup(p);
        assert_eq!(r, [0.25, 0.75, 0.5]);
        // Inverse used by the color resampler: (X, Y, Z) -> (X, -Z, Y).
        assert_eq!([r[0], -r[2], r[1]], p);
    }

    #[test]
    fn voxel_attr_sampling() {
        // Two voxels along +x at res 4: centers x = (i + 0.5)/4 - 0.5.
        let coords = vec![[1, 2, 2], [2, 2, 2]];
        let feats = vec![0.0f32, 1.0]; // 1 channel each
        // At voxel 0's center: exactly feats[0].
        let c0 = [(1.0f32 + 0.5) / 4.0 - 0.5, (2.0 + 0.5) / 4.0 - 0.5, (2.0 + 0.5) / 4.0 - 0.5];
        // Halfway between the two centers: weight-normalized mean = 0.5
        // (the two inactive y/z corners renormalize away).
        let mid = [c0[0] + 0.125, c0[1], c0[2]];
        // Far away from everything: nearest-voxel fallback.
        let far = [c0[0], c0[1] + 0.9, c0[2]];
        let out = t2_sample_voxel_attrs(&coords, &feats, 1, 4, &[c0, mid, far]).unwrap();
        assert!((out[0] - 0.0).abs() < 1e-6, "center sample {}", out[0]);
        assert!((out[1] - 0.5).abs() < 1e-6, "mid sample {}", out[1]);
        assert!((out[2] - 0.0).abs() < 1e-6, "fallback sample {}", out[2]);
    }
}
