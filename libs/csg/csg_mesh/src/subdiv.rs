//! Catmull-Clark subdivision of a closed cage of polygons, down to its limit surface.
//!
//! A step (Catmull and Clark, 1978) puts a face point at each face's centroid, an edge point at
//! the average of each edge's two ends and its two faces' points, and moves each vertex to
//! (Q + 2R + (n - 3) S) / n: Q the average of the points of its n faces, R of the midpoints of its
//! n edges, S itself. A face of k sides becomes k quads (its corner, the next edge's point, its
//! face point, the last edge's point), so after one step every face is a quad.
//!
//! On a quad mesh the limit surface's point under a vertex v of valence n is
//! (n² v + 4 Σ e_i + Σ f_i) / (n (n + 5)), with e_i its neighbours along its edges in turn round
//! it and f_i the corner across the quad between e_i and e_i+1. Its tangents are
//! Σ A c_i e_i + (c_i + c_i+1) f_i and the same with every c_i one back, c_i = cos(2πi / n) and
//! A = 1 + cos(2π / n) + cos(π / n) sqrt(2 (9 + cos(2π / n))); the normal is their cross product
//! (Halstead, Kass and DeRose, 1993). The mesh is the last step's quads with every vertex on the
//! limit surface, each quad split along its shorter diagonal, with the limit normals at the
//! corners.
//!
//! A cage is closed: every edge lies between exactly two faces that run along it in opposite
//! directions (so all face the same way: listed counter-clockwise seen from outside), and the
//! faces round each point make one fan; every point is a corner of some face.

use crate::TriMesh;
use makepad_csg_math::Vec3d;
use std::collections::HashMap;
use std::f64::consts::PI;

/// The most steps.
pub const MAX_LEVELS: u32 = 6;

/// The most quads the last step may make.
pub const MAX_QUADS: usize = 2_000_000;

/// How many steps to take.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Levels {
    /// This many (1 to [`MAX_LEVELS`]).
    Steps(u32),
    /// As many as bring every point of the last step within this distance of its limit point
    /// (1 to [`MAX_LEVELS`], and no more than make [`MAX_QUADS`]): about how far the mesh is
    /// from the limit surface.
    Chord(f64),
}

/// Polygons over points: face f is `idx[start[f]..start[f + 1]]`.
struct Polys {
    verts: Vec<Vec3d>,
    start: Vec<u32>,
    idx: Vec<u32>,
}

impl Polys {
    fn faces(&self) -> usize {
        self.start.len() - 1
    }

    fn face(&self, f: usize) -> &[u32] {
        &self.idx[self.start[f] as usize..self.start[f + 1] as usize]
    }
}

/// The edges of `p`: each one's ends and its two faces, and each face's edges in its order (the
/// edge from its corner i to corner i + 1).
struct Edges {
    ends: Vec<[u32; 2]>,
    faces: Vec<[u32; 2]>,
    of_face: Vec<u32>,
}

fn edges(p: &Polys) -> Edges {
    let mut at: HashMap<(u32, u32), u32> = HashMap::with_capacity(p.idx.len() / 2);
    let mut e = Edges { ends: Vec::with_capacity(p.idx.len() / 2), faces: Vec::with_capacity(p.idx.len() / 2), of_face: Vec::with_capacity(p.idx.len()) };
    for f in 0..p.faces() {
        let c = p.face(f);
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            let key = (a.min(b), a.max(b));
            let k = *at.entry(key).or_insert_with(|| {
                e.ends.push([key.0, key.1]);
                e.faces.push([f as u32, u32::MAX]);
                (e.ends.len() - 1) as u32
            });
            if e.faces[k as usize][0] != f as u32 {
                e.faces[k as usize][1] = f as u32;
            }
            e.of_face.push(k);
        }
    }
    e
}

/// The cage as polygons, or what is wrong with it.
fn cage(points: &[Vec3d], faces: &[Vec<u32>]) -> Result<Polys, String> {
    if faces.is_empty() {
        return Err("a cage needs faces".into());
    }
    let mut p = Polys { verts: points.to_vec(), start: vec![0], idx: Vec::new() };
    for (f, c) in faces.iter().enumerate() {
        if c.len() < 3 {
            return Err(format!("face {f} has {} points: a face has 3 or more", c.len()));
        }
        for (i, &v) in c.iter().enumerate() {
            if v as usize >= points.len() {
                return Err(format!("face {f} names point {v}, but there are {} points (0 to {})", points.len(), points.len().saturating_sub(1)));
            }
            if c[..i].contains(&v) {
                return Err(format!("face {f} names point {v} twice"));
            }
        }
        p.idx.extend_from_slice(c);
        p.start.push(p.idx.len() as u32);
    }
    // every edge between two faces that run along it opposite ways
    let mut runs: HashMap<(u32, u32), Vec<(u32, bool)>> = HashMap::new();
    for f in 0..p.faces() {
        let c = p.face(f);
        for i in 0..c.len() {
            let (a, b) = (c[i], c[(i + 1) % c.len()]);
            runs.entry((a.min(b), a.max(b))).or_default().push((f as u32, a < b));
        }
    }
    let mut keys: Vec<&(u32, u32)> = runs.keys().collect();
    keys.sort();
    for key in keys {
        let r = &runs[key];
        let (a, b) = *key;
        match r.as_slice() {
            [_] => return Err(format!("edge {a}-{b} has only face {}: a cage is closed, every edge between two faces", r[0].0)),
            [x, y] if x.1 == y.1 => return Err(format!("faces {} and {} both run from point {} to point {}: they face opposite ways (list one's points the other way round)", x.0, y.0, if x.1 { a } else { b }, if x.1 { b } else { a })),
            [_, _] => {}
            _ => return Err(format!("edge {a}-{b} is in {} faces: a cage's edge is between two", r.len())),
        }
    }
    // the faces round each point: one fan
    let mut next: HashMap<(u32, u32), u32> = HashMap::new();
    let mut corners = vec![0u32; points.len()];
    for f in 0..p.faces() {
        let c = p.face(f);
        let k = c.len();
        for i in 0..k {
            // round c[i]: from the edge to the next corner on to the edge from the last one
            next.insert((c[i], c[(i + 1) % k]), c[(i + k - 1) % k]);
            corners[c[i] as usize] += 1;
        }
    }
    let mut seen = vec![false; points.len()];
    for &(v, first) in next.keys() {
        if seen[v as usize] {
            continue;
        }
        seen[v as usize] = true;
        let (mut e, mut n) = (first, 0);
        loop {
            e = next[&(v, e)];
            n += 1;
            if e == first {
                break;
            }
        }
        if n != corners[v as usize] {
            return Err(format!("the faces round point {v} make more than one fan: a cage's surface passes each point once"));
        }
    }
    if let Some(v) = corners.iter().position(|&n| n == 0) {
        return Err(format!("point {v} is in no face: each point of a cage is a corner of its faces"));
    }
    Ok(p)
}

/// What is wrong with the cage `points` and `faces`, if anything (see [`catmull_clark`]).
pub fn check(points: &[Vec3d], faces: &[Vec<u32>]) -> Result<(), String> {
    cage(points, faces).map(|_| ())
}

/// One step: the vertex points (in the vertices' order), then the edge points, then the face
/// points; every face's quads.
fn step(p: &Polys) -> Polys {
    let e = edges(p);
    let (nv, ne, nf) = (p.verts.len(), e.ends.len(), p.faces());
    let face_pt: Vec<Vec3d> = (0..nf)
        .map(|f| {
            let c = p.face(f);
            c.iter().fold(Vec3d::ZERO, |s, &v| s + p.verts[v as usize]) / c.len() as f64
        })
        .collect();
    let mut verts = Vec::with_capacity(nv + ne + nf);
    // per vertex: the sum of its faces' points, of its edges' midpoints, and how many (every
    // vertex is in a face)
    let (mut q, mut r, mut n) = (vec![Vec3d::ZERO; nv], vec![Vec3d::ZERO; nv], vec![0u32; nv]);
    for f in 0..nf {
        for &v in p.face(f) {
            q[v as usize] += face_pt[f];
        }
    }
    for [a, b] in &e.ends {
        let mid = (p.verts[*a as usize] + p.verts[*b as usize]) * 0.5;
        for v in [*a, *b] {
            r[v as usize] += mid;
            n[v as usize] += 1;
        }
    }
    for v in 0..nv {
        let k = n[v] as f64;
        verts.push((q[v] / k + r[v] * (2.0 / k) + p.verts[v] * (k - 3.0)) / k);
    }
    for (ends, faces) in e.ends.iter().zip(&e.faces) {
        verts.push((p.verts[ends[0] as usize] + p.verts[ends[1] as usize] + face_pt[faces[0] as usize] + face_pt[faces[1] as usize]) * 0.25);
    }
    verts.extend_from_slice(&face_pt);
    let mut out = Polys { verts, start: Vec::with_capacity(p.idx.len() + 1), idx: Vec::with_capacity(p.idx.len() * 4) };
    out.start.push(0);
    for f in 0..nf {
        let (s, k) = (p.start[f] as usize, p.face(f).len());
        for i in 0..k {
            let edge = |j: usize| nv as u32 + e.of_face[s + j];
            out.idx.extend_from_slice(&[p.idx[s + i], edge(i), (nv + ne + f) as u32, edge((i + k - 1) % k)]);
            out.start.push(out.idx.len() as u32);
        }
    }
    out
}

/// Every vertex's ring on a closed quad mesh, counter-clockwise seen from outside: per vertex its
/// (edge neighbour e_i, corner across f_i) in turn.
fn rings(p: &Polys) -> Vec<Vec<(u32, u32)>> {
    // round v: the quad (v, e, f, e') links e to (f, e')
    let mut link: Vec<Vec<(u32, u32, u32)>> = vec![Vec::new(); p.verts.len()];
    for f in 0..p.faces() {
        let q = p.face(f);
        for j in 0..4 {
            link[q[j] as usize].push((q[(j + 1) % 4], q[(j + 2) % 4], q[(j + 3) % 4]));
        }
    }
    link.iter()
        .map(|l| {
            let mut ring = Vec::with_capacity(l.len());
            if let Some(&(first, ..)) = l.first() {
                let mut e = first;
                loop {
                    let &(_, f, next) = l.iter().find(|x| x.0 == e).expect("a closed cage's ring");
                    ring.push((e, f));
                    e = next;
                    if e == first {
                        break;
                    }
                }
            }
            ring
        })
        .collect()
}

/// How much nearer its limit point a step brings a point of valence `n` whose ring is alike all
/// round. A step takes (v, e, f) to ((n - 7/4) v + 3/2 e + 1/4 f) / n, (3 v + 4 e + f) / 8 and
/// (v + 2 e + f) / 4; the point's way to its limit shrinks by that map's larger root besides 1, of
/// x² - (3n - 7) / 4n x + (n - 3) / 16n: 1/6 at valence 3, 1/4 at 4, 0.3225 at 5.
fn rate(n: usize) -> f64 {
    let n = n as f64;
    let (s, p) = ((3.0 * n - 7.0) / (4.0 * n), (n - 3.0) / (16.0 * n));
    (s + (s * s - 4.0 * p).sqrt()) / 2.0
}

/// The limit point under a vertex at `v` with this ring.
fn limit(p: &Polys, v: Vec3d, ring: &[(u32, u32)]) -> Vec3d {
    let n = ring.len() as f64;
    let (mut e, mut f) = (Vec3d::ZERO, Vec3d::ZERO);
    for &(a, b) in ring {
        e += p.verts[a as usize];
        f += p.verts[b as usize];
    }
    (v * (n * n) + e * 4.0 + f) / (n * (n + 5.0))
}

/// The limit normal (not of unit length) under a vertex with this ring.
fn normal(p: &Polys, ring: &[(u32, u32)]) -> Vec3d {
    let n = ring.len();
    let w = 2.0 * PI / n as f64;
    let a = 1.0 + w.cos() + (w * 0.5).cos() * (2.0 * (9.0 + w.cos())).sqrt();
    let c = |i: isize| (w * i as f64).cos();
    let (mut tu, mut tv) = (Vec3d::ZERO, Vec3d::ZERO);
    for (i, &(e, f)) in ring.iter().enumerate() {
        let i = i as isize;
        let (e, f) = (p.verts[e as usize], p.verts[f as usize]);
        tu += e * (a * c(i)) + f * (c(i) + c(i + 1));
        tv += e * (a * c(i - 1)) + f * (c(i - 1) + c(i));
    }
    tu.cross(tv)
}

/// The error for a limit normal of no length at subdivided point `v` of a cage of `cage` points:
/// a step keeps the cage's points first, so only a `v` under `cage` is one of them.
fn folds_at(v: usize, cage: usize) -> String {
    if v < cage {
        format!("the surface turns flat on itself near point {v}: its cage folds there")
    } else {
        "the surface turns flat on itself between the cage's points: its cage folds there".into()
    }
}

/// The cage `points` and `faces` (each listed counter-clockwise seen from outside) subdivided by
/// Catmull-Clark `levels` times, its points on the limit surface, with its limit normals. A
/// `draft` (a live drag's) takes one level fewer than `levels` asks, and at least one: a chord's
/// stops where the next level would bring every point within it, by the rate the vertex rule
/// sets at the highest valence there.
pub fn catmull_clark(points: &[Vec3d], faces: &[Vec<u32>], levels: Levels, draft: bool) -> Result<TriMesh, String> {
    let mut p = cage(points, faces)?;
    let most = match levels {
        Levels::Steps(n) if (1..=MAX_LEVELS).contains(&n) => if draft { (n - 1).max(1) } else { n },
        Levels::Steps(n) => return Err(format!("{n} levels: a cage takes 1 to {MAX_LEVELS}")),
        Levels::Chord(c) if c > 0.0 => MAX_LEVELS,
        Levels::Chord(c) => return Err(format!("a chord of {c}: it is more than 0")),
    };
    let mut level = 0;
    let (ring, at) = loop {
        // a step makes a quad of each corner of each face
        if p.idx.len() > MAX_QUADS {
            return Err(format!("{} quads at {} levels (at most {MAX_QUADS}): fewer levels, or fewer faces", p.idx.len(), level + 1));
        }
        p = step(&p);
        level += 1;
        let ring = rings(&p);
        let at: Vec<Vec3d> = (0..p.verts.len()).map(|v| limit(&p, p.verts[v], &ring[v])).collect();
        let done = match levels {
            Levels::Steps(_) => level == most,
            // the chord asks for no more levels than make at most MAX_QUADS
            Levels::Chord(c) => {
                let off = p.verts.iter().zip(&at).map(|(v, l)| v.distance(*l)).fold(0.0, f64::max);
                let next = if draft { rate(ring.iter().map(Vec::len).max().unwrap_or(4)) } else { 1.0 };
                level == most || p.idx.len() > MAX_QUADS || off * next <= c
            }
        };
        if done {
            break (ring, at);
        }
    };
    let mut normals = Vec::with_capacity(at.len());
    for (v, r) in ring.iter().enumerate() {
        let n = normal(&p, r);
        let l = n.length();
        if !(l > 1e-30) {
            return Err(folds_at(v, points.len()));
        }
        normals.push(n / l);
    }
    let mut mesh = TriMesh::with_capacity(at.len(), p.faces() * 2);
    mesh.vertices = at;
    for f in 0..p.faces() {
        let [a, b, c, d]: [u32; 4] = p.face(f).try_into().expect("a quad");
        let (va, vb, vc, vd) = (mesh.vertices[a as usize], mesh.vertices[b as usize], mesh.vertices[c as usize], mesh.vertices[d as usize]);
        let tris = if va.distance(vc) <= vb.distance(vd) { [[a, b, c], [a, c, d]] } else { [[a, b, d], [b, c, d]] };
        mesh.triangles.extend_from_slice(&tris);
    }
    mesh.corner_normals = mesh.triangles.iter().flat_map(|t| t.map(|v| normals[v as usize])).collect();
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_csg_math::dvec3;

    /// The cube from -1 to 1, its faces counter-clockwise seen from outside.
    fn cube() -> (Vec<Vec3d>, Vec<Vec<u32>>) {
        let points = (0..8).map(|i| dvec3(if i & 1 == 0 { -1.0 } else { 1.0 }, if i & 2 == 0 { -1.0 } else { 1.0 }, if i & 4 == 0 { -1.0 } else { 1.0 })).collect();
        let faces = vec![vec![0, 2, 3, 1], vec![4, 5, 7, 6], vec![0, 1, 5, 4], vec![2, 6, 7, 3], vec![0, 4, 6, 2], vec![1, 3, 7, 5]];
        (points, faces)
    }

    /// A prism on a pentagon, its points pushed about: valences 3 at the corners, 5 at the
    /// pentagons' face points, 4 elsewhere.
    fn prism() -> (Vec<Vec3d>, Vec<Vec<u32>>) {
        let mut points = Vec::new();
        for (k, y) in [-1.0f64, 1.5].iter().enumerate() {
            for i in 0..5 {
                let a = i as f64 * 2.0 * PI / 5.0;
                let r = 2.0 + 0.3 * ((i * 3 + k * 2) % 5) as f64;
                points.push(dvec3(r * a.cos(), y + 0.2 * (i as f64 - 2.0) * (k as f64 - 0.5), r * a.sin()));
            }
        }
        let mut faces = vec![(0..5).collect::<Vec<u32>>(), (5..10).rev().collect()];
        for i in 0..5u32 {
            let j = (i + 1) % 5;
            faces.push(vec![i, i + 5, j + 5, j]);
        }
        (points, faces)
    }

    fn close(a: Vec3d, b: Vec3d, tol: f64) -> bool {
        a.distance(b) <= tol
    }

    /// One step on the cube makes the rules' points: face points at the faces' centres, edge
    /// points at the average of the ends and the two face points, each corner at
    /// (Q + 2R + (n - 3) S) / n = 5/9 of itself.
    #[test]
    fn one_step_on_a_cube_makes_the_rules_points() {
        let (points, faces) = cube();
        let p = step(&cage(&points, &faces).unwrap());
        assert_eq!(p.verts.len(), 8 + 12 + 6);
        assert_eq!(p.faces(), 24);
        for v in 0..8 {
            assert!(close(p.verts[v], points[v] * (5.0 / 9.0), 1e-12), "corner {v}: {:?}", p.verts[v]);
        }
        for v in 8..20 {
            // an edge point: two coordinates at +-0.75, one at 0
            let c = [p.verts[v].x, p.verts[v].y, p.verts[v].z];
            assert_eq!(c.iter().filter(|x| (x.abs() - 0.75).abs() < 1e-12).count(), 2, "edge point {c:?}");
            assert_eq!(c.iter().filter(|x| x.abs() < 1e-12).count(), 1, "edge point {c:?}");
        }
        for v in 20..26 {
            let c = [p.verts[v].x, p.verts[v].y, p.verts[v].z];
            assert_eq!(c.iter().filter(|x| (x.abs() - 1.0).abs() < 1e-12).count(), 1, "face point {c:?}");
        }
    }

    /// The cube's limit surface passes (1/2, 1/2, 1/2) under each corner, and 0.8395 out under
    /// each face's centre (the limit rule at a valence 4 point after a step: (16 v + 4 Σ e + Σ f)
    /// / 36); further steps close on the limit points the rule gives, each by the rate the vertex
    /// rule sets at the highest valence there.
    #[test]
    fn a_cube_cage_s_limit_points_are_the_rules() {
        let (points, faces) = cube();
        let mesh = catmull_clark(&points, &faces, Levels::Steps(1), false).unwrap();
        for v in 0..8 {
            assert!(close(mesh.vertices[v], points[v] * 0.5, 1e-12), "corner {v}: {:?}", mesh.vertices[v]);
        }
        let face: f64 = (16.0 + 4.0 * 4.0 * 0.75 + 4.0 * 5.0 / 9.0) / 36.0;
        assert!((face - 0.83950617).abs() < 1e-8);
        for v in 20..26 {
            assert!((mesh.vertices[v].length() - face).abs() < 1e-12, "face point {v}: {:?}", mesh.vertices[v]);
        }
        // The subdivided points close on the limit points (the vertex points keep their places),
        // each step by the vertex rule's rate at the highest valence there.
        assert!((rate(3) - 1.0 / 6.0).abs() < 1e-12 && (rate(4) - 0.25).abs() < 1e-12 && (rate(5) - 0.3225).abs() < 1e-4);
        for (points, faces, valence) in [(cube().0, cube().1, 4), (prism().0, prism().1, 5)] {
            let r = rate(valence);
            let at = catmull_clark(&points, &faces, Levels::Steps(1), false).unwrap().vertices;
            let mut p = step(&cage(&points, &faces).unwrap());
            let mut last = f64::INFINITY;
            for _ in 0..6 {
                p = step(&p);
                let off = (0..at.len()).map(|v| p.verts[v].distance(at[v])).fold(0.0, f64::max);
                assert!(off < last * r * 1.01, "{off} after {last}: more than {r}");
                if last.is_finite() && p.verts.len() > 50_000 {
                    assert!(off > last * r * 0.995, "{off} after {last}: less than {r}");
                }
                last = off;
            }
            assert!(last < 1e-3, "{last}");
        }
    }

    /// The limit normals face out, and they are the limit surface's: across from the ring of
    /// points round each point a few steps on, at valences 3, 4 and 5 of a lopsided cage.
    #[test]
    fn the_limit_normals_are_the_surface_s() {
        for (points, faces) in [cube(), prism()] {
            let mesh = catmull_clark(&points, &faces, Levels::Steps(1), false).unwrap();
            let normals = mesh.exact_normals().expect("its normals").to_vec();
            let mut at = vec![Vec3d::ZERO; mesh.vertices.len()];
            for (t, tri) in mesh.triangles.iter().enumerate() {
                for k in 0..3 {
                    at[tri[k] as usize] = normals[t * 3 + k];
                }
            }
            // a few steps on, the ring round each first-step point spans its tangent plane
            let mut p = step(&cage(&points, &faces).unwrap());
            for _ in 0..5 {
                p = step(&p);
            }
            let ring = rings(&p);
            let centre = mesh.vertices.iter().fold(Vec3d::ZERO, |s, v| s + *v) / mesh.vertices.len() as f64;
            for v in 0..mesh.vertices.len() {
                let r: Vec<Vec3d> = ring[v].iter().map(|(e, _)| limit(&p, p.verts[*e as usize], &ring[*e as usize])).collect();
                let mut n = Vec3d::ZERO;
                for i in 0..r.len() {
                    n += r[i].cross(r[(i + 1) % r.len()]);
                }
                let n = n.normalize();
                assert!(at[v].dot(n) > 0.99995, "point {v} of valence {}: {:?} against {:?}", ring[v].len(), at[v], n);
                assert!(at[v].dot(mesh.vertices[v] - centre) > 0.0, "point {v} faces in");
            }
        }
    }

    /// The chord picks the levels: the fewest that bring every point within it of its limit
    /// point (on the cube, each level about four times nearer); a draft takes one fewer.
    #[test]
    fn the_chord_picks_the_levels() {
        let (points, faces) = cube();
        let tris = |levels| catmull_clark(&points, &faces, levels, false).unwrap().triangles.len();
        assert_eq!(tris(Levels::Steps(2)), 6 * 16 * 2);
        let mut p = cage(&points, &faces).unwrap();
        let mut off = Vec::new();
        for _ in 0..4 {
            p = step(&p);
            let ring = rings(&p);
            off.push((0..p.verts.len()).map(|v| p.verts[v].distance(limit(&p, p.verts[v], &ring[v]))).fold(0.0, f64::max));
        }
        for w in off.windows(2) {
            assert!(w[1] < w[0] * 0.3 && w[1] > w[0] * 0.2, "{off:?}");
        }
        assert_eq!(tris(Levels::Chord(off[1] * 1.001)), tris(Levels::Steps(2)));
        assert_eq!(tris(Levels::Chord(off[1] * 0.999)), tris(Levels::Steps(3)));
        assert_eq!(tris(Levels::Chord(1.0)), tris(Levels::Steps(1)));
        assert_eq!(tris(Levels::Chord(1e-9)), tris(Levels::Steps(MAX_LEVELS)));
        // a draft: one level fewer, at least one
        let draft = |levels| catmull_clark(&points, &faces, levels, true).unwrap().triangles.len();
        assert_eq!(draft(Levels::Steps(3)), tris(Levels::Steps(2)));
        assert_eq!(draft(Levels::Steps(1)), tris(Levels::Steps(1)));
        assert_eq!(tris(Levels::Chord(off[1] * 0.5)), tris(Levels::Steps(3)));
        assert_eq!(draft(Levels::Chord(off[1] * 0.5)), tris(Levels::Steps(2)));
        assert_eq!(draft(Levels::Chord(1.0)), tris(Levels::Steps(1)));
        assert!(catmull_clark(&points, &faces, Levels::Steps(0), false).is_err());
        assert!(catmull_clark(&points, &faces, Levels::Chord(0.0), false).is_err());
    }

    /// A cage that is not closed, faces wound against each other, an edge in three faces, a
    /// point two fans meet at, a bad index: each says what is wrong.
    #[test]
    fn a_cage_must_be_closed() {
        let (points, faces) = cube();
        let err = |faces: Vec<Vec<u32>>| catmull_clark(&points, &faces, Levels::Steps(1), false).unwrap_err();
        assert!(err(faces[..5].to_vec()).contains("has only face"), "open");
        let mut flipped = faces.clone();
        flipped[1].reverse();
        assert!(err(flipped).contains("face opposite ways"), "wound");
        let mut extra = faces.clone();
        extra.push(vec![0, 1, 5]);
        assert!(err(extra).contains("faces"), "three");
        assert!(err(vec![vec![0, 1, 9]]).contains("point 9"), "index");
        assert!(err(vec![vec![0, 1, 1]]).contains("twice"), "repeat");
        // two cubes sharing a corner: a point where two fans meet
        let mut two = points.clone();
        two.extend((1..8).map(|i| points[i] + dvec3(2.0, 2.0, 2.0)));
        let shift = |v: u32| if v == 0 { 7 } else { v + 7 };
        let mut both = faces.clone();
        both.extend(faces.iter().map(|f| f.iter().map(|v| shift(*v)).collect::<Vec<u32>>()));
        let e = catmull_clark(&two, &both, Levels::Steps(1), false).unwrap_err();
        assert!(e.contains("more than one fan"), "{e}");
        let mut spare = points.clone();
        spare.push(dvec3(5.0, 0.0, 0.0));
        assert!(catmull_clark(&spare, &faces, Levels::Steps(1), false).unwrap_err().contains("point 8 is in no face"), "spare");
    }

    /// A fold names a cage point only where it is one: a point subdivision made is none (the last
    /// cage point was named for it).
    #[test]
    fn a_fold_names_a_cage_point_only_at_one() {
        // a pillow of two triangles back to back: its corners fold
        let pillow = [dvec3(0.0, 0.0, 0.0), dvec3(1.0, 0.0, 0.0), dvec3(0.0, 1.0, 0.0)];
        let e = catmull_clark(&pillow, &[vec![0, 1, 2], vec![0, 2, 1]], Levels::Steps(1), false).unwrap_err();
        assert!(e.contains("near point 0:"), "{e}");
        assert!(super::folds_at(7, 8).contains("near point 7:"));
        let made = super::folds_at(20, 8);
        assert!(made.contains("between the cage's points") && !made.contains("point 7"), "{made}");
    }
}
