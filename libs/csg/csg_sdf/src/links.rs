//! Many separate links (closed centreline polylines swept by one radius), as
//! one distance field: chains and chainmail. A grid over the segments keeps
//! each query local, so thousands of links cost what a few nearby ones do.
//!
//! The field is `max(box, min(R, nearest))` (then clipped at the bed): exact
//! wherever the surface is nearer than the search radius `R`, a lower bound
//! beyond it, and 1-Lipschitz everywhere, as the mesher and the slicer need.

use crate::shape::P3;
use makepad_csg_math::Vec3d;

/// One link: a closed loop (point i joins point i + 1, the last the first).
#[derive(Clone, Debug, PartialEq)]
pub struct LinkLoop {
    pub pts: Vec<P3>,
    /// Material slot (0: the enclosing one).
    pub slot: u8,
}

/// Links as a field.
#[derive(Debug)]
pub struct Links {
    pub loops: Vec<LinkLoop>,
    pub radius: f64,
    /// Keep only `dot(n, p) >= d` (`(n, d)`, n unit): the bed.
    pub clip: Option<(P3, f64)>,
    pub min: P3,
    pub max: P3,
    key: [u8; 32],
    // the grid: cell size, origin, dims; per cell a range into `cell_segs`
    cell: f64,
    origin: P3,
    dims: [usize; 3],
    starts: Vec<u32>,
    cell_segs: Vec<u32>,
    // per segment: a, b, loop
    segs: Vec<(P3, P3, u32)>,
}


impl Links {
    pub fn new(loops: Vec<LinkLoop>, radius: f64, clip: Option<(P3, f64)>) -> Links {
        // the canonical bytes: radius, clip, then every loop's slot, point count and points
        let mut bytes: Vec<u8> = Vec::new();
        bytes.extend_from_slice(&radius.to_bits().to_le_bytes());
        match clip {
            Some((n, d)) => {
                bytes.push(1);
                for x in n.iter().chain(std::iter::once(&d)) {
                    bytes.extend_from_slice(&x.to_bits().to_le_bytes());
                }
            }
            None => bytes.push(0),
        }
        let mut segs = Vec::new();
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for (li, l) in loops.iter().enumerate() {
            bytes.push(l.slot);
            bytes.extend_from_slice(&(l.pts.len() as u64).to_le_bytes());
            let n = l.pts.len();
            for i in 0..n {
                let (a, b) = (l.pts[i], l.pts[(i + 1) % n]);
                for k in 0..3 {
                    bytes.extend_from_slice(&a[k].to_bits().to_le_bytes());
                    min[k] = min[k].min(a[k] - radius);
                    max[k] = max[k].max(a[k] + radius);
                }
                if n > 1 {
                    segs.push((a, b, li as u32));
                }
            }
        }
        if segs.is_empty() {
            min = [0.0; 3];
            max = [0.0; 3];
        }
        // an axis-aligned clip cuts the box too
        if let Some((n, d)) = clip {
            for k in 0..3 {
                if (n[k].abs() - 1.0).abs() < 1e-12 {
                    if n[k] > 0.0 {
                        min[k] = min[k].max(d);
                    } else {
                        max[k] = max[k].min(-d);
                    }
                }
            }
        }
        let key = makepad_core_util::sha256::sha256(&bytes);
        // cells about two bar diameters: a query looks at 2-3 cells a side
        let cell = (4.0 * radius).max(1e-3);
        let origin = min.map(|x| x - cell);
        let dims: [usize; 3] = std::array::from_fn(|k| (((max[k] - min[k]) / cell).ceil() as usize + 3).max(1));
        let ncell = dims[0] * dims[1] * dims[2];
        let index = |c: [usize; 3]| (c[2] * dims[1] + c[1]) * dims[0] + c[0];
        let span = |a: P3, b: P3| -> ([usize; 3], [usize; 3]) {
            let lo: [usize; 3] = std::array::from_fn(|k| (((a[k].min(b[k]) - radius - origin[k]) / cell).floor().max(0.0) as usize).min(dims[k] - 1));
            let hi: [usize; 3] = std::array::from_fn(|k| (((a[k].max(b[k]) + radius - origin[k]) / cell).floor().max(0.0) as usize).min(dims[k] - 1));
            (lo, hi)
        };
        let mut counts = vec![0u32; ncell + 1];
        for (a, b, _) in &segs {
            let (lo, hi) = span(*a, *b);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        counts[index([x, y, z]) + 1] += 1;
                    }
                }
            }
        }
        for i in 0..ncell {
            counts[i + 1] += counts[i];
        }
        let starts = counts.clone();
        let mut fill = counts;
        let mut cell_segs = vec![0u32; starts[ncell] as usize];
        for (si, (a, b, _)) in segs.iter().enumerate() {
            let (lo, hi) = span(*a, *b);
            for z in lo[2]..=hi[2] {
                for y in lo[1]..=hi[1] {
                    for x in lo[0]..=hi[0] {
                        let c = index([x, y, z]);
                        cell_segs[fill[c] as usize] = si as u32;
                        fill[c] += 1;
                    }
                }
            }
        }
        Links { loops, radius, clip, min, max, key, cell, origin, dims, starts, cell_segs, segs }
    }

    /// The same links with the slots `slot(centre of the loop's points)` picks (None: unchanged).
    pub fn with_slots(&self, slot: impl Fn(P3) -> Option<u8>) -> Links {
        let loops = self
            .loops
            .iter()
            .map(|l| {
                let n = l.pts.len().max(1) as f64;
                let c = l.pts.iter().fold([0.0; 3], |a, p| [a[0] + p[0] / n, a[1] + p[1] / n, a[2] + p[2] / n]);
                LinkLoop { pts: l.pts.clone(), slot: slot(c).unwrap_or(l.slot) }
            })
            .collect();
        Links::new(loops, self.radius, self.clip)
    }

    /// The links' identity: the sha256 of their canonical bytes (geometry, radius, slots, clip).
    pub fn key(&self) -> [u8; 32] {
        self.key
    }

    /// Signed distance and the nearest link's slot.
    pub fn distance_slot(&self, p: Vec3d) -> (f64, u8) {
        let q = [p.x, p.y, p.z];
        let r = self.radius;
        // the search radius: the surface within `reach` of q is always found
        let reach = self.cell;
        let lo: [isize; 3] = std::array::from_fn(|k| ((q[k] - reach - r - self.origin[k]) / self.cell).floor() as isize);
        let hi: [isize; 3] = std::array::from_fn(|k| ((q[k] + reach + r - self.origin[k]) / self.cell).floor() as isize);
        let mut best = f64::INFINITY;
        let mut slot = 0u8;
        let mut seen_last = u32::MAX;
        for z in lo[2].max(0)..=hi[2].min(self.dims[2] as isize - 1) {
            for y in lo[1].max(0)..=hi[1].min(self.dims[1] as isize - 1) {
                for x in lo[0].max(0)..=hi[0].min(self.dims[0] as isize - 1) {
                    let c = (z as usize * self.dims[1] + y as usize) * self.dims[0] + x as usize;
                    for &si in &self.cell_segs[self.starts[c] as usize..self.starts[c + 1] as usize] {
                        if si == seen_last {
                            continue;
                        }
                        seen_last = si;
                        let (a, b, li) = self.segs[si as usize];
                        let d = segment_distance(q, a, b) - r;
                        if d < best {
                            best = d;
                            slot = self.loops[li as usize].slot;
                        }
                    }
                }
            }
        }
        // outside the bounds box: its distance is a lower bound everywhere
        let out: [f64; 3] = std::array::from_fn(|k| (self.min[k] - q[k]).max(q[k] - self.max[k]).max(0.0));
        let d_box = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
        // (inside the box the local search decides; the box holds every link with its radius)
        let near = best.min(reach);
        let mut d = if d_box > 0.0 { d_box.max(near) } else { near };
        if let Some((n, off)) = self.clip {
            d = d.max(off - (n[0] * q[0] + n[1] * q[1] + n[2] * q[2]));
        }
        (d, slot)
    }

    pub fn distance(&self, p: Vec3d) -> f64 {
        self.distance_slot(p).0
    }

    /// Calls `f(a, b, loop)` for every segment whose tube may reach the box `lo..hi` (a segment can come
    /// more than once).
    pub fn segments_near(&self, lo: P3, hi: P3, mut f: impl FnMut(P3, P3, usize)) {
        let r = self.radius;
        let c0: [isize; 3] = std::array::from_fn(|k| ((lo[k] - r - self.origin[k]) / self.cell).floor() as isize);
        let c1: [isize; 3] = std::array::from_fn(|k| ((hi[k] + r - self.origin[k]) / self.cell).floor() as isize);
        for z in c0[2].max(0)..=c1[2].min(self.dims[2] as isize - 1) {
            for y in c0[1].max(0)..=c1[1].min(self.dims[1] as isize - 1) {
                for x in c0[0].max(0)..=c1[0].min(self.dims[0] as isize - 1) {
                    let c = (z as usize * self.dims[1] + y as usize) * self.dims[0] + x as usize;
                    for &si in &self.cell_segs[self.starts[c] as usize..self.starts[c + 1] as usize] {
                        let (a, b, li) = self.segs[si as usize];
                        f(a, b, li as usize);
                    }
                }
            }
        }
    }
}

impl Links {
    /// The links as one closed triangle mesh: each loop a tube of `sides` facets round, mitred at every bend
    /// (an ellipse in the bisecting plane), its rings carried round the loop without twist; vertices below the
    /// clip plane are pressed onto it (a flat foot). Bends should be gentle (at most about 30 degrees): round
    /// sharp corners into arcs first.
    pub fn mesh(&self, sides: u32) -> makepad_csg_mesh::mesh::TriMesh {
        let sides = sides.max(3) as usize;
        let mut m = makepad_csg_mesh::mesh::TriMesh::new();
        let sub = |a: P3, b: P3| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let dot = |a: P3, b: P3| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let cross = |a: P3, b: P3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let norm = |a: P3| {
            let l = dot(a, a).sqrt();
            if l > 0.0 { a.map(|x| x / l) } else { a }
        };
        for l in &self.loops {
            let n = l.pts.len();
            if n < 3 {
                continue;
            }
            let p = &l.pts;
            let dir: Vec<P3> = (0..n).map(|i| norm(sub(p[(i + 1) % n], p[i]))).collect();
            // per vertex: the bisecting tangent, the bend direction and the miter stretch
            let mut t = Vec::with_capacity(n);
            let mut bend = Vec::with_capacity(n);
            for i in 0..n {
                let (din, dout) = (dir[(i + n - 1) % n], dir[i]);
                let tv = norm([din[0] + dout[0], din[1] + dout[1], din[2] + dout[2]]);
                let c = dot(din, dout).clamp(-1.0, 1.0);
                let half = 0.5 * c.acos();
                let b = norm(sub(dout, din));
                t.push(tv);
                bend.push((b, 1.0 / half.cos().max(0.2) - 1.0));
            }
            // frames carried round the loop, the closing twist spread evenly
            let any = if t[0][0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] };
            let mut u = vec![norm(cross(cross(t[0], any), t[0]))];
            for i in 1..=n {
                let ti = t[i % n];
                let prev = u[i - 1];
                u.push(norm(sub(prev, ti.map(|x| x * dot(prev, ti)))));
            }
            let (u0, un) = (u[0], u[n]);
            let twist = dot(cross(u0, un), t[0]).atan2(dot(u0, un));
            let base = m.vertices.len() as u32;
            for i in 0..n {
                let ti = t[i];
                let ang = -twist * i as f64 / n as f64;
                let (sa, ca) = ang.sin_cos();
                let ui = u[i];
                let vi = cross(ti, ui);
                let ur: P3 = std::array::from_fn(|k| ca * ui[k] + sa * vi[k]);
                let vr = cross(ti, ur);
                let (b, stretch) = bend[i];
                for s in 0..sides {
                    let phi = std::f64::consts::TAU * s as f64 / sides as f64;
                    let (sp, cp) = phi.sin_cos();
                    let w: P3 = std::array::from_fn(|k| cp * ur[k] + sp * vr[k]);
                    let wb = dot(w, b) * stretch;
                    let mut q: P3 = std::array::from_fn(|k| p[i][k] + self.radius * (w[k] + wb * b[k]));
                    if let Some((cn, off)) = self.clip {
                        let h = dot(cn, q) - off;
                        if h < 0.0 {
                            q = std::array::from_fn(|k| q[k] - h * cn[k]);
                        }
                    }
                    m.add_vertex(Vec3d::new(q[0], q[1], q[2]));
                }
            }
            for i in 0..n {
                let j = (i + 1) % n;
                for s in 0..sides {
                    let s1 = (s + 1) % sides;
                    let a = base + (i * sides + s) as u32;
                    let b = base + (i * sides + s1) as u32;
                    let c = base + (j * sides + s1) as u32;
                    let d = base + (j * sides + s) as u32;
                    // outward: rings advance along the loop, sides turn about it right-handed
                    m.add_triangle(a, b, c);
                    m.add_triangle(a, c, d);
                }
            }
        }
        m
    }
}

pub fn segment_distance(p: P3, a: P3, b: P3) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1] + ab[2] * ab[2];
    let t = if l2 > 0.0 { ((ap[0] * ab[0] + ap[1] * ab[1] + ap[2] * ab[2]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let d = [ap[0] - ab[0] * t, ap[1] - ab[1] * t, ap[2] - ab[2] * t];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ring(c: P3, a: f64, slot: u8) -> LinkLoop {
        LinkLoop { pts: vec![[c[0] - a, c[1], c[2] - a], [c[0] + a, c[1], c[2] - a], [c[0] + a, c[1], c[2] + a], [c[0] - a, c[1], c[2] + a]], slot }
    }

    #[test]
    fn exact_near_and_lipschitz_far() {
        let loops: Vec<LinkLoop> = (0..20).map(|i| ring([i as f64 * 12.0, 1.0, 0.0], 4.0, (i % 3) as u8)).collect();
        let links = Links::new(loops.clone(), 0.8, Some(([0.0, 1.0, 0.0], 0.0)));
        let brute = |p: P3| {
            let mut best = (f64::INFINITY, 0u8);
            for l in &loops {
                for i in 0..4 {
                    let d = segment_distance(p, l.pts[i], l.pts[(i + 1) % 4]) - 0.8;
                    if d < best.0 {
                        best = (d, l.slot);
                    }
                }
            }
            (best.0.max(-p[1]), best.1)
        };
        let mut prev: Option<(P3, f64)> = None;
        for k in 0..4000 {
            let t = k as f64 * 0.061;
            let p = [t * 1.0 - 20.0, 1.0 + 6.0 * (t * 0.37).sin(), 5.0 * (t * 0.53).cos()];
            let (d, s) = links.distance_slot(Vec3d::new(p[0], p[1], p[2]));
            let (e, es) = brute(p);
            assert!(d <= e + 1e-9, "never above the true distance: {d} {e} at {p:?}");
            if e < links.cell {
                assert!((d - e).abs() < 1e-9, "exact near the surface: {d} {e} at {p:?}");
                if e < 0.0 && p[1] > 1e-6 {
                    assert_eq!(s, es);
                }
            }
            if let Some((q, dq)) = prev {
                let step = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt();
                assert!((d - dq).abs() <= step + 1e-9, "1-Lipschitz");
            }
            prev = Some((p, d));
        }
        assert_ne!(links.key(), Links::new(loops, 0.9, None).key());
    }

    #[test]
    fn mesh_is_closed_and_outward() {
        // a smooth ring (bends of 15 degrees)
        let pts: Vec<P3> = (0..24).map(|k| {
            let a = k as f64 / 24.0 * std::f64::consts::TAU;
            [10.0 * a.cos(), 2.0, 10.0 * a.sin()]
        }).collect();
        let links = Links::new(vec![LinkLoop { pts, slot: 0 }], 1.0, Some(([0.0, 1.0, 0.0], 1.5)));
        let m = links.mesh(12);
        let mut edges = std::collections::HashMap::new();
        for t in &m.triangles {
            for k in 0..3 {
                *edges.entry((t[k], t[(k + 1) % 3])).or_insert(0i32) += 1;
            }
        }
        for ((a, b), c) in &edges {
            assert_eq!(*c, 1);
            assert_eq!(edges.get(&(*b, *a)), Some(&1), "closed: every edge once each way");
        }
        // signed volume ~ the torus volume less the clipped foot, and positive (outward)
        let mut vol = 0.0;
        for t in &m.triangles {
            let (a, b, c) = (m.vertices[t[0] as usize], m.vertices[t[1] as usize], m.vertices[t[2] as usize]);
            vol += a.dot(b.cross(c)) / 6.0;
        }
        let torus = 2.0 * std::f64::consts::PI.powi(2) * 10.0;
        assert!(vol > 0.6 * torus && vol < torus, "{vol} vs {torus}");
        for v in &m.vertices {
            assert!(v.y >= 1.5 - 1e-9);
        }
    }
}
