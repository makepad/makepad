//! Baked weathering: ambient occlusion from the whole model and convex-edge
//! wear, written as vertex colours that multiply every material. Crevices
//! darken, flat panels dim slightly and bevelled edges stay bright, which is
//! what makes hard-surface shapes read at game distance. One colour
//! operation per object.
use makepad_csg_math::portable::PortableFloat;
use crate::{transform::*, Workspace, Operation, Result, SurfaceOperation};
use std::collections::BTreeMap;

fn poll(cancelled: Option<&dyn Fn() -> bool>) -> Result<()> {
    if cancelled.is_some_and(|c| c()) { return Err(crate::mesh::MeshError::Cancelled.into()); }
    Ok(())
}

struct Bvh {
    #[cfg(test)]
    original_tris: Vec<[[f64; 3]; 3]>,
    tris: Vec<[[f64; 3]; 3]>, nodes: Vec<([f64; 3], [f64; 3], usize, usize, Option<(usize, usize)>)>, order: Vec<usize> }
impl Bvh {
    fn new(tris: Vec<[[f64; 3]; 3]>, cancelled: Option<&dyn Fn() -> bool>) -> Result<Self> {
        let mut order = Vec::with_capacity(tris.len());
        for start in (0..tris.len()).step_by(1024) {
            poll(cancelled)?;
            order.extend(start..(start + 1024).min(tris.len()));
        }
        let mut b = Bvh {
            #[cfg(test)]
            original_tris: tris.clone(),
            order, tris, nodes: Vec::new()
        };
        if !b.tris.is_empty() { b.build(0, b.tris.len(), cancelled)?; }
        // Bounds and splitting use original vertices. Queries only need the
        // origin and two edges, so reuse the same storage after building.
        for chunk in b.tris.chunks_mut(512) {
            poll(cancelled)?;
            for p in chunk { *p = [p[0], sub(p[1], p[0]), sub(p[2], p[0])]; }
        }
        Ok(b)
    }
    fn build(&mut self, start: usize, end: usize, cancelled: Option<&dyn Fn() -> bool>) -> Result<usize> {
        let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        let index = self.nodes.len();
        if index % 64 == 0 { poll(cancelled)?; }
        for (block, indices) in self.order[start..end].chunks(256).enumerate() {
            if block != 0 { poll(cancelled)?; }
            for &i in indices { for p in self.tris[i] { for d in 0..3 { lo[d] = lo[d].min(p[d]); hi[d] = hi[d].max(p[d]); } } }
        }
        self.nodes.push((lo, hi, start, end, None));
        if end - start > 6 {
            let axis = (0..3).max_by(|a, b| (hi[*a] - lo[*a]).total_cmp(&(hi[*b] - lo[*b]))).unwrap();
            let tris = &self.tris;
            let mut comparisons = 0usize;
            let mut stopped = false;
            self.order[start..end].sort_by(|a, b| {
                comparisons += 1;
                // The stable sort needs its original ordering through the last comparison.
                if comparisons % 512 == 0 { stopped |= cancelled.is_some_and(|c| c()); }
                (tris[*a][0][axis] + tris[*a][1][axis] + tris[*a][2][axis]).total_cmp(&(tris[*b][0][axis] + tris[*b][1][axis] + tris[*b][2][axis]))
            });
            if stopped { return Err(crate::mesh::MeshError::Cancelled.into()); }
            let mid = (start + end) / 2;
            let (a, b) = (self.build(start, mid, cancelled)?, self.build(mid, end, cancelled)?);
            self.nodes[index].4 = Some((a, b));
        }
        Ok(index)
    }
    // Keep the slab divisions: multiplying by a rounded reciprocal can change
    // boundary hits. Return the entry distance to order children without testing
    // their bounds again when they are popped.
    fn entry(&self, node: usize, o: [f64; 3], d: [f64; 3], max: f64) -> Option<f64> {
        let (lo, hi, ..) = self.nodes[node];
        let (mut near, mut far) = (0f64, max);
        for c in 0..3 {
            if d[c].abs() < 1e-12 { if o[c] < lo[c] || o[c] > hi[c] { return None; } continue; }
            let (a, b) = ((lo[c] - o[c]) / d[c], (hi[c] - o[c]) / d[c]);
            near = near.max(a.min(b)); far = far.min(a.max(b));
        }
        if far < near { None } else { Some(near) }
    }
    fn hit(&self, o: [f64; 3], d: [f64; 3], max: f64, stack: &mut Vec<usize>, cancelled: Option<&dyn Fn() -> bool>) -> Result<bool> {
        // Every stacked node has already passed its bounds test.
        stack.clear();
        if self.nodes.is_empty() || self.entry(0, o, d, max).is_none() { return Ok(false); }
        stack.push(0);
        let mut visited = 0usize;
        while let Some(n) = stack.pop() {
            visited += 1;
            if visited % 256 == 0 { poll(cancelled)?; }
            let (_, _, s, e, kids) = self.nodes[n];
            match kids {
                Some((a, b)) => match (self.entry(a, o, d, max), self.entry(b, o, d, max)) {
                    (Some(near_a), Some(near_b)) => {
                        if near_a < near_b { stack.push(b); stack.push(a); }
                        else { stack.push(a); stack.push(b); }
                    }
                    (Some(_), None) => stack.push(a),
                    (None, Some(_)) => stack.push(b),
                    (None, None) => {}
                },
                None => for &i in &self.order[s..e] {
                    let p = self.tris[i];
                    let (e1, e2) = (p[1], p[2]);
                    let h = cross(d, e2); let det = dot(e1, h);
                    if det.abs() < 1e-14 { continue; }
                    let sv = sub(o, p[0]); let u = dot(sv, h) / det;
                    if !(0. ..=1.).contains(&u) { continue; }
                    let q = cross(sv, e1); let v = dot(d, q) / det;
                    if v < 0. || u + v > 1. { continue; }
                    let t = dot(e2, q) / det;
                    if t > 1e-5 && t < max { return Ok(true); }
                }
            }
        }
        Ok(false)
    }
    #[cfg(test)]
    fn hit_reference(&self, o: [f64; 3], d: [f64; 3], max: f64, stack: &mut Vec<usize>) -> bool {
        // A hit can return with unvisited nodes still on the stack.
        stack.clear();
        if self.nodes.is_empty() { return false; }
        stack.push(0);
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
                    let p = self.original_tris[i];
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
pub(crate) fn weathering_ops(doc: &Workspace, objects: Option<&[String]>, ao: f64, distance: f64, edges: f64, samples: usize,
    timing: &mut crate::WeatheringTiming, cancelled: Option<&dyn Fn() -> bool>) -> Result<Vec<Operation>> {
    weathering_ops_with_hit(doc, objects, ao, distance, edges, samples, timing, cancelled,
        |bvh, o, d, max, stack| bvh.hit(o, d, max, stack, cancelled))
}

fn weathering_ops_with_hit(doc: &Workspace, objects: Option<&[String]>, ao: f64, distance: f64, edges: f64, samples: usize,
    timing: &mut crate::WeatheringTiming, cancelled: Option<&dyn Fn() -> bool>,
    mut hit: impl FnMut(&Bvh, [f64; 3], [f64; 3], f64, &mut Vec<usize>) -> Result<bool>) -> Result<Vec<Operation>> {
    let setup = super::ProgramInstant::now();
    let mut tris = Vec::new();
    let mut per_object: Vec<(String, BTreeMap<crate::mesh::VertexId, [f64; 3]>, BTreeMap<crate::mesh::VertexId, Vec<crate::mesh::VertexId>>, BTreeMap<crate::mesh::VertexId, [f64; 3]>)> = Vec::new();
    for (name, mesh) in doc.objects() {
        poll(cancelled)?;
        if name.starts_with("__") { continue; }
        let selected = objects.is_none_or(|list| list.iter().any(|o| o == name));
        if !selected && ao == 0. { continue; }
        let world = doc.scene().world_matrix(name).unwrap_or(IDENTITY_MATRIX);
        let mut pos = BTreeMap::new();
        for vertices in mesh.vertices().chunks(256) {
            poll(cancelled)?;
            pos.extend(vertices.iter().map(|v| (v.id, transform_point(world, v.position))));
        }
        let mut normal: BTreeMap<_, [f64; 3]> = BTreeMap::new();
        let mut neighbours: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for (face_index, face) in mesh.faces().iter().enumerate() {
            if face_index % 64 == 0 { poll(cancelled)?; }
            let Ok(cs) = mesh.face_corners(face.id) else { continue };
            let mut n = [0.; 3];
            if cs.len() >= 3 {
                let first = pos[&cs[0].vertex];
                let mut prev = pos[&cs[1].vertex];
                for (i, c) in cs[2..].iter().enumerate() {
                    if i % 256 == 255 { poll(cancelled)?; }
                    let next = pos[&c.vertex];
                    n = add(n, cross(sub(prev, first), sub(next, first)));
                    if ao != 0. { tris.push([first, prev, next]); }
                    prev = next;
                }
            }
            // All objects occlude; only painted objects need vertex adjacency.
            if !selected { continue; }
            for (i, c) in cs.iter().enumerate() {
                if i % 256 == 255 { poll(cancelled)?; }
                let e = normal.entry(c.vertex).or_insert([0.; 3]); *e = add(*e, n);
                let list = neighbours.entry(c.vertex).or_default();
                for other in [cs[(i + 1) % cs.len()].vertex, cs[(i + cs.len() - 1) % cs.len()].vertex] { if !list.contains(&other) { list.push(other); } }
            }
        }
        if !selected { continue; }
        per_object.push((name.to_string(), pos, neighbours, normal));
    }
    timing.setup += setup.elapsed();
    let acceleration = super::ProgramInstant::now();
    let bvh = Bvh::new(tris, cancelled)?;
    timing.acceleration += acceleration.elapsed();
    let queries = super::ProgramInstant::now();
    let golden = std::f64::consts::PI * (3. - 5f64.sqrt());
    // These coefficients depend only on the sample index/count. Keep their
    // arithmetic and the per-vertex vector addition order unchanged.
    let directions: Vec<[f64; 3]> = (0..if ao == 0. { 0 } else { samples }).map(|k| {
        let r = ((k as f64 + 0.5) / samples as f64).sqrt(); let a = golden * k as f64;
        [r * a.pcos(), r * a.psin(), (1. - r * r).max(0.).sqrt()]
    }).collect();
    let mut stack = Vec::new();
    let mut ops = Vec::new();
    // A sampled vertex can trace 64 rays. Edge wear alone is much cheaper.
    let vertices_per_poll = if directions.is_empty() { 128 } else { (64 / directions.len()).max(1) };
    for (name, pos, neighbours, normal) in per_object {
        let (mut ids, mut colors) = (Vec::new(), Vec::new());
        for (i, (id, p)) in pos.iter().enumerate() {
            if i % vertices_per_poll == 0 { poll(cancelled)?; }
            timing.vertices += 1;
            let Ok(n) = normalized(*normal.get(id).unwrap_or(&[0., 1., 0.])) else { continue };
            timing.rays += directions.len();
            let (t1, t2) = { let a = if n[1].abs() < 0.9 { [0., 1., 0.] } else { [1., 0., 0.] }; let t = normalized(cross(a, n)).unwrap(); (t, cross(n, t)) };
            let origin = add(*p, mul(n, distance * 0.02));
            let mut blocked = 0usize;
            for &[x, y, z] in &directions {
                // Cosine-weighted directions on a spiral.
                let d = add(add(mul(t1, x), mul(t2, y)), mul(n, z));
                if hit(&bvh, origin, d, distance, &mut stack)? { blocked += 1; }
            }
            let occlusion = blocked as f64 / samples.max(1) as f64;
            // Convexity: how far neighbours fall below the tangent plane.
            let convex = if let Some(list) = neighbours.get(id) {
                let s = list.iter().filter_map(|o| pos.get(o)).enumerate().map(|(i, q)| {
                    if i % 256 == 255 { poll(cancelled)?; }
                    let d = sub(*p, *q); let l = length(d);
                    Ok(if l > 1e-9 { dot(n, d) / l } else { 0. })
                }).sum::<Result<f64>>()?;
                s / list.len().max(1) as f64
            } else { 0. };
            let wear = (convex * 4.).clamp(0., 1.);
            let value = ((1. - ao * occlusion.ppowf(1.3)) * (1. - edges * 0.14 * (1. - wear))).clamp(0.15, 1.);
            if value >= 0.995 { continue; }
            ids.push(*id);
            // Multiply any authored vertex tint (faces, lips, iris shading).
            let base = doc.surface().vertex_colors.get(&(name.clone(), *id)).copied().unwrap_or([1.; 4]);
            colors.push([base[0] * value, base[1] * value, base[2] * value, base[3]]);
        }
        if !ids.is_empty() {
            ops.push(Operation::Surface(SurfaceOperation::VertexColors { object: name, vertices: ids, colors }));
        }
    }
    timing.queries += queries.elapsed();
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traversal_preserves_boundary_hits_and_reuses_stack() {
        let tris = (0..8).map(|z| {
            let z = z as f64;
            [[0., 0., z], [1., 0., z], [0., 1., z]]
        }).collect();
        let bvh = Bvh::new(tris, None).unwrap();
        let mut stack = Vec::new();
        let mut reference_stack = Vec::new();
        // Vertices, edges, parallel axes, the near-parallel cutoff and both
        // strict distance boundaries. Alternate hits/misses on the same stack.
        for origin in [[0., 0., -1.], [0.5, 0.5, -1.], [1., 1., -1.], [0.25, 0.25, -1e-5], [0.25, 0.25, 0.]] {
            for direction in [[0., 0., 1.], [-0., 0., 1.], [1e-12, 0., 1.], [0., 0., -1.], [1., 0., 0.]] {
                for distance in [1e-5, 1., f64::from_bits(1f64.to_bits() + 1), 10.] {
                    assert_eq!(bvh.hit(origin, direction, distance, &mut stack, None).unwrap(),
                        bvh.hit_reference(origin, direction, distance, &mut reference_stack),
                        "origin={origin:?}, direction={direction:?}, distance={distance}");
                }
            }
        }
        assert!(bvh.hit([0., 0., -1.], [0., 0., 1.], 2., &mut stack, None).unwrap());
        assert!(!bvh.hit([0., 0., -1.], [0., 0., 1.], 1., &mut stack, None).unwrap());
        assert!(!Bvh::new(Vec::new(), None).unwrap().hit([0.; 3], [0., 0., 1.], 2., &mut stack, None).unwrap());
    }

    #[test]
    fn weathering_vertex_colors_match_reference_traversal() {
        let mut doc = Workspace::new(crate::Limits::default()).unwrap();
        doc.execute(&[
                Operation::Plane { object: "painted".into(), size: [2.; 2] },
                Operation::Cube { object: "occluder".into(), size: [3.; 3] },
        ], None).unwrap();
        let vertices: Vec<_> = doc.object("painted").unwrap().vertices().iter().map(|v| v.id).collect();
        doc.execute(&[Operation::Surface(SurfaceOperation::VertexColors {
                object: "painted".into(), colors: vec![[0.25, 0.5, 0.75, 0.6]; vertices.len()], vertices,
            })], None).unwrap();
        let objects = ["painted".to_string()];
        for samples in [0, 1, 17] {
            for distance in [0.1, 4.] {
                let mut timing = crate::WeatheringTiming::default();
                let actual = weathering_ops(&doc, Some(&objects), 0.7, distance, 1., samples, &mut timing, None).unwrap();
                let expected = weathering_ops_with_hit(&doc, Some(&objects), 0.7, distance, 1., samples,
                    &mut crate::WeatheringTiming::default(), None, |bvh, o, d, max, stack| Ok(bvh.hit_reference(o, d, max, stack))).unwrap();
                assert!(!actual.is_empty(), "flat surface must receive edge shading");
                assert_eq!(actual, expected);
                for (actual, expected) in actual.iter().zip(&expected) {
                    let (Operation::Surface(SurfaceOperation::VertexColors { colors: a, .. }),
                         Operation::Surface(SurfaceOperation::VertexColors { colors: b, .. })) = (actual, expected)
                        else { panic!("expected vertex colors") };
                    assert_eq!(a.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>(),
                        b.iter().flatten().map(|v| v.to_bits()).collect::<Vec<_>>());
                }
            }
        }
        let mut timing = crate::WeatheringTiming::default();
        let without_ao = weathering_ops_with_hit(&doc, Some(&objects), 0., 4., 1., 17, &mut timing, None,
            |_, _, _, _, _| panic!("zero AO must not trace rays")).unwrap();
        assert_eq!(timing.rays, 0);
        let Operation::Surface(SurfaceOperation::VertexColors { colors, .. }) = &without_ao[0]
            else { panic!("expected vertex colors") };
        for color in colors {
            assert_eq!(color.map(f64::to_bits), [0.25 * 0.86, 0.5 * 0.86, 0.75 * 0.86, 0.6].map(f64::to_bits));
        }
    }
}
