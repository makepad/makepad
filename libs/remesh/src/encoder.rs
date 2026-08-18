//! FCT encoder — exact port of faithcontour/encoder.py on the validated
//! CPU-oracle semantics (see local/faithc_ref/cpu_polyfill.py).
//!
//! Structure differs from the reference (fused octree recursion instead of a
//! level-walk over cube lists + BVH), but produces the IDENTICAL result:
//! - subtree triangle lists are narrowed with SAT(NARROW_EPS=1e-4), a provable
//!   fp-superset of every SAT(1e-6) hit in the subtree (interval nesting with
//!   >25x fp headroom), so no candidate is ever lost;
//! - cube activity at levels [min_level, max_level) gates on SAT(SAT_EPS=1e-6)
//!   exactly like the reference broadphase;
//! - final-level pairs = SAT(1e-6) hits, clipped; voxel active iff >=1 clip hit;
//! - DFS emission in CUBE_CORNERS child order = the reference's traversal
//!   order (BFS with contiguous-children expansion + order-preserving filters
//!   yields the same leaf sequence), so token order matches the oracle dumps;
//! - per-voxel samples are visited in ascending triangle id, matching the
//!   oracle's fp32 accumulation order in the QEF.

use std::sync::Arc;

use crate::grid::{Grid, CUBE_CORNERS};
use crate::math::*;
use crate::qef::{solve_qef_voxel, QefSample};
use crate::sat::{clip_tri_box, tri_box_sat, NARROW_EPS, SAT_EPS};
use crate::spatial::{
    closest_tri, segment_nearest_hit, tri_barycentric, tri_closest_point, BinGrid,
};
use crate::parallel::{par_ranges, par_sort_by_key};
use makepad_csg_math::thread_pool;

#[derive(Clone, Copy)]
pub struct EncodeOptions {
    pub lambda_n: f32,
    pub lambda_d: f32,
    pub clamp_anchors: bool,
    pub compute_flux: bool,
    /// Octree start level; default = min(4, max(1, max_level - 1)) (demo config).
    pub min_level: Option<u32>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions {
            lambda_n: 1.0,
            lambda_d: 1e-3,
            clamp_anchors: true,
            compute_flux: true,
            min_level: None,
        }
    }
}

/// FCT tokens: 18 dims per active voxel (anchor 3 + normal 3 + flux 12).
pub struct FctTokens {
    pub resolution: u32,
    /// linear voxel indices (i*r^2 + j*r + k), in reference traversal order
    pub voxel_indices: Vec<i64>,
    pub anchors: Vec<V3>,
    pub normals: Vec<V3>,
    pub flux: Vec<[i8; 12]>,
}

struct SharedEnc {
    grid: Grid,
    tris: Vec<[V3; 3]>,
    face_normals: Vec<V3>,
    min_level: u32,
    max_level: u32,
}

struct TaskOut {
    /// (linear voxel id, sample count); samples appended in voxel order
    voxels: Vec<(i64, u32)>,
    samples: Vec<QefSample>,
}

fn recurse(
    sh: &SharedEnc,
    level: u32,
    ijk: [i64; 3],
    ids: &[u32],
    scratch: &mut Vec<Vec<u32>>,
    out: &mut TaskOut,
) {
    let clevel = level + 1;
    for corner in CUBE_CORNERS {
        let child = [
            ijk[0] * 2 + corner[0],
            ijk[1] * 2 + corner[1],
            ijk[2] * 2 + corner[2],
        ];
        let (mn, mx) = Grid::cube_aabb_level(child, clevel);
        if clevel == sh.max_level {
            // final level: pair set = SAT(1e-6) hits, then exact clip
            let start = out.samples.len();
            for &t in ids {
                let tri = &sh.tris[t as usize];
                if tri_box_sat(tri, mn, mx, SAT_EPS) {
                    let clip = clip_tri_box(tri, mn, mx);
                    if clip.hit {
                        out.samples.push(QefSample {
                            point: clip.centroid,
                            normal: sh.face_normals[t as usize],
                            area: clip.area,
                        });
                    }
                }
            }
            let count = out.samples.len() - start;
            if count > 0 {
                out.voxels.push((sh.grid.linear(child), count as u32));
            }
        } else {
            // narrow with the superset eps; gate activity on the exact eps
            let mut narrowed = std::mem::take(&mut scratch[clevel as usize]);
            narrowed.clear();
            let gated = clevel >= sh.min_level;
            let mut active = !gated;
            for &t in ids {
                let tri = &sh.tris[t as usize];
                if tri_box_sat(tri, mn, mx, NARROW_EPS) {
                    narrowed.push(t);
                    if !active && tri_box_sat(tri, mn, mx, SAT_EPS) {
                        active = true;
                    }
                }
            }
            if active && !narrowed.is_empty() {
                recurse(sh, clevel, child, &narrowed, scratch, out);
            }
            scratch[clevel as usize] = narrowed;
        }
    }
}

/// Encoder-flavor face normals: cross(v1-v0, v2-v0) / clamp_min(norm, 1e-8).
fn encoder_face_normals(tris: &[[V3; 3]]) -> Vec<V3> {
    tris.iter()
        .map(|t| {
            let n = cross3(sub3(t[1], t[0]), sub3(t[2], t[0]));
            let l = norm3(n).max(1e-8);
            [n[0] / l, n[1] / l, n[2] / l]
        })
        .collect()
}

/// Segment-ops-flavor face normal: cross(v1-v0, v2-v0) / (norm + 1e-8).
#[inline]
fn seg_face_normal(tri: &[V3; 3]) -> V3 {
    let n = cross3(sub3(tri[1], tri[0]), sub3(tri[2], tri[0]));
    let l = norm3(n) + 1e-8;
    [n[0] / l, n[1] / l, n[2] / l]
}

/// The reference `_clamp_and_project_anchors` stage: clamp each anchor to its
/// voxel AABB; if it moved (>1e-8), project it to the closest surface point
/// with barycentrics clamped to [1e-4, 1-1e-4]; finally re-clamp to the voxel.
/// Public so the stage can be parity-tested against the oracle in isolation.
pub fn clamp_and_project_anchors(
    resolution: u32,
    tris: &[[V3; 3]],
    bin_grid: &Arc<BinGrid>,
    voxel_indices: &Arc<Vec<i64>>,
    anchors: &[V3],
) -> Vec<V3> {
    let grid = Grid::new(resolution);
    let k = anchors.len();
    let tris_arc: Arc<Vec<[V3; 3]>> = Arc::new(tris.to_vec());
    let anchors_in: Arc<Vec<V3>> = Arc::new(anchors.to_vec());
    let chunks = {
        let bg = bin_grid.clone();
        let voxel_indices = voxel_indices.clone();
        let tris = tris_arc;
        let anchors_in = anchors_in;
        par_ranges(k, 8192, move |s, e| {
            let uvw_lo = 1e-4f32;
            let uvw_hi = (1.0f64 - 1e-4) as f32;
            let mut out = Vec::with_capacity(e - s);
            for i in s..e {
                let ijk = grid.ijk_of(voxel_indices[i]);
                let (mn, mx) = grid.cube_aabb(ijk);
                let a = anchors_in[i];
                let clamped = clamp3(a, mn, mx);
                let moved = norm3(sub3(clamped, a)) > 1e-8;
                let mut refined = clamped;
                if moved {
                    let (_, tid) = closest_tri(&bg, &tris, clamped);
                    let tri = &tris[tid as usize];
                    let cp = tri_closest_point(clamped, tri);
                    let uvw = tri_barycentric(cp, tri);
                    let uc = [
                        uvw[0].max(uvw_lo).min(uvw_hi),
                        uvw[1].max(uvw_lo).min(uvw_hi),
                        uvw[2].max(uvw_lo).min(uvw_hi),
                    ];
                    let sum = uc[0] + uc[1] + uc[2];
                    let un = [uc[0] / sum, uc[1] / sum, uc[2] / sum];
                    refined = [
                        tri[0][0] * un[0] + tri[1][0] * un[1] + tri[2][0] * un[2],
                        tri[0][1] * un[0] + tri[1][1] * un[1] + tri[2][1] * un[2],
                        tri[0][2] * un[0] + tri[1][2] * un[1] + tri[2][2] * un[2],
                    ];
                }
                // final re-clamp relative to voxel center
                let center = [
                    (mn[0] + mx[0]) / 2.0,
                    (mn[1] + mx[1]) / 2.0,
                    (mn[2] + mx[2]) / 2.0,
                ];
                let half = [
                    (mx[0] - mn[0]) / 2.0,
                    (mx[1] - mn[1]) / 2.0,
                    (mx[2] - mn[2]) / 2.0,
                ];
                let rel = sub3(refined, center);
                let relc = clamp3(rel, [-half[0], -half[1], -half[2]], half);
                out.push(add3(relc, center));
            }
            out
        })
    };
    let mut out = Vec::with_capacity(k);
    for c in chunks {
        out.extend_from_slice(&c);
    }
    out
}

pub fn encode(tris: &[[V3; 3]], resolution: u32, opts: &EncodeOptions) -> FctTokens {
    assert!(resolution >= 4, "resolution must be >= 4");
    let timings = std::env::var_os("REMESH_TIMINGS").is_some();
    let mut t = std::time::Instant::now();
    let stage = |name: &str, t: &mut std::time::Instant| {
        if timings {
            eprintln!("  encode/{name}: {:.3}s", t.elapsed().as_secs_f64());
        }
        *t = std::time::Instant::now();
    };
    let grid = Grid::new(resolution);
    let max_level = grid.max_level;
    let min_level = opts
        .min_level
        .unwrap_or_else(|| 4.min(1.max(max_level.saturating_sub(1))));

    let sh = Arc::new(SharedEnc {
        grid,
        tris: tris.to_vec(),
        face_normals: encoder_face_normals(tris),
        min_level,
        max_level,
    });

    // Serial narrowing expansion down to min_level. The reference seeds its
    // level walk with ALL min_level cubes in RASTER order (meshgrid i,j,k
    // ascending), so the min_level node list must be raster-sorted; from
    // there BFS-with-contiguous-children == our DFS in CUBE_CORNERS order.
    struct Node {
        level: u32,
        ijk: [i64; 3],
        ids: Vec<u32>,
    }
    let mut nodes = vec![Node {
        level: 0,
        ijk: [0, 0, 0],
        ids: (0..tris.len() as u32).collect(),
    }];
    for level in 0..min_level {
        let mut next = Vec::new();
        for node in &nodes {
            for corner in CUBE_CORNERS {
                let child = [
                    node.ijk[0] * 2 + corner[0],
                    node.ijk[1] * 2 + corner[1],
                    node.ijk[2] * 2 + corner[2],
                ];
                let (mn, mx) = Grid::cube_aabb_level(child, level + 1);
                let ids: Vec<u32> = node
                    .ids
                    .iter()
                    .copied()
                    .filter(|&t| tri_box_sat(&tris[t as usize], mn, mx, NARROW_EPS))
                    .collect();
                if !ids.is_empty() {
                    next.push(Node {
                        level: level + 1,
                        ijk: child,
                        ids,
                    });
                }
            }
        }
        nodes = next;
    }
    // raster order at min_level, then the reference's activity gate (SAT 1e-6)
    let ml_res = 1i64 << min_level;
    nodes.sort_by_key(|n| (n.ijk[0] * ml_res + n.ijk[1]) * ml_res + n.ijk[2]);
    nodes.retain(|n| {
        let (mn, mx) = Grid::cube_aabb_level(n.ijk, min_level);
        n.ids
            .iter()
            .any(|&t| tri_box_sat(&tris[t as usize], mn, mx, SAT_EPS))
    });

    // parallel recursion over the min_level nodes (result order preserved)
    let tasks: Vec<Box<dyn FnOnce() -> TaskOut + Send>> = nodes
        .into_iter()
        .map(|node| {
            let sh = sh.clone();
            Box::new(move || {
                let mut out = TaskOut {
                    voxels: Vec::new(),
                    samples: Vec::new(),
                };
                let mut scratch: Vec<Vec<u32>> = vec![Vec::new(); sh.max_level as usize + 1];
                recurse(&sh, node.level, node.ijk, &node.ids, &mut scratch, &mut out);
                out
            }) as Box<dyn FnOnce() -> TaskOut + Send>
        })
        .collect();
    let results = thread_pool::parallel_for(tasks);
    stage("octree+sat+clip", &mut t);

    // flatten (traversal order)
    let mut voxel_indices: Vec<i64> = Vec::new();
    let mut sample_ranges: Vec<(u32, u32)> = Vec::new();
    let mut samples: Vec<QefSample> = Vec::new();
    for r in results {
        let mut s = samples.len() as u32;
        for (lin, count) in r.voxels {
            voxel_indices.push(lin);
            sample_ranges.push((s, count));
            s += count;
        }
        samples.extend_from_slice(&r.samples);
    }
    let k = voxel_indices.len();
    if k == 0 {
        return FctTokens {
            resolution,
            voxel_indices,
            anchors: Vec::new(),
            normals: Vec::new(),
            flux: Vec::new(),
        };
    }
    let voxel_indices = Arc::new(voxel_indices);
    let samples = Arc::new(samples);
    let sample_ranges = Arc::new(sample_ranges);

    // QEF per voxel (accumulation order inside each voxel = ascending tri id)
    let cell = grid.cell;
    let (lambda_n, lambda_d) = (opts.lambda_n, opts.lambda_d);
    let qef_out = {
        let samples = samples.clone();
        let sample_ranges = sample_ranges.clone();
        par_ranges(k, 8192, move |s, e| {
            let mut anchors = Vec::with_capacity(e - s);
            let mut normals = Vec::with_capacity(e - s);
            for i in s..e {
                let (start, count) = sample_ranges[i];
                let r = solve_qef_voxel(
                    &samples[start as usize..(start + count) as usize],
                    cell,
                    lambda_n,
                    lambda_d,
                );
                anchors.push(r.anchor);
                normals.push(r.normal);
            }
            (anchors, normals)
        })
    };
    let mut anchors: Vec<V3> = Vec::with_capacity(k);
    let mut normals: Vec<V3> = Vec::with_capacity(k);
    for (a, n) in qef_out {
        anchors.extend_from_slice(&a);
        normals.extend_from_slice(&n);
    }
    stage("qef", &mut t);

    // spatial bin grid for clamp-UDF + flux queries
    let need_grid = opts.clamp_anchors || opts.compute_flux;
    let bin_grid = if need_grid {
        Some(Arc::new(BinGrid::build(&sh.tris)))
    } else {
        None
    };
    stage("bin_grid", &mut t);

    // anchor clamp + surface reprojection (encoder._clamp_and_project_anchors)
    if opts.clamp_anchors {
        anchors = clamp_and_project_anchors(
            resolution,
            &sh.tris,
            bin_grid.as_ref().unwrap(),
            &voxel_indices,
            &anchors,
        );
    }
    stage("clamp", &mut t);

    // edge flux (encoder._compute_edge_flux + segment_ops)
    let flux = if opts.compute_flux {
        // unique edges over all active voxels
        let edge_chunks = {
            let sh = sh.clone();
            let voxel_indices = voxel_indices.clone();
            par_ranges(k, 32768, move |s, e| {
                let mut ids = Vec::with_capacity((e - s) * 12);
                for i in s..e {
                    ids.extend_from_slice(&sh.grid.cube_edge_ids(sh.grid.ijk_of(voxel_indices[i])));
                }
                ids
            })
        };
        let unique: Vec<i64> = par_sort_by_key(
            edge_chunks.into_iter().flatten().collect(),
            |&x| x as i128,
            true,
        );
        stage("flux/unique-edges", &mut t);
        let unique = Arc::new(unique);
        let n_e = unique.len();

        // torch preamble: dirs, lengths, global max_t over the batch
        let seg_chunks = {
            let sh = sh.clone();
            let unique = unique.clone();
            par_ranges(n_e, 65536, move |s, e| {
                let mut v = Vec::with_capacity(e - s);
                for &eid in &unique[s..e] {
                    let (e0, e1) = sh.grid.edge_endpoints(eid);
                    let d = sub3(e1, e0);
                    let len = norm3(d);
                    v.push((e0, e1, d, len));
                }
                v
            })
        };
        let seg_data: Vec<(V3, V3, V3, f32)> = seg_chunks.into_iter().flatten().collect();
        let max_t = seg_data
            .iter()
            .fold(f32::NEG_INFINITY, |m, &(_, _, _, l)| m.max(l));
        let seg_data = Arc::new(seg_data);

        let flux_chunks = {
            let sh = sh.clone();
            let bg = bin_grid.as_ref().unwrap().clone();
            let seg_data = seg_data.clone();
            par_ranges(n_e, 16384, move |s, e| {
                let mut out = Vec::with_capacity(e - s);
                for &(e0, e1, d, len) in &seg_data[s..e] {
                    let inv = len + 1e-8;
                    let rd = [d[0] / inv, d[1] / inv, d[2] / inv];
                    let (t, face) =
                        segment_nearest_hit(&bg, &sh.tris, e0, e1, e0, rd, max_t);
                    let f: i8 = if face >= 0 && t <= len {
                        let n = seg_face_normal(&sh.tris[face as usize]);
                        let dp = dot3(rd, n);
                        if dp > 0.0 {
                            1
                        } else if dp < 0.0 {
                            -1
                        } else {
                            0
                        }
                    } else {
                        0
                    };
                    out.push(f);
                }
                out
            })
        };
        let edge_flux: Vec<i8> = flux_chunks.into_iter().flatten().collect();
        stage("flux/segments", &mut t);
        let edge_flux = Arc::new(edge_flux);

        // map back to per-voxel 12 edges
        let per_voxel = {
            let sh = sh.clone();
            let voxel_indices = voxel_indices.clone();
            let unique = unique.clone();
            let edge_flux = edge_flux.clone();
            par_ranges(k, 32768, move |s, e| {
                let mut out = Vec::with_capacity(e - s);
                for i in s..e {
                    let ids = sh.grid.cube_edge_ids(sh.grid.ijk_of(voxel_indices[i]));
                    let mut f = [0i8; 12];
                    for (row, id) in ids.iter().enumerate() {
                        let pos = unique.binary_search(id).unwrap();
                        f[row] = edge_flux[pos];
                    }
                    out.push(f);
                }
                out
            })
        };
        per_voxel.into_iter().flatten().collect()
    } else {
        vec![[0i8; 12]; k]
    };
    stage("flux/scatter", &mut t);

    let voxel_indices = Arc::try_unwrap(voxel_indices).unwrap_or_else(|a| (*a).clone());
    FctTokens {
        resolution,
        voxel_indices,
        anchors,
        normals,
        flux,
    }
}
