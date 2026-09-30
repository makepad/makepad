//! Baked weathering: ambient occlusion from the whole model and convex-edge
//! wear, written as vertex colours that multiply every material. Crevices
//! darken, flat panels dim slightly and bevelled edges stay bright, which is
//! what makes hard-surface shapes read at game distance. One colour
//! operation per object.
use makepad_csg_math::portable::PortableFloat;
use crate::{json::{self, Value}, transform::*, Document};
use std::collections::BTreeMap;

struct Bvh { tris: Vec<[[f64; 3]; 3]>, nodes: Vec<([f64; 3], [f64; 3], usize, usize, Option<(usize, usize)>)>, order: Vec<usize> }
impl Bvh {
    fn new(tris: Vec<[[f64; 3]; 3]>) -> Self {
        let mut b = Bvh { order: (0..tris.len()).collect(), tris, nodes: Vec::new() };
        if !b.tris.is_empty() { b.build(0, b.tris.len()); }
        b
    }
    fn build(&mut self, start: usize, end: usize) -> usize {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for &i in &self.order[start..end] { for p in self.tris[i] { for d in 0..3 { lo[d] = lo[d].min(p[d]); hi[d] = hi[d].max(p[d]); } } }
        let index = self.nodes.len();
        self.nodes.push((lo, hi, start, end, None));
        if end - start > 6 {
            let axis = (0..3).max_by(|a, b| (hi[*a] - lo[*a]).total_cmp(&(hi[*b] - lo[*b]))).unwrap();
            let tris = &self.tris;
            self.order[start..end].sort_by(|a, b| (tris[*a][0][axis] + tris[*a][1][axis] + tris[*a][2][axis]).total_cmp(&(tris[*b][0][axis] + tris[*b][1][axis] + tris[*b][2][axis])));
            let mid = (start + end) / 2;
            let (a, b) = (self.build(start, mid), self.build(mid, end));
            self.nodes[index].4 = Some((a, b));
        }
        index
    }
    fn hit(&self, o: [f64; 3], d: [f64; 3], max: f64) -> bool {
        if self.nodes.is_empty() { return false; }
        let mut stack = vec![0];
        while let Some(n) = stack.pop() {
            let (lo, hi, s, e, kids) = self.nodes[n];
            let (mut near, mut far) = (0f64, max);
            for c in 0..3 {
                if d[c].abs() < 1e-12 { if o[c] < lo[c] || o[c] > hi[c] { far = -1.; break; } continue; }
                let (a, b) = ((lo[c] - o[c]) / d[c], (hi[c] - o[c]) / d[c]);
                near = near.max(a.min(b)); far = far.min(a.max(b));
            }
            if far < near { continue; }
            match kids {
                Some((a, b)) => { stack.push(a); stack.push(b); }
                None => for &i in &self.order[s..e] {
                    let p = self.tris[i];
                    let (e1, e2) = (sub(p[1], p[0]), sub(p[2], p[0]));
                    let h = cross(d, e2); let det = dot(e1, h);
                    if det.abs() < 1e-14 { continue; }
                    let sv = sub(o, p[0]); let u = dot(sv, h) / det;
                    if !(0. ..=1.).contains(&u) { continue; }
                    let q = cross(sv, e1); let v = dot(d, q) / det;
                    if v < 0. || u + v > 1. { continue; }
                    let t = dot(e2, q) / det;
                    if t > 1e-5 && t < max { return true; }
                }
            }
        }
        false
    }
}

/// Vertex-paint operations for every (listed) object of `doc`.
pub(crate) fn weathering_ops(doc: &Document, objects: Option<&[String]>, ao: f64, distance: f64, edges: f64, samples: usize) -> Vec<Value> {
    let mut tris = Vec::new();
    let mut per_object: Vec<(String, Vec<(crate::mesh::VertexId, [f64; 3])>, BTreeMap<crate::mesh::VertexId, Vec<crate::mesh::VertexId>>, BTreeMap<crate::mesh::VertexId, [f64; 3]>)> = Vec::new();
    for (name, mesh) in doc.objects() {
        if name.starts_with("__") { continue; }
        let world = doc.scene().world_matrix(name).unwrap_or(IDENTITY_MATRIX);
        let pos: BTreeMap<_, _> = mesh.vertices().iter().map(|v| (v.id, transform_point(world, v.position))).collect();
        let mut normal: BTreeMap<_, [f64; 3]> = BTreeMap::new();
        let mut neighbours: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for face in mesh.faces() {
            let Ok(cs) = mesh.face_corners(face.id) else { continue };
            let pts: Vec<[f64; 3]> = cs.iter().map(|c| pos[&c.vertex]).collect();
            let mut n = [0.; 3];
            for i in 1..pts.len() - 1 { n = add(n, cross(sub(pts[i], pts[0]), sub(pts[i + 1], pts[0]))); tris.push([pts[0], pts[i], pts[i + 1]]); }
            for (i, c) in cs.iter().enumerate() {
                let e = normal.entry(c.vertex).or_insert([0.; 3]); *e = add(*e, n);
                let list = neighbours.entry(c.vertex).or_default();
                for other in [cs[(i + 1) % cs.len()].vertex, cs[(i + cs.len() - 1) % cs.len()].vertex] { if !list.contains(&other) { list.push(other); } }
            }
        }
        if objects.is_some_and(|list| !list.iter().any(|o| o == name)) { continue; }
        per_object.push((name.to_string(), pos.into_iter().collect(), neighbours, normal));
    }
    let bvh = Bvh::new(tris);
    let golden = std::f64::consts::PI * (3. - 5f64.sqrt());
    let mut ops = Vec::new();
    for (name, pos, neighbours, normal) in per_object {
        let lookup: BTreeMap<_, _> = pos.iter().copied().collect();
        let (mut ids, mut colors) = (Vec::new(), Vec::new());
        for (id, p) in &pos {
            let Ok(n) = normalized(*normal.get(id).unwrap_or(&[0., 1., 0.])) else { continue };
            let (t1, t2) = { let a = if n[1].abs() < 0.9 { [0., 1., 0.] } else { [1., 0., 0.] }; let t = normalized(cross(a, n)).unwrap(); (t, cross(n, t)) };
            let origin = add(*p, mul(n, distance * 0.02));
            let mut blocked = 0usize;
            for k in 0..samples {
                // Cosine-weighted directions on a spiral.
                let r = ((k as f64 + 0.5) / samples as f64).sqrt(); let a = golden * k as f64;
                let d = add(add(mul(t1, r * a.pcos()), mul(t2, r * a.psin())), mul(n, (1. - r * r).max(0.).sqrt()));
                if bvh.hit(origin, d, distance) { blocked += 1; }
            }
            let occlusion = blocked as f64 / samples.max(1) as f64;
            // Convexity: how far neighbours fall below the tangent plane.
            let convex = neighbours.get(id).map_or(0., |list| {
                let s: f64 = list.iter().filter_map(|o| lookup.get(o)).map(|q| { let d = sub(*p, *q); let l = length(d); if l > 1e-9 { dot(n, d) / l } else { 0. } }).sum();
                s / list.len().max(1) as f64
            });
            let wear = (convex * 4.).clamp(0., 1.);
            let value = ((1. - ao * occlusion.ppowf(1.3)) * (1. - edges * 0.14 * (1. - wear))).clamp(0.15, 1.);
            if value >= 0.995 { continue; }
            ids.push(json::s(id.0.to_string()));
            // Multiply any authored vertex tint (faces, lips, iris shading).
            let base = doc.surface().vertex_colors.get(&(name.clone(), *id)).copied().unwrap_or([1.; 4]);
            colors.push(Value::Arr(vec![Value::F64(base[0] * value), Value::F64(base[1] * value), Value::F64(base[2] * value), Value::F64(base[3])]));
        }
        if !ids.is_empty() {
            ops.push(json::obj(vec![("op", json::s("surface_vertex_colors")), ("object", json::s(&name)), ("vertices", Value::Arr(ids)), ("colors", Value::Arr(colors))]));
        }
    }
    ops
}
