//! Indexed triangle BVH and the TRELLIS.2 reference narrow-band UDF
//! dual-contour remesher.
//!
//! This is the CPU counterpart of `cumesh.cuBVH` plus
//! `remeshing.remesh_narrow_band_dc`: build a BVH over the original decoded
//! surface, extract the `UDF - band * cell == 0` offset shell, and optionally
//! project it back toward the input. The same BVH is reusable by texture
//! baking to snap low-poly texels to the original high-resolution surface.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub dist2: f32,
    pub face: u32,
    pub point: [f32; 3],
}

#[derive(Clone, Copy, Default)]
struct BvhNode {
    bmin: [f32; 3],
    bmax: [f32; 3],
    left: u32,
    right: u32,
    begin: u32,
    count: u32,
}

/// Median-split AABB tree over an indexed triangle mesh. The input slices
/// must outlive the tree; vertex and index storage is not duplicated.
pub struct SurfaceBvh<'a> {
    positions: &'a [[f32; 3]],
    indices: &'a [u32],
    prim: Vec<u32>,
    nodes: Vec<BvhNode>,
}

impl<'a> SurfaceBvh<'a> {
    pub fn build(positions: &'a [[f32; 3]], indices: &'a [u32]) -> Result<Self, String> {
        if indices.len() % 3 != 0 {
            return Err("surface BVH indices are not triangles".to_string());
        }
        if indices.iter().any(|&v| v as usize >= positions.len()) {
            return Err("surface BVH index out of range".to_string());
        }
        let faces = indices.len() / 3;
        let mut tree = Self {
            positions,
            indices,
            prim: (0..faces as u32).collect(),
            nodes: Vec::with_capacity(faces.saturating_mul(2).div_ceil(7)),
        };
        if faces != 0 {
            tree.build_span(0, faces);
        }
        Ok(tree)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn face_vertices(&self, face: u32) -> [[f32; 3]; 3] {
        let at = face as usize * 3;
        [
            self.positions[self.indices[at] as usize],
            self.positions[self.indices[at + 1] as usize],
            self.positions[self.indices[at + 2] as usize],
        ]
    }

    fn build_span(&mut self, begin: usize, end: usize) -> u32 {
        let node_id = self.nodes.len() as u32;
        self.nodes.push(BvhNode::default());
        let mut bmin = [f32::INFINITY; 3];
        let mut bmax = [f32::NEG_INFINITY; 3];
        let mut cmin = [f32::INFINITY; 3];
        let mut cmax = [f32::NEG_INFINITY; 3];
        for i in begin..end {
            let tri = self.face_vertices(self.prim[i]);
            let mut center = [0.0f32; 3];
            for p in tri {
                for axis in 0..3 {
                    bmin[axis] = bmin[axis].min(p[axis]);
                    bmax[axis] = bmax[axis].max(p[axis]);
                    center[axis] += p[axis] / 3.0;
                }
            }
            for axis in 0..3 {
                cmin[axis] = cmin[axis].min(center[axis]);
                cmax[axis] = cmax[axis].max(center[axis]);
            }
        }
        let count = end - begin;
        if count <= 8 {
            self.nodes[node_id as usize] = BvhNode {
                bmin,
                bmax,
                begin: begin as u32,
                count: count as u32,
                ..Default::default()
            };
            return node_id;
        }
        let mut axis = 0usize;
        if cmax[1] - cmin[1] > cmax[axis] - cmin[axis] {
            axis = 1;
        }
        if cmax[2] - cmin[2] > cmax[axis] - cmin[axis] {
            axis = 2;
        }
        let mid = begin + count / 2;
        let positions = self.positions;
        let indices = self.indices;
        self.prim[begin..end].select_nth_unstable_by(mid - begin, |&fa, &fb| {
            let centroid = |face: u32| {
                let at = face as usize * 3;
                (positions[indices[at] as usize][axis]
                    + positions[indices[at + 1] as usize][axis]
                    + positions[indices[at + 2] as usize][axis])
                    / 3.0
            };
            centroid(fa)
                .total_cmp(&centroid(fb))
                .then_with(|| fa.cmp(&fb))
        });
        let left = self.build_span(begin, mid);
        let right = self.build_span(mid, end);
        self.nodes[node_id as usize] = BvhNode {
            bmin,
            bmax,
            left,
            right,
            ..Default::default()
        };
        node_id
    }

    /// Exact closest point within `max_dist`. Returns `None` when the surface
    /// is farther away than the bound.
    pub fn closest(&self, p: [f32; 3], max_dist: f32) -> Option<SurfaceHit> {
        if self.nodes.is_empty() || !max_dist.is_finite() || max_dist < 0.0 {
            return None;
        }
        let mut best = SurfaceHit {
            dist2: max_dist * max_dist,
            face: u32::MAX,
            point: [0.0; 3],
        };
        let root_dist = point_box_dist2(p, self.nodes[0].bmin, self.nodes[0].bmax);
        let mut stack = Vec::with_capacity(64);
        stack.push((0u32, root_dist));
        while let Some((node_id, node_dist)) = stack.pop() {
            if node_dist >= best.dist2 {
                continue;
            }
            let node = self.nodes[node_id as usize];
            if node.count != 0 {
                for i in node.begin..node.begin + node.count {
                    let face = self.prim[i as usize];
                    let point = closest_on_triangle(p, self.face_vertices(face));
                    let dist2 = squared_distance(p, point);
                    if dist2 < best.dist2
                        || (dist2 == best.dist2 && face < best.face)
                    {
                        best = SurfaceHit { dist2, face, point };
                    }
                }
                continue;
            }
            let left = self.nodes[node.left as usize];
            let right = self.nodes[node.right as usize];
            let dl = point_box_dist2(p, left.bmin, left.bmax);
            let dr = point_box_dist2(p, right.bmin, right.bmax);
            if dl < dr {
                if dr < best.dist2 {
                    stack.push((node.right, dr));
                }
                if dl < best.dist2 {
                    stack.push((node.left, dl));
                }
            } else {
                if dl < best.dist2 {
                    stack.push((node.left, dl));
                }
                if dr < best.dist2 {
                    stack.push((node.right, dr));
                }
            }
        }
        (best.face != u32::MAX).then_some(best)
    }
}

#[derive(Default)]
pub struct SurfaceMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl SurfaceMesh {
    pub fn ok(&self) -> bool {
        !self.positions.is_empty() && !self.indices.is_empty()
    }
}

/// Reference narrow-band UDF dual contouring. Input positions are expected in
/// TRELLIS canonical space around `[-0.5, 0.5]^3`; `resolution` is the
/// attribute/shape grid resolution and `band` is measured in grid cells.
pub fn remesh_narrow_band_dc(
    positions: &[[f32; 3]],
    indices: &[u32],
    bvh: &SurfaceBvh<'_>,
    resolution: usize,
    band: usize,
    project_back: f32,
) -> Result<SurfaceMesh, String> {
    remesh_narrow_band_dc_ctl(
        positions,
        indices,
        bvh,
        resolution,
        band,
        project_back,
        &mut |_, _| true,
    )
}

/// Same as [`remesh_narrow_band_dc`], with a cancel/progress hook.
/// `ctl(label, 0..=1)` returning false aborts.
pub fn remesh_narrow_band_dc_ctl(
    positions: &[[f32; 3]],
    indices: &[u32],
    bvh: &SurfaceBvh<'_>,
    resolution: usize,
    band: usize,
    project_back: f32,
    ctl: &mut impl FnMut(&str, f64) -> bool,
) -> Result<SurfaceMesh, String> {
    if indices.is_empty() || bvh.is_empty() || resolution < 8 || band == 0 {
        return Err("narrow-band remesh has invalid input".to_string());
    }
    if indices.len() % 3 != 0 {
        return Err("narrow-band remesh indices are not triangles".to_string());
    }
    if !ctl("voxelize", 0.02) {
        return Err("narrow-band remesh cancelled".to_string());
    }
    let res = resolution as i32;
    let scale = (resolution + 3 * band) as f32 / resolution as f32;
    let cell = scale / resolution as f32;
    let eps = band as f32 * cell;
    let keep = 0.87 * cell;
    let nbits = resolution
        .checked_mul(resolution)
        .and_then(|v| v.checked_mul(resolution))
        .ok_or_else(|| "narrow-band grid overflow".to_string())?;
    let words: Vec<AtomicU64> = (0..nbits.div_ceil(64))
        .map(|_| AtomicU64::new(0))
        .collect();
    let dil = band as i32 + 1;
    let faces = indices.len() / 3;
    let threads = worker_count();
    let face_waves = 8usize;
    let faces_per_wave = faces.div_ceil(face_waves).max(1);
    for wave in 0..face_waves {
        let face0 = wave * faces_per_wave;
        if face0 >= faces {
            break;
        }
        let face1 = (face0 + faces_per_wave).min(faces);
        if !ctl(
            "voxelize",
            0.02 + 0.10 * (wave as f64 / face_waves as f64),
        ) {
            return Err("narrow-band remesh cancelled".to_string());
        }
        let face_rows = &indices[face0 * 3..face1 * 3];
        let chunk = (face1 - face0).div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for face_part in face_rows.chunks(chunk * 3) {
                let words = &words;
                scope.spawn(move || {
                    for tri in face_part.chunks_exact(3) {
                        let a = positions[tri[0] as usize];
                        let b = positions[tri[1] as usize];
                        let c = positions[tri[2] as usize];
                        let mut c0 = [0i32; 3];
                        let mut c1 = [0i32; 3];
                        for axis in 0..3 {
                            let lo = a[axis].min(b[axis]).min(c[axis]);
                            let hi = a[axis].max(b[axis]).max(c[axis]);
                            c0[axis] = (((lo / scale + 0.5) * resolution as f32).floor() as i32
                                - dil)
                                .clamp(0, res - 1);
                            c1[axis] = (((hi / scale + 0.5) * resolution as f32).floor() as i32
                                + dil)
                                .clamp(0, res - 1);
                        }
                        for x in c0[0]..=c1[0] {
                            for y in c0[1]..=c1[1] {
                                for z in c0[2]..=c1[2] {
                                    let id = ((x as usize * resolution + y as usize) * resolution
                                        + z as usize) as u64;
                                    words[(id >> 6) as usize]
                                        .fetch_or(1u64 << (id & 63), Ordering::Relaxed);
                                }
                            }
                        }
                    }
                });
            }
        });
    }

    let mut candidates = Vec::<u64>::new();
    for (word_id, word) in words.iter().enumerate() {
        let mut bits = word.load(Ordering::Relaxed);
        while bits != 0 {
            let bit = bits.trailing_zeros() as u64;
            bits &= bits - 1;
            let cell_id = word_id as u64 * 64 + bit;
            if cell_id < nbits as u64 {
                candidates.push(cell_id);
            }
        }
    }
    drop(words);
    if !ctl("active cells", 0.16) {
        return Err("narrow-band remesh cancelled".to_string());
    }

    let mut active = vec![0u8; candidates.len()];
    let cell_waves = 8usize;
    let cells_per_wave = candidates.len().div_ceil(cell_waves).max(1);
    for wave in 0..cell_waves {
        let start = wave * cells_per_wave;
        if start >= candidates.len() {
            break;
        }
        let end = (start + cells_per_wave).min(candidates.len());
        if !ctl(
            "active cells",
            0.16 + 0.28 * (wave as f64 / cell_waves as f64),
        ) {
            return Err("narrow-band remesh cancelled".to_string());
        }
        let cells = &candidates[start..end];
        let flags = &mut active[start..end];
        let chunk = cells.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (cells, flags) in cells.chunks(chunk).zip(flags.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (&id, flag) in cells.iter().zip(flags) {
                        let x = id as usize / (resolution * resolution);
                        let y = id as usize / resolution % resolution;
                        let z = id as usize % resolution;
                        let p = [
                            ((x as f32 + 0.5) / resolution as f32 - 0.5) * scale,
                            ((y as f32 + 0.5) / resolution as f32 - 0.5) * scale,
                            ((z as f32 + 0.5) / resolution as f32 - 0.5) * scale,
                        ];
                        if let Some(hit) = bvh.closest(p, eps + keep) {
                            if (hit.dist2.sqrt() - eps).abs() < keep {
                                *flag = 1;
                            }
                        }
                    }
                });
            }
        });
    }
    let mut acoord = Vec::<[i32; 3]>::new();
    for (&id, &flag) in candidates.iter().zip(&active) {
        if flag == 0 {
            continue;
        }
        acoord.push([
            (id as usize / (resolution * resolution)) as i32,
            (id as usize / resolution % resolution) as i32,
            (id as usize % resolution) as i32,
        ]);
    }
    drop(candidates);
    drop(active);
    if acoord.len() < 100 {
        return Err(format!(
            "narrow-band remesh found only {} active cells",
            acoord.len()
        ));
    }

    let mut vox = HashMap::<u64, usize>::with_capacity(acoord.len() * 2);
    for (i, &c) in acoord.iter().enumerate() {
        vox.insert(key3(c[0], c[1], c[2]), i);
    }
    let mut vcoord = Vec::<[i32; 3]>::with_capacity(acoord.len() * 3);
    let mut vmap = HashMap::<u64, usize>::with_capacity(acoord.len() * 3);
    for &c in &acoord {
        for dx in 0..2 {
            for dy in 0..2 {
                for dz in 0..2 {
                    let p = [c[0] + dx, c[1] + dy, c[2] + dz];
                    let key = key3(p[0], p[1], p[2]);
                    if let std::collections::hash_map::Entry::Vacant(entry) = vmap.entry(key) {
                        entry.insert(vcoord.len());
                        vcoord.push(p);
                    }
                }
            }
        }
    }
    let mut fvert = vec![0.0f32; vcoord.len()];
    let field_waves = 8usize;
    let verts_per_wave = vcoord.len().div_ceil(field_waves).max(1);
    for wave in 0..field_waves {
        let start = wave * verts_per_wave;
        if start >= vcoord.len() {
            break;
        }
        let end = (start + verts_per_wave).min(vcoord.len());
        if !ctl("field", 0.48 + 0.22 * (wave as f64 / field_waves as f64)) {
            return Err("narrow-band remesh cancelled".to_string());
        }
        let coords = &vcoord[start..end];
        let values = &mut fvert[start..end];
        let chunk = coords.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (coords, values) in coords.chunks(chunk).zip(values.chunks_mut(chunk)) {
                scope.spawn(move || {
                    for (&c, value) in coords.iter().zip(values) {
                        let p = [
                            (c[0] as f32 / resolution as f32 - 0.5) * scale,
                            (c[1] as f32 / resolution as f32 - 0.5) * scale,
                            (c[2] as f32 / resolution as f32 - 0.5) * scale,
                        ];
                        *value = bvh
                            .closest(p, eps + 2.0 * cell)
                            .map(|hit| hit.dist2.sqrt())
                            .unwrap_or(eps + 2.0 * cell)
                            - eps;
                    }
                });
            }
        });
    }
    if !ctl("contour", 0.72) {
        return Err("narrow-band remesh cancelled".to_string());
    }

    let mut dual = vec![[0.0f32; 3]; acoord.len()];
    let mut owned = vec![[0i8; 3]; acoord.len()];
    let chunk = acoord.len().div_ceil(threads).max(1);
    std::thread::scope(|scope| {
        for ((coords, dual_out), owned_out) in acoord
            .chunks(chunk)
            .zip(dual.chunks_mut(chunk))
            .zip(owned.chunks_mut(chunk))
        {
            let vmap = &vmap;
            let fvert = &fvert;
            scope.spawn(move || {
                let fval = |c: [i32; 3]| -> f32 {
                    vmap
                        .get(&key3(c[0], c[1], c[2]))
                        .map(|&i| fvert[i])
                        .unwrap_or(1e9)
                };
                for ((&c, d), own) in coords.iter().zip(dual_out).zip(owned_out) {
                    let mut sum = [0.0f64; 3];
                    let mut count = 0usize;
                    for axis in 0..3 {
                        for u in 0..2 {
                            for v in 0..2 {
                                let mut a0 = c;
                                a0[(axis + 1) % 3] += u;
                                a0[(axis + 2) % 3] += v;
                                let mut a1 = a0;
                                a1[axis] += 1;
                                let v0 = fval(a0);
                                let v1 = fval(a1);
                                let forward = v0 < 0.0 && v1 >= 0.0;
                                let reverse = v0 >= 0.0 && v1 < 0.0;
                                if forward || reverse {
                                    let t = -v0 / (v1 - v0);
                                    let mut point = [a0[0] as f64, a0[1] as f64, a0[2] as f64];
                                    point[axis] += t as f64;
                                    for k in 0..3 {
                                        sum[k] += point[k];
                                    }
                                    count += 1;
                                }
                                if u == 1 && v == 1 {
                                    own[axis] = if forward { 1 } else if reverse { -1 } else { 0 };
                                }
                            }
                        }
                    }
                    if count != 0 {
                        for k in 0..3 {
                            d[k] = (sum[k] / count as f64) as f32;
                        }
                    } else {
                        *d = [c[0] as f32 + 0.5, c[1] as f32 + 0.5, c[2] as f32 + 0.5];
                    }
                }
            });
        }
    });

    const OFFSETS: [[[i32; 3]; 4]; 3] = [
        [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]],
        [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]],
        [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]],
    ];
    const SPLIT_NEG: [usize; 6] = [0, 1, 2, 0, 2, 3];
    const SPLIT_POS: [usize; 6] = [0, 2, 1, 0, 3, 2];
    let mut faces_out = Vec::<u32>::with_capacity(acoord.len() * 6);
    let mut used = vec![false; acoord.len()];
    for (i, &c) in acoord.iter().enumerate() {
        for axis in 0..3 {
            let direction = owned[i][axis];
            if direction == 0 {
                continue;
            }
            let mut quad = [0usize; 4];
            let mut valid = true;
            for k in 0..4 {
                let o = OFFSETS[axis][k];
                let key = key3(c[0] + o[0], c[1] + o[1], c[2] + o[2]);
                match vox.get(&key) {
                    Some(&index) => quad[k] = index,
                    None => {
                        valid = false;
                        break;
                    }
                }
            }
            if !valid {
                continue;
            }
            let split = if direction > 0 { SPLIT_POS } else { SPLIT_NEG };
            for k in split {
                faces_out.push(quad[k] as u32);
            }
            for q in quad {
                used[q] = true;
            }
        }
    }
    if faces_out.is_empty() {
        return Err("narrow-band remesh produced no faces".to_string());
    }
    if !ctl("faces", 0.92) {
        return Err("narrow-band remesh cancelled".to_string());
    }
    let mut remap = vec![u32::MAX; acoord.len()];
    let mut out = SurfaceMesh {
        positions: Vec::new(),
        indices: faces_out,
    };
    for (i, &is_used) in used.iter().enumerate() {
        if !is_used {
            continue;
        }
        remap[i] = out.positions.len() as u32;
        out.positions.push([
            (dual[i][0] / resolution as f32 - 0.5) * scale,
            (dual[i][1] / resolution as f32 - 0.5) * scale,
            (dual[i][2] / resolution as f32 - 0.5) * scale,
        ]);
    }
    for index in &mut out.indices {
        *index = remap[*index as usize];
    }
    if project_back > 0.0 {
        let amount = project_back.clamp(0.0, 1.0);
        let chunk = out.positions.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for points in out.positions.chunks_mut(chunk) {
                scope.spawn(move || {
                    for point in points {
                        if let Some(hit) = bvh.closest(*point, eps + 2.0 * cell) {
                            for axis in 0..3 {
                                point[axis] += amount * (hit.point[axis] - point[axis]);
                            }
                        }
                    }
                });
            }
        });
    }
    Ok(out)
}

#[inline]
fn worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(8)
        .clamp(1, 16)
}

#[inline]
fn key3(x: i32, y: i32, z: i32) -> u64 {
    ((x as u64) << 42) | ((y as u64) << 21) | z as u64
}

#[inline]
fn squared_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[inline]
fn point_box_dist2(p: [f32; 3], bmin: [f32; 3], bmax: [f32; 3]) -> f32 {
    let mut out = 0.0;
    for axis in 0..3 {
        let d = if p[axis] < bmin[axis] {
            bmin[axis] - p[axis]
        } else if p[axis] > bmax[axis] {
            p[axis] - bmax[axis]
        } else {
            0.0
        };
        out += d * d;
    }
    out
}

/// Ericson, Real-Time Collision Detection §5.1.5.
fn closest_on_triangle(p: [f32; 3], tri: [[f32; 3]; 3]) -> [f32; 3] {
    let [a, b, c] = tri;
    let sub = |x: [f32; 3], y: [f32; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let dot = |x: [f32; 3], y: [f32; 3]| x[0] * y[0] + x[1] * y[1] + x[2] * y[2];
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [a[0] + v * ab[0], a[1] + v * ab[1], a[2] + v * ab[2]];
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [a[0] + w * ac[0], a[1] + w * ac[1], a[2] + w * ac[2]];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [
            b[0] + w * (c[0] - b[0]),
            b[1] + w * (c[1] - b[1]),
            b[2] + w * (c[2] - b[2]),
        ];
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> (Vec<[f32; 3]>, Vec<u32>) {
        let p = vec![
            [-0.2, -0.2, -0.2], [0.2, -0.2, -0.2], [0.2, 0.2, -0.2], [-0.2, 0.2, -0.2],
            [-0.2, -0.2, 0.2], [0.2, -0.2, 0.2], [0.2, 0.2, 0.2], [-0.2, 0.2, 0.2],
        ];
        let i = vec![
            0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4,
            1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6, 3, 0, 4, 3, 4, 7,
        ];
        (p, i)
    }

    #[test]
    fn bvh_projects_to_triangle() {
        let p = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let i = vec![0, 1, 2];
        let bvh = SurfaceBvh::build(&p, &i).unwrap();
        let hit = bvh.closest([0.25, 0.25, 0.5], 1.0).unwrap();
        assert_eq!(hit.face, 0);
        assert!((hit.dist2 - 0.25).abs() < 1e-6);
        assert_eq!(hit.point, [0.25, 0.25, 0.0]);
    }

    #[test]
    fn narrow_band_reports_inner_stages() {
        let (p, i) = cube();
        let bvh = SurfaceBvh::build(&p, &i).unwrap();
        let mut labels = Vec::new();
        let out = remesh_narrow_band_dc_ctl(&p, &i, &bvh, 32, 1, 0.0, &mut |label, _| {
            labels.push(label.to_string());
            true
        })
        .unwrap();
        assert!(out.ok());
        for need in ["voxelize", "active cells", "field", "contour", "faces"] {
            assert!(
                labels.iter().any(|l| l == need),
                "missing remesh stage {need} in {labels:?}"
            );
        }
    }

    #[test]
    fn narrow_band_cube_is_closed() {
        let (p, i) = cube();
        let bvh = SurfaceBvh::build(&p, &i).unwrap();
        let out = remesh_narrow_band_dc(&p, &i, &bvh, 32, 1, 0.0).unwrap();
        assert!(out.ok());
        let mut edges = Vec::with_capacity(out.indices.len());
        for tri in out.indices.chunks_exact(3) {
            for e in 0..3 {
                let (a, b) = (tri[e], tri[(e + 1) % 3]);
                edges.push(if a < b { (a, b) } else { (b, a) });
            }
        }
        edges.sort_unstable();
        let mut at = 0;
        while at < edges.len() {
            let mut end = at + 1;
            while end < edges.len() && edges[end] == edges[at] {
                end += 1;
            }
            assert_eq!(end - at, 2, "edge {:?}", edges[at]);
            at = end;
        }
    }
}
