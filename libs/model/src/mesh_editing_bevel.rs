//! Rounded multi-segment bevel of every edge sharper than an angle, on any
//! closed polygon mesh, plus crease marking for subdivision. Faces shrink in
//! their own planes along beveled edges, each beveled edge becomes a strip of
//! `segments` rows following a quadratic profile, and every vertex touching a
//! bevel gets a patch closing the corner. Faces keep flat shading; strips and
//! patches carry interpolated normals, so big flat faces never pick up a
//! smoothing gradient. The object is rebuilt: element IDs are new.
use crate::{mesh::*, transform::*, Error, Result};
use std::collections::{BTreeMap, BTreeSet};

type Key = (VertexId, VertexId);
fn key(a: VertexId, b: VertexId) -> Key { if a < b { (a, b) } else { (b, a) } }

struct FaceInfo { material: u32, verts: Vec<VertexId>, uvs: Vec<[f64; 2]>, normals: Vec<Option<[f64; 3]>>, normal: [f64; 3] }

fn newell(points: &[[f64; 3]]) -> [f64; 3] {
    let mut n = [0.; 3];
    for i in 0..points.len() {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        n = add(n, [(a[1] - b[1]) * (a[2] + b[2]), (a[2] - b[2]) * (a[0] + b[0]), (a[0] - b[0]) * (a[1] + b[1])]);
    }
    normalized(n).unwrap_or([0., 1., 0.])
}
fn slerp_n(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] { normalized(add(mul(a, 1. - t), mul(b, t))).unwrap_or(a) }
fn bezier(a: [f64; 3], c: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] { add(add(mul(a, (1. - t) * (1. - t)), mul(c, 2. * t * (1. - t))), mul(b, t * t)) }
fn closest_on_line(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    let d = sub(b, a); let l = dot(d, d);
    if l <= 1e-18 { a } else { add(a, mul(d, dot(sub(p, a), d) / l)) }
}
fn plane_uv(p: [f64; 3], n: [f64; 3]) -> [f64; 2] {
    let e1 = if n[1].abs() > 0.9 { [1., 0., 0.] } else { normalized(cross([0., 1., 0.], n)).unwrap_or([1., 0., 0.]) };
    let e2 = cross(n, e1);
    [dot(p, e1), dot(p, e2)]
}

/// Edges whose face normals differ by more than `angle` radians.
fn sharp_edges(faces: &[FaceInfo], pos: &BTreeMap<VertexId, [f64; 3]>, angle: f64) -> (BTreeMap<Key, Vec<(usize, usize)>>, BTreeSet<Key>) {
    let _ = pos;
    let mut uses: BTreeMap<Key, Vec<(usize, usize)>> = BTreeMap::new();
    for (f, face) in faces.iter().enumerate() { for i in 0..face.verts.len() { uses.entry(key(face.verts[i], face.verts[(i + 1) % face.verts.len()])).or_default().push((f, i)); } }
    let cos = angle.cos();
    let sharp = uses.iter().filter(|(_, u)| u.len() == 2 && dot(faces[u[0].0].normal, faces[u[1].0].normal) < cos).map(|(k, _)| *k).collect();
    (uses, sharp)
}
fn read_faces(mesh: &Mesh) -> Result<(Vec<FaceInfo>, BTreeMap<VertexId, [f64; 3]>)> {
    let pos: BTreeMap<VertexId, [f64; 3]> = mesh.vertices().iter().map(|v| (v.id, v.position)).collect();
    let mut faces = Vec::new();
    for face in mesh.faces() {
        let cs = mesh.face_corners(face.id)?;
        let verts: Vec<VertexId> = cs.iter().map(|c| c.vertex).collect();
        let normal = newell(&verts.iter().map(|v| pos[v]).collect::<Vec<_>>());
        faces.push(FaceInfo { material: face.material, uvs: cs.iter().map(|c| c.uv).collect(), normals: cs.iter().map(|c| c.normal).collect(), verts, normal });
    }
    Ok((faces, pos))
}

/// Marks every edge sharper than `angle` with subdivision crease `weight`.
pub(crate) fn crease(mesh: &mut Mesh, angle: f64, weight: f64, ctx: &mut Context<'_>) -> Result<ChangeSet> {
    if !(0. ..=1.).contains(&weight) { return Err(Error::Invalid("crease weight must be 0..1")); }
    let (faces, pos) = read_faces(mesh)?;
    let (_, sharp) = sharp_edges(&faces, &pos, angle);
    let mut changes = ChangeSet::default();
    for (a, b) in sharp {
        ctx.checkpoint(1)?;
        let seam = mesh.edge_attributes().get(&EdgeKey::new(a, b)).is_some_and(|d| d.attributes.seam);
        let c = mesh.set_edge_attributes(EdgeKey::new(a, b), EdgeAttributes { seam, crease: weight }, ctx)?;
        changes.retained.extend(c.retained);
    }
    Ok(changes)
}

pub(crate) fn bevel(mesh: &Mesh, angle: f64, width: f64, segments: u32, ctx: &mut Context<'_>) -> Result<Mesh> {
    if !(width.is_finite() && width > 0.) || !(1..=16).contains(&segments) || !(angle.is_finite() && angle >= 0.) { return Err(Error::Invalid("bevel needs width > 0, segments 1..16, angle >= 0")); }
    let (faces, pos) = read_faces(mesh)?;
    let weights: BTreeMap<VertexId, Vec<JointWeight>> = mesh.vertices().iter().map(|v| (v.id, v.weights.clone())).collect();
    let (uses, sharp) = sharp_edges(&faces, &pos, angle);
    if sharp.is_empty() { return Ok(mesh.clone()); }
    let len = |k: &Key| length(sub(pos[&k.0], pos[&k.1]));
    let mut touched: BTreeSet<VertexId> = BTreeSet::new();
    for k in &sharp { touched.insert(k.0); touched.insert(k.1); }
    // Never let neighbouring bevels meet: clamp to the shortest edge nearby.
    let shortest = uses.keys().filter(|k| touched.contains(&k.0) || touched.contains(&k.1)).map(len).fold(f64::INFINITY, f64::min);
    let w = width.min(shortest * 0.45);
    // Corner fans around every touched vertex, in face order.
    let mut corner_of: BTreeMap<(usize, VertexId), usize> = BTreeMap::new();
    for (f, face) in faces.iter().enumerate() { for (i, v) in face.verts.iter().enumerate() { corner_of.insert((f, *v), i); } }
    let other_face = |f: usize, k: Key| -> Option<usize> { uses.get(&k).filter(|u| u.len() == 2).map(|u| if u[0].0 == f { u[1].0 } else { u[0].0 }) };
    // Slide distance along an unbeveled edge at a vertex.
    let mut slide: BTreeMap<(Key, VertexId), f64> = BTreeMap::new();
    for face in &faces {
        let n = face.verts.len();
        for i in 0..n {
            ctx.checkpoint(1)?;
            let v = face.verts[i];
            if !touched.contains(&v) { continue; }
            let (p, q) = (face.verts[(i + n - 1) % n], face.verts[(i + 1) % n]);
            let (ein, eout) = (key(p, v), key(v, q));
            for (e, other, far, far_other) in [(ein, eout, p, q), (eout, ein, q, p)] {
                if sharp.contains(&e) { continue; }
                let t = if sharp.contains(&other) {
                    let a = normalized(sub(pos[&far], pos[&v])).unwrap_or([1., 0., 0.]);
                    let b = normalized(sub(pos[&far_other], pos[&v])).unwrap_or([0., 0., 1.]);
                    w / length(cross(a, b)).max(0.2)
                } else { w };
                let t = t.min(len(&e) * 0.45);
                let slot = slide.entry((e, v)).or_insert(0.);
                *slot = slot.max(t);
            }
        }
    }
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut vweights: Vec<Vec<JointWeight>> = Vec::new();
    let mut ids: BTreeMap<VertexId, u32> = BTreeMap::new();
    let new_point = |p: [f64; 3], from: VertexId, positions: &mut Vec<[f64; 3]>, vweights: &mut Vec<Vec<JointWeight>>| -> u32 { positions.push(p); vweights.push(weights[&from].clone()); (positions.len() - 1) as u32 };
    let mut original = |v: VertexId, positions: &mut Vec<[f64; 3]>, vweights: &mut Vec<Vec<JointWeight>>| -> u32 {
        *ids.entry(v).or_insert_with(|| { positions.push(pos[&v]); vweights.push(weights[&v].clone()); (positions.len() - 1) as u32 })
    };
    let mut slide_ids: BTreeMap<(Key, VertexId), u32> = BTreeMap::new();
    // Replacement points of each face corner: (in-side, out-side).
    let mut repl: BTreeMap<(usize, usize), (u32, u32)> = BTreeMap::new();
    for (f, face) in faces.iter().enumerate() {
        let n = face.verts.len();
        for i in 0..n {
            let v = face.verts[i];
            if !touched.contains(&v) { continue; }
            let (p, q) = (face.verts[(i + n - 1) % n], face.verts[(i + 1) % n]);
            let (ein, eout) = (key(p, v), key(v, q));
            let mut slide_point = |e: Key, far: VertexId, positions: &mut Vec<[f64; 3]>, vweights: &mut Vec<Vec<JointWeight>>| -> u32 {
                if let Some(id) = slide_ids.get(&(e, v)) { return *id; }
                let t = slide[&(e, v)];
                let d = normalized(sub(pos[&far], pos[&v])).unwrap_or([0.; 3]);
                let id = new_point(add(pos[&v], mul(d, t)), v, positions, vweights);
                slide_ids.insert((e, v), id);
                id
            };
            let (sin, sout) = (sharp.contains(&ein), sharp.contains(&eout));
            let r = if sin && sout {
                // Intersection of both edges' inward offset lines in the face plane.
                let (a, b, c) = (pos[&p], pos[&v], pos[&q]);
                let din = normalized(cross(face.normal, sub(b, a))).unwrap_or([0.; 3]);
                let dout = normalized(cross(face.normal, sub(c, b))).unwrap_or([0.; 3]);
                let (o1, u1) = (add(a, mul(din, w)), sub(b, a));
                let (o2, u2) = (add(b, mul(dout, w)), sub(c, b));
                let nn = cross(u1, u2); let den = dot(nn, nn);
                let point = if den < 1e-18 { add(b, mul(din, w)) } else { add(o1, mul(u1, dot(cross(sub(o2, o1), u2), nn) / den)) };
                let point = if length(sub(point, b)) > w * 6. { add(b, mul(normalized(sub(point, b)).unwrap_or(din), w * 6.)) } else { point };
                let id = new_point(point, v, &mut positions, &mut vweights);
                (id, id)
            } else if sin {
                let id = slide_point(eout, q, &mut positions, &mut vweights); (id, id)
            } else if sout {
                let id = slide_point(ein, p, &mut positions, &mut vweights); (id, id)
            } else {
                (slide_point(ein, p, &mut positions, &mut vweights), slide_point(eout, q, &mut positions, &mut vweights))
            };
            repl.insert((f, i), r);
        }
    }
    let mut polygons: Vec<Polygon> = Vec::new();
    let mut corner_normals: Vec<Vec<Option<[f64; 3]>>> = Vec::new();
    // Shrunken faces, UVs by the face's own affine map.
    for (f, face) in faces.iter().enumerate() {
        ctx.checkpoint(face.verts.len() as u64)?;
        let n = face.verts.len();
        let pts: Vec<[f64; 3]> = face.verts.iter().map(|v| pos[v]).collect();
        // Three spread corners anchor the affine UV map.
        let (mut i1, mut i2, mut best) = (1usize, 2usize.min(n - 1), 0.);
        for a in 1..n { for b in a + 1..n { let area = length(cross(sub(pts[a], pts[0]), sub(pts[b], pts[0]))); if area > best { best = area; i1 = a; i2 = b; } } }
        let (e1, e2) = (sub(pts[i1], pts[0]), sub(pts[i2], pts[0]));
        let (g11, g12, g22) = (dot(e1, e1), dot(e1, e2), dot(e2, e2)); let det = g11 * g22 - g12 * g12;
        let uv_at = |p: [f64; 3]| -> [f64; 2] {
            if det.abs() < 1e-18 { return face.uvs[0]; }
            let d = sub(p, pts[0]); let (b1, b2) = (dot(d, e1), dot(d, e2));
            let (s, t) = ((b1 * g22 - b2 * g12) / det, (b2 * g11 - b1 * g12) / det);
            std::array::from_fn(|k| face.uvs[0][k] + s * (face.uvs[i1][k] - face.uvs[0][k]) + t * (face.uvs[i2][k] - face.uvs[0][k]))
        };
        let (mut vs, mut uvs, mut ns): (Vec<u32>, Vec<[f64; 2]>, Vec<Option<[f64; 3]>>) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n {
            let v = face.verts[i];
            let list = match repl.get(&(f, i)) { Some(&(a, b)) if a == b => vec![a], Some(&(a, b)) => vec![a, b], None => vec![original(v, &mut positions, &mut vweights)] };
            for id in list {
                if vs.last() == Some(&id) || (vs.first() == Some(&id) && i + 1 == n) { continue; }
                vs.push(id);
                if repl.contains_key(&(f, i)) { uvs.push(uv_at(positions[id as usize])); ns.push(None); } else { uvs.push(face.uvs[i]); ns.push(face.normals[i]); }
            }
        }
        if vs.len() < 3 { return Err(Error::Invalid("bevel consumed a whole face; use a smaller width")); }
        polygons.push(Polygon { vertices: vs, uvs, material: face.material });
        corner_normals.push(ns);
    }
    // Strips along beveled edges; arc points keyed by (edge, endpoint) from the
    // face that runs the edge a->b (A) toward the other face (B).
    let seg = segments as usize;
    let mut arcs: BTreeMap<(Key, VertexId), (usize, Vec<u32>)> = BTreeMap::new();
    for k in &sharp {
        let u = &uses[k];
        let (fa, ia) = u[0];
        let na = faces[fa].verts.len();
        let (a, b) = (faces[fa].verts[ia], faces[fa].verts[(ia + 1) % na]);
        let fb = u[1].0;
        let side = |f: usize, v: VertexId, out: bool| -> u32 { let i = corner_of[&(f, v)]; let r = repl[&(f, i)]; if out { r.1 } else { r.0 } };
        // In A the edge leaves a (out side) and enters b (in side); in B the reverse.
        let (pa_a, pa_b, pb_a, pb_b) = (side(fa, a, true), side(fa, b, false), side(fb, a, false), side(fb, b, true));
        let (nfa, nfb) = (faces[fa].normal, faces[fb].normal);
        let arc = |x: VertexId, from: u32, to: u32, positions: &mut Vec<[f64; 3]>, vweights: &mut Vec<Vec<JointWeight>>| -> Vec<u32> {
            let (p0, p1) = (positions[from as usize], positions[to as usize]);
            let c = closest_on_line(mul(add(p0, p1), 0.5), pos[&k.0], pos[&k.1]);
            let mut out = vec![from];
            for s in 1..seg { out.push(new_point(bezier(p0, c, p1, s as f64 / seg as f64), x, positions, vweights)); }
            out.push(to);
            out
        };
        let arc_a = arc(a, pa_a, pb_a, &mut positions, &mut vweights);
        let arc_b = arc(b, pa_b, pb_b, &mut positions, &mut vweights);
        for s in 0..seg {
            let quad = vec![arc_b[s], arc_a[s], arc_a[s + 1], arc_b[s + 1]];
            let (t0, t1) = (s as f64 / seg as f64, (s + 1) as f64 / seg as f64);
            let normal = newell(&quad.iter().map(|&i| positions[i as usize]).collect::<Vec<_>>());
            let uvs = quad.iter().map(|&i| plane_uv(positions[i as usize], normal)).collect();
            polygons.push(Polygon { vertices: quad, uvs, material: faces[fa].material });
            corner_normals.push(vec![Some(slerp_n(nfa, nfb, t0)), Some(slerp_n(nfa, nfb, t0)), Some(slerp_n(nfa, nfb, t1)), Some(slerp_n(nfa, nfb, t1))]);
        }
        arcs.insert((*k, a), (fa, arc_a));
        arcs.insert((*k, b), (fa, arc_b));
    }
    // Corner patches.
    let mut normal_of: BTreeMap<u32, [f64; 3]> = BTreeMap::new();
    for (poly, ns) in polygons.iter().zip(&corner_normals) { for (v, n) in poly.vertices.iter().zip(ns) { if let Some(n) = n { normal_of.entry(*v).or_insert(*n); } } }
    for &v in &touched {
        ctx.checkpoint(1)?;
        let start = corner_of.keys().find(|(_, x)| *x == v).map(|(f, _)| *f).ok_or(Error::Invalid("bevel vertex without faces"))?;
        let mut seq: Vec<u32> = Vec::new(); let mut normals_sum = [0.; 3];
        let mut f = start; let mut guard = 0;
        loop {
            guard += 1; if guard > 256 { return Err(Error::Invalid("bevel vertex fan too large")); }
            let face = &faces[f]; let n = face.verts.len(); let i = corner_of[&(f, v)];
            normals_sum = add(normals_sum, face.normal);
            let (a, b) = repl[&(f, i)];
            seq.push(a); seq.push(b);
            let q = face.verts[(i + 1) % n];
            let e = key(v, q);
            let Some(g) = other_face(f, e) else { return Err(Error::Invalid("bevel needs a closed surface around beveled vertices")) };
            if let Some((fa, arc)) = arcs.get(&(e, v)) {
                let inner: Vec<u32> = if *fa == f { arc[1..arc.len() - 1].to_vec() } else { arc[1..arc.len() - 1].iter().rev().copied().collect() };
                seq.extend(inner);
            }
            f = g;
            if f == start { break; }
        }
        seq.dedup();
        while seq.len() > 1 && seq.first() == seq.last() { seq.pop(); }
        let mut unique = seq.clone(); unique.sort(); unique.dedup();
        if unique.len() < 3 { continue; }
        seq.reverse();
        let avg_n = normalized(normals_sum).unwrap_or([0., 1., 0.]);
        let ring: Vec<[f64; 3]> = seq.iter().map(|&i| positions[i as usize]).collect();
        if seq.len() == 3 {
            let normal = newell(&ring);
            polygons.push(Polygon { uvs: ring.iter().map(|p| plane_uv(*p, normal)).collect(), vertices: seq.clone(), material: faces[start].material });
            corner_normals.push(seq.iter().map(|i| Some(*normal_of.get(i).unwrap_or(&avg_n))).collect());
            continue;
        }
        // Fan from a centre raised toward the original corner.
        let avg = mul(ring.iter().fold([0.; 3], |s, p| add(s, *p)), 1. / ring.len() as f64);
        let centre = add(mul(avg, 0.6), mul(pos[&v], 0.4));
        let c = new_point(centre, v, &mut positions, &mut vweights);
        for j in 0..seq.len() {
            let tri = vec![seq[j], seq[(j + 1) % seq.len()], c];
            let pts: Vec<[f64; 3]> = tri.iter().map(|&i| positions[i as usize]).collect();
            let normal = newell(&pts);
            polygons.push(Polygon { uvs: pts.iter().map(|p| plane_uv(*p, normal)).collect(), vertices: tri.clone(), material: faces[start].material });
            corner_normals.push(vec![Some(*normal_of.get(&tri[0]).unwrap_or(&avg_n)), Some(*normal_of.get(&tri[1]).unwrap_or(&avg_n)), Some(avg_n)]);
        }
    }
    let mut out = Mesh::from_weighted_polygons(&positions, &vweights, &polygons, ctx)?;
    let values: Vec<(CornerId, Option<[f64; 3]>)> = out.faces().iter().zip(&corner_normals).flat_map(|(face, ns)| {
        out.face_corners(face.id).map(|cs| cs.iter().zip(ns).filter_map(|(c, n)| n.map(|n| (c.id, Some(n)))).collect::<Vec<_>>()).unwrap_or_default()
    }).collect();
    if !values.is_empty() { out.set_corner_normals_bulk(&values, ctx)?; }
    Ok(out)
}
