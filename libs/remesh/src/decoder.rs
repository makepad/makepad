//! FCT decoder — exact port of faithcontour/decoder.py.
//!
//! Dual-contouring-style extraction: every unique voxel edge with nonzero
//! flux whose 4 incident voxels are all active yields a quad of their anchors,
//! oriented by the flux sign (flux > 0 reverses the CCW incident order), then
//! triangulated (default 'auto' = normal_abs consistency).

use std::sync::Arc;

use crate::grid::Grid;
use crate::math::*;
use crate::parallel::{par_ranges, par_sort_by_key};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TriangulationMode {
    /// normal_abs when normals are provided, else length (reference 'auto')
    Auto,
    Simple02,
    Simple13,
    Length,
    Angle,
    Normal,
    NormalAbs,
}

pub struct DecodedMesh {
    pub vertices: Vec<V3>,
    pub faces: Vec<[u32; 3]>,
    /// For each output vertex, the index of its source voxel in the token
    /// arrays (extra metadata; the reference returns only vertices/faces).
    pub used_voxels: Vec<u32>,
}

/// Decode FCT tokens to a mesh.
///
/// `voxel_indices`: [K] linear ids; `anchors`/`normals`: [K]; `flux`: [K][12].
pub fn decode(
    resolution: u32,
    voxel_indices: &[i64],
    anchors: &[V3],
    flux: &[[i8; 12]],
    normals: Option<&[V3]>,
    mode: TriangulationMode,
) -> DecodedMesh {
    let timings = std::env::var_os("REMESH_TIMINGS").is_some();
    let mut t = std::time::Instant::now();
    let stage = |name: &str, t: &mut std::time::Instant| {
        if timings {
            eprintln!("  decode/{name}: {:.3}s", t.elapsed().as_secs_f64());
        }
        *t = std::time::Instant::now();
    };
    let grid = Grid::new(resolution);
    let k = voxel_indices.len();
    if k == 0 {
        return DecodedMesh {
            vertices: Vec::new(),
            faces: Vec::new(),
            used_voxels: Vec::new(),
        };
    }

    // (edge_id, flat_idx, flux) per voxel-edge copy, then parallel sort by
    // (edge_id, flat_idx): runs = unique edges; flux per edge = the copy with
    // max |flux|, first such copy in flat order (reference scatter_max).
    let voxel_indices_arc: Arc<Vec<i64>> = Arc::new(voxel_indices.to_vec());
    let flux_arc: Arc<Vec<[i8; 12]>> = Arc::new(flux.to_vec());
    let tuples: Vec<(i64, u32, i8)> = {
        let vi = voxel_indices_arc.clone();
        let fx = flux_arc.clone();
        par_ranges(k, 32768, move |s, e| {
            let mut out = Vec::with_capacity((e - s) * 12);
            for i in s..e {
                let ids = grid.cube_edge_ids(grid.ijk_of(vi[i]));
                for (r, &id) in ids.iter().enumerate() {
                    out.push((id, (i * 12 + r) as u32, fx[i][r]));
                }
            }
            out
        })
        .into_iter()
        .flatten()
        .collect()
    };
    stage("edge_tuples", &mut t);
    let tuples = par_sort_by_key(tuples, |t| ((t.0 as i128) << 33) | t.1 as i128, false);
    stage("edge_sort", &mut t);

    // scan runs in parallel (chunk starts snapped to run boundaries)
    let n_t = tuples.len();
    let tuples = Arc::new(tuples);
    let uf_chunks: Vec<(Vec<i64>, Vec<i8>)> = {
        let tuples = tuples.clone();
        par_ranges(n_t, n_t.div_ceil(64).max(1), move |mut s, e| {
            let mut unique = Vec::new();
            let mut fluxes = Vec::new();
            if s > 0 && tuples[s].0 == tuples[s - 1].0 {
                // skip forward to the next run start
                while s < e && tuples[s].0 == tuples[s - 1].0 {
                    s += 1;
                }
            }
            let mut i = s;
            while i < e {
                let id = tuples[i].0;
                let mut best_abs = -1i8;
                let mut best = 0i8;
                let mut j = i;
                while j < n_t && tuples[j].0 == id {
                    let f = tuples[j].2;
                    if f.abs() > best_abs {
                        best_abs = f.abs();
                        best = f;
                    }
                    j += 1;
                }
                unique.push(id);
                fluxes.push(best);
                i = j;
            }
            (unique, fluxes)
        })
    };
    let mut unique: Vec<i64> = Vec::new();
    let mut edge_flux: Vec<i8> = Vec::new();
    for (u, f) in uf_chunks {
        unique.extend_from_slice(&u);
        edge_flux.extend_from_slice(&f);
    }
    let n_e = unique.len();
    stage("edge_runs", &mut t);

    // sorted (linear id, original position) for active lookup
    let sorted_active: Vec<(i64, u32)> = par_sort_by_key(
        voxel_indices
            .iter()
            .enumerate()
            .map(|(i, &l)| (l, i as u32))
            .collect(),
        |&(l, _)| l as i128,
        false,
    );
    stage("sort_active", &mut t);

    // valid edges -> oriented quads of LOCAL voxel indices
    // (order = ascending unique edge id, as in the reference)
    let unique = Arc::new(unique);
    let edge_flux = Arc::new(edge_flux);
    let sorted_active = Arc::new(sorted_active);
    let quads: Vec<[u32; 4]> = {
        let unique = unique.clone();
        let edge_flux = edge_flux.clone();
        let sorted_active = sorted_active.clone();
        par_ranges(n_e, 65536, move |s, e| {
            let mut out = Vec::new();
            for ei in s..e {
                if edge_flux[ei] == 0 {
                    continue;
                }
                let inc = grid.edge_incident_cubes(unique[ei]);
                let mut local = [0u32; 4];
                let mut ok = true;
                for (j, &c) in inc.iter().enumerate() {
                    if c < 0 {
                        ok = false;
                        break;
                    }
                    match sorted_active.binary_search_by_key(&c, |&(l, _)| l) {
                        Ok(p) => local[j] = sorted_active[p].1,
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    continue;
                }
                if edge_flux[ei] > 0 {
                    local.reverse();
                }
                out.push(local);
            }
            out
        })
        .into_iter()
        .flatten()
        .collect()
    };
    stage("quads", &mut t);

    if quads.is_empty() {
        // reference: vertices = ALL anchors, faces empty
        return DecodedMesh {
            vertices: anchors.to_vec(),
            faces: Vec::new(),
            used_voxels: (0..anchors.len() as u32).collect(),
        };
    }

    // unique used vertices (ascending), remap quads
    let used: Vec<u32> = par_sort_by_key(
        quads.iter().flatten().copied().collect(),
        |&x| x as i128,
        true,
    );
    let vertices: Vec<V3> = used.iter().map(|&u| anchors[u as usize]).collect();
    let vnormals: Option<Vec<V3>> =
        normals.map(|n| used.iter().map(|&u| n[u as usize]).collect());
    let used_arc = Arc::new(used);
    let quads: Vec<[u32; 4]> = {
        let used = used_arc.clone();
        let quads = Arc::new(quads);
        let quads2 = quads.clone();
        par_ranges(quads.len(), 65536, move |s, e| {
            let mut out = Vec::with_capacity(e - s);
            for q in &quads2[s..e] {
                out.push([
                    used.binary_search(&q[0]).unwrap() as u32,
                    used.binary_search(&q[1]).unwrap() as u32,
                    used.binary_search(&q[2]).unwrap() as u32,
                    used.binary_search(&q[3]).unwrap() as u32,
                ]);
            }
            out
        })
        .into_iter()
        .flatten()
        .collect()
    };
    stage("used+remap", &mut t);

    // triangulate
    let mode = match mode {
        TriangulationMode::Auto => {
            if vnormals.is_some() {
                TriangulationMode::NormalAbs
            } else {
                TriangulationMode::Length
            }
        }
        m => m,
    };
    let vertices_arc = Arc::new(vertices);
    let vnormals_arc = vnormals.map(Arc::new);
    let faces: Vec<[u32; 3]> = {
        let quads = Arc::new(quads);
        let vertices = vertices_arc.clone();
        let vnormals = vnormals_arc.clone();
        let quads2 = quads.clone();
        par_ranges(quads.len(), 32768, move |s, e| {
            let mut out = Vec::with_capacity((e - s) * 2);
            for q in &quads2[s..e] {
                let pattern0 = match mode {
                    TriangulationMode::Simple02 => true,
                    TriangulationMode::Simple13 => false,
                    TriangulationMode::Length => {
                        let v0 = vertices[q[0] as usize];
                        let v1 = vertices[q[1] as usize];
                        let v2 = vertices[q[2] as usize];
                        let v3 = vertices[q[3] as usize];
                        norm3(sub3(v2, v0)) <= norm3(sub3(v3, v1))
                    }
                    TriangulationMode::Angle => angle_condition(
                        vertices[q[0] as usize],
                        vertices[q[1] as usize],
                        vertices[q[2] as usize],
                        vertices[q[3] as usize],
                    ),
                    TriangulationMode::Normal | TriangulationMode::NormalAbs => {
                        let nrm = vnormals
                            .as_ref()
                            .expect("normal triangulation modes require normals");
                        normal_condition(
                            [
                                vertices[q[0] as usize],
                                vertices[q[1] as usize],
                                vertices[q[2] as usize],
                                vertices[q[3] as usize],
                            ],
                            [
                                nrm[q[0] as usize],
                                nrm[q[1] as usize],
                                nrm[q[2] as usize],
                                nrm[q[3] as usize],
                            ],
                            mode == TriangulationMode::NormalAbs,
                        )
                    }
                    TriangulationMode::Auto => unreachable!(),
                };
                if pattern0 {
                    out.push([q[0], q[1], q[2]]);
                    out.push([q[0], q[2], q[3]]);
                } else {
                    out.push([q[0], q[1], q[3]]);
                    out.push([q[1], q[2], q[3]]);
                }
            }
            out
        })
        .into_iter()
        .flatten()
        .collect()
    };
    stage("triangulate", &mut t);

    DecodedMesh {
        vertices: Arc::try_unwrap(vertices_arc).unwrap_or_else(|a| (*a).clone()),
        faces,
        used_voxels: Arc::try_unwrap(used_arc).unwrap_or_else(|a| (*a).clone()),
    }
}

/// Reference _compute_angle_condition: normalize edges, interior angles via
/// atan2(|cross|, dot); pattern0 iff angle0+angle2 < angle1+angle3.
fn angle_condition(v0: V3, v1: V3, v2: V3, v3: V3) -> bool {
    #[inline]
    fn normalize_eps(v: V3) -> V3 {
        // torch.nn.functional.normalize: v / clamp_min(norm, 1e-12)
        let l = norm3(v).max(1e-12);
        [v[0] / l, v[1] / l, v[2] / l]
    }
    #[inline]
    fn angle(e1: V3, e2: V3) -> f32 {
        let c = norm3(cross3(e1, e2));
        let d = dot3(e1, e2);
        c.atan2(d)
    }
    let e01 = normalize_eps(sub3(v1, v0));
    let e12 = normalize_eps(sub3(v2, v1));
    let e23 = normalize_eps(sub3(v3, v2));
    let e30 = normalize_eps(sub3(v0, v3));
    let a0 = angle(e01, e30);
    let a1 = angle(e12, e01);
    let a2 = angle(e23, e12);
    let a3 = angle(e30, e23);
    (a0 + a2) < (a1 + a3)
}

/// Reference _compute_normal_consistency_condition: pattern0 iff the mean
/// (over 2 tris x 3 verts) of (abs) dot(tri geometric normal, vertex normal)
/// is STRICTLY greater for the 0-2 split than the 1-3 split.
fn normal_condition(v: [V3; 4], n: [V3; 4], use_abs: bool) -> bool {
    const P0: [[usize; 3]; 2] = [[0, 1, 2], [0, 2, 3]];
    const P1: [[usize; 3]; 2] = [[0, 1, 3], [1, 2, 3]];
    #[inline]
    fn consistency(v: &[V3; 4], n: &[V3; 4], pat: &[[usize; 3]; 2], use_abs: bool) -> f32 {
        let mut sum = 0.0f32;
        for tri in pat {
            let a = v[tri[0]];
            let b = v[tri[1]];
            let c = v[tri[2]];
            let g = cross3(sub3(b, a), sub3(c, a));
            let l = norm3(g).max(1e-9);
            let gn = [g[0] / l, g[1] / l, g[2] / l];
            for &vi in tri {
                let d = dot3(gn, n[vi]);
                sum += if use_abs { d.abs() } else { d };
            }
        }
        sum / 6.0
    }
    consistency(&v, &n, &P0, use_abs) > consistency(&v, &n, &P1, use_abs)
}
