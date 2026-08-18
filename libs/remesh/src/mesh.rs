//! GLB loading + the reference demo's normalization pipeline.
//!
//! Replicates `trimesh.load(path, force='mesh')` + demo.py `normalize_mesh`
//! + `nondegenerate_faces` exactly (all in float64, cast to f32 at the end):
//! - scene-graph traversal is trimesh's LIFO stack (children pushed in list
//!   order, popped from the back -> visited in REVERSE order)
//! - node transform = M @ T @ R @ S (float64; GLTF column-major matrix,
//!   XYZW quaternion -> trimesh quaternion_matrix, diag scale)
//! - transform_points skips matrices within 1e-8 of identity (no fp ops)
//! - normalize: bbox-center, scale (1-margin)/max|v| (only if max|v| > 1e-8)
//! - degenerate filter: 2D OBB extents (2*area/edge_len) must BOTH exceed
//!   1e-8 (trimesh tol.merge); then remove unreferenced vertices

use makepad_gltf::{decode_mesh_primitive, load_gltf_from_bytes, GltfNode, LoadedGltf};

use crate::math::V3;

pub struct Mesh {
    pub positions: Vec<V3>,
    pub faces: Vec<[u32; 3]>,
}

impl Mesh {
    pub fn tri_soup(&self) -> Vec<[V3; 3]> {
        self.faces
            .iter()
            .map(|f| {
                [
                    self.positions[f[0] as usize],
                    self.positions[f[1] as usize],
                    self.positions[f[2] as usize],
                ]
            })
            .collect()
    }
}

/// Inverse data of the normalization: p_original = p_normalized / scale + center.
#[derive(Clone, Copy)]
pub struct NormalizeInfo {
    pub center: [f64; 3],
    pub scale: f64,
}

type M4 = [[f64; 4]; 4]; // row-major

const IDENTITY: M4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn mat_mul(a: &M4, b: &M4) -> M4 {
    let mut out = [[0.0f64; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[r][k] * b[k][c];
            }
            out[r][c] = s;
        }
    }
    out
}

fn is_near_identity(m: &M4) -> bool {
    // trimesh transform_points: max |M - I| < 1e-8 -> skip transform entirely
    let mut mx = 0.0f64;
    for r in 0..4 {
        for c in 0..4 {
            mx = mx.max((m[r][c] - IDENTITY[r][c]).abs());
        }
    }
    mx < 1e-8
}

/// trimesh transformations.quaternion_matrix on a WXYZ quaternion.
fn quaternion_matrix(w: f64, x: f64, y: f64, z: f64) -> M4 {
    let eps = f64::EPSILON * 4.0;
    let mut q = [w, x, y, z];
    let n = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if n < eps {
        return IDENTITY;
    }
    let s = (2.0 / n).sqrt();
    for v in q.iter_mut() {
        *v *= s;
    }
    let o = |i: usize, j: usize| q[i] * q[j];
    [
        [1.0 - o(2, 2) - o(3, 3), o(1, 2) - o(3, 0), o(1, 3) + o(2, 0), 0.0],
        [o(1, 2) + o(3, 0), 1.0 - o(1, 1) - o(3, 3), o(2, 3) - o(1, 0), 0.0],
        [o(1, 3) - o(2, 0), o(2, 3) + o(1, 0), 1.0 - o(1, 1) - o(2, 2), 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// node local transform = M @ T @ R @ S (trimesh gltf loader)
fn node_local_matrix(node: &GltfNode) -> M4 {
    let mut m = IDENTITY;
    if let Some(g) = &node.matrix {
        // GLTF matrices are column-major; trimesh does reshape(4,4).T
        for r in 0..4 {
            for c in 0..4 {
                m[r][c] = g[c * 4 + r] as f64;
            }
        }
    }
    if let Some(t) = &node.translation {
        let mut tm = IDENTITY;
        tm[0][3] = t[0] as f64;
        tm[1][3] = t[1] as f64;
        tm[2][3] = t[2] as f64;
        m = mat_mul(&m, &tm);
    }
    if let Some(r) = &node.rotation {
        // GLTF XYZW -> WXYZ
        let rm = quaternion_matrix(r[3] as f64, r[0] as f64, r[1] as f64, r[2] as f64);
        m = mat_mul(&m, &rm);
    }
    if let Some(s) = &node.scale {
        let mut sm = IDENTITY;
        sm[0][0] = s[0] as f64;
        sm[1][1] = s[1] as f64;
        sm[2][2] = s[2] as f64;
        m = mat_mul(&m, &sm);
    }
    m
}

fn collect_scene_mesh(loaded: &LoadedGltf) -> Result<(Vec<[f64; 3]>, Vec<[u32; 3]>), String> {
    let doc = &loaded.document;
    let nodes = doc.nodes_slice();
    let scenes = doc.scenes_slice();
    let scene_index = doc.scene.unwrap_or(0);
    let roots: Vec<usize> = scenes
        .get(scene_index)
        .and_then(|s| s.nodes.clone())
        .unwrap_or_default();

    // trimesh LIFO traversal: push roots in order, pop from the back
    let mut stack: Vec<(usize, M4)> = roots.iter().map(|&r| (r, IDENTITY)).collect();
    let mut visited = vec![false; nodes.len()];
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut faces: Vec<[u32; 3]> = Vec::new();

    while let Some((ni, parent_world)) = stack.pop() {
        if visited[ni] {
            continue;
        }
        visited[ni] = true;
        let node = &nodes[ni];
        let world = mat_mul(&parent_world, &node_local_matrix(node));
        if let Some(children) = &node.children {
            for &c in children {
                stack.push((c, world));
            }
        }
        if let Some(mesh_idx) = node.mesh {
            let mesh = &doc.meshes_slice()[mesh_idx];
            for prim_idx in 0..mesh.primitives.len() {
                let prim = decode_mesh_primitive(loaded, mesh_idx, prim_idx)
                    .map_err(|e| format!("primitive decode failed: {e:?}"))?;
                let base = positions.len() as u32;
                let skip = is_near_identity(&world);
                for p in &prim.positions {
                    let v = [p[0] as f64, p[1] as f64, p[2] as f64];
                    if skip {
                        positions.push(v);
                    } else {
                        positions.push([
                            world[0][0] * v[0] + world[0][1] * v[1] + world[0][2] * v[2] + world[0][3],
                            world[1][0] * v[0] + world[1][1] * v[1] + world[1][2] * v[2] + world[1][3],
                            world[2][0] * v[0] + world[2][1] * v[1] + world[2][2] * v[2] + world[2][3],
                        ]);
                    }
                }
                for f in prim.indices.chunks_exact(3) {
                    faces.push([base + f[0], base + f[1], base + f[2]]);
                }
            }
        }
    }
    if faces.is_empty() {
        return Err("GLB contains no triangles".into());
    }
    Ok((positions, faces))
}

/// Load a GLB and run the reference demo preprocessing at the given margin
/// (demo default 0.05). Returns the normalized f32 mesh + inverse transform.
pub fn load_glb_normalized(bytes: &[u8], margin: f64) -> Result<(Mesh, NormalizeInfo), String> {
    let loaded = load_gltf_from_bytes(bytes, None).map_err(|e| format!("GLB parse failed: {e:?}"))?;
    let (mut positions, faces) = collect_scene_mesh(&loaded)?;

    // normalize_mesh (float64): bbox-center then scale to (1-margin)
    let mut mn = [f64::INFINITY; 3];
    let mut mx = [f64::NEG_INFINITY; 3];
    for p in &positions {
        for d in 0..3 {
            mn[d] = mn[d].min(p[d]);
            mx[d] = mx[d].max(p[d]);
        }
    }
    let center = [
        (mn[0] + mx[0]) / 2.0,
        (mn[1] + mx[1]) / 2.0,
        (mn[2] + mx[2]) / 2.0,
    ];
    for p in positions.iter_mut() {
        for d in 0..3 {
            p[d] -= center[d];
        }
    }
    let mut half = 0.0f64;
    for p in &positions {
        for d in 0..3 {
            half = half.max(p[d].abs());
        }
    }
    let mut scale = 1.0f64;
    if half > 1e-8 {
        scale = (1.0 - margin) / half;
        for p in positions.iter_mut() {
            for d in 0..3 {
                p[d] *= scale;
            }
        }
    }

    // nondegenerate_faces (float64, trimesh tol.merge = 1e-8)
    let keep: Vec<bool> = faces
        .iter()
        .map(|f| {
            let v0 = positions[f[0] as usize];
            let v1 = positions[f[1] as usize];
            let v2 = positions[f[2] as usize];
            let a = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
            let b = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
            let cr = [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ];
            let area = (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt() / 2.0;
            let la = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
            let lb = (b[0] * b[0] + b[1] * b[1] + b[2] * b[2]).sqrt();
            let e0 = if la > 1e-8 { area * 2.0 / la } else { 0.0 };
            let e1 = if lb > 1e-8 { area * 2.0 / lb } else { 0.0 };
            e0 > 1e-8 && e1 > 1e-8
        })
        .collect();

    let (positions, faces) = if keep.iter().all(|&k| k) {
        (positions, faces)
    } else {
        // update_faces + remove_unreferenced_vertices (order-preserving)
        let kept: Vec<[u32; 3]> = faces
            .iter()
            .zip(&keep)
            .filter(|(_, &k)| k)
            .map(|(f, _)| *f)
            .collect();
        let mut used = vec![false; positions.len()];
        for f in &kept {
            for &v in f {
                used[v as usize] = true;
            }
        }
        let mut remap = vec![u32::MAX; positions.len()];
        let mut newpos = Vec::new();
        for (i, &u) in used.iter().enumerate() {
            if u {
                remap[i] = newpos.len() as u32;
                newpos.push(positions[i]);
            }
        }
        let faces2: Vec<[u32; 3]> = kept
            .iter()
            .map(|f| [remap[f[0] as usize], remap[f[1] as usize], remap[f[2] as usize]])
            .collect();
        (newpos, faces2)
    };

    let mesh = Mesh {
        positions: positions
            .iter()
            .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
            .collect(),
        faces,
    };
    Ok((mesh, NormalizeInfo { center, scale }))
}
