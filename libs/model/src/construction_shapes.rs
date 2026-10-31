//! Parametric solids: analytic primitives and architectural pieces. Every
//! generator emits closed, outward-wound shells with metre-scale UVs (one UV
//! unit is one metre of surface) and explicit corner normals, so tiling
//! materials read at real size and curved surfaces shade smoothly without a
//! separate normals pass. Openings, stairs and roofs are built constructively,
//! never by boolean cuts, so they stay exact and cheap at any size.
use makepad_csg_math::portable::PortableFloat;
use super::*;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

pub(super) const MAX_PROFILE: usize = 512;
const MAX_OPENINGS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Opening {
    pub at: f64, pub width: f64, pub height: f64, pub sill: f64,
    /// Frame bar width (0 = bare opening) and depth; glass fills a framed window.
    pub frame: f64, pub frame_depth: f64, pub frame_material: u32, pub glass: Option<u32>, pub mullions: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    pub path: Vec<[f64; 2]>, pub closed: bool, pub height: f64, pub thickness: f64, pub base: f64,
    pub openings: Vec<Opening>, pub inner_material: Option<u32>, pub trim_material: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoofKind { Gable, Hip, Flat }
#[derive(Clone, Debug, PartialEq)]
pub struct Roof {
    pub kind: RoofKind, pub center: [f64; 3], pub size: [f64; 2], pub pitch: f64, pub overhang: f64, pub thickness: f64,
    /// Gable ridge runs along X (0) or Z (2). Hip roofs ridge along the longer side.
    pub ridge: usize, pub trim_material: Option<u32>, pub gable_material: Option<u32>, pub gable_thickness: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Box { size: [f64; 3], radius: f64, segments: u32 },
    Capsule { radius: f64, height: f64, segments: u32, rings: u32 },
    Cone { bottom: f64, top: f64, height: f64, segments: u32, smooth: bool },
    Torus { radius: f64, tube: f64, segments: u32, sides: u32 },
    Wedge { size: [f64; 3] },
    Extrude { profile: Vec<[f64; 2]>, depth: f64, axis: usize, center: bool, corner_radius: f64, corner_segments: u32, bevel: f64, bevel_segments: u32 },
    Tube { path: Vec<[f64; 3]>, radius: f64, segments: u32, caps: bool },
    Wall(Wall),
    Stair { from: [f64; 3], to: [f64; 3], width: f64, steps: u32, thickness: f64 },
    Roof(Roof),
}

fn positive(v: f64, what: &'static str) -> Result<f64> { if v.is_finite() && v > 0. && v <= 1e5 { Ok(v) } else { Err(Error::Invalid(what)) } }
fn in_range(v: u32, lo: u32, hi: u32, what: &'static str) -> Result<u32> { if (lo..=hi).contains(&v) { Ok(v) } else { Err(Error::Invalid(what)) } }
impl Shape {
    pub(super) fn generate(&self, material: u32, ctx: &mut Context<'_>) -> Result<Mesh> {
        let mut b = Builder::default();
        match self {
            Shape::Box { size, radius, segments } => rounded_box(&mut b, *size, *radius, *segments, material)?,
            Shape::Capsule { radius, height, segments, rings } => capsule(&mut b, *radius, *height, *segments as usize, *rings as usize, material),
            Shape::Cone { bottom, top, height, segments, smooth } => cone(&mut b, *bottom, *top, *height, *segments as usize, *smooth, material),
            Shape::Torus { radius, tube, segments, sides } => torus(&mut b, *radius, *tube, *segments as usize, *sides as usize, material),
            Shape::Wedge { size } => wedge(&mut b, *size, material),
            Shape::Extrude { profile, depth, axis, center, corner_radius, corner_segments, bevel, bevel_segments } => {
                let (u, v, w) = axis_basis(*axis);
                let w0 = if *center { -depth * 0.5 } else { 0. };
                let profile = round_corners(profile, *corner_radius, *corner_segments)?;
                // Insetting an arc by its own radius collapses it; keep the
                // cap bevel inside the fillets.
                let bevel = if *corner_radius > 0. { bevel.min(corner_radius * 0.7) } else { *bevel };
                prism(&mut b, &profile, Frame { origin: [0.; 3], u, v, w }, w0, w0 + depth, bevel, *bevel_segments, &|_, _| material, material)?;
            }
            Shape::Tube { path, radius, segments, caps } => tube(&mut b, path, *radius, *segments as usize, *caps, material)?,
            Shape::Wall(w) => wall(&mut b, w, material)?,
            Shape::Stair { from, to, width, steps, thickness } => stair(&mut b, *from, *to, *width, *steps as usize, *thickness, material)?,
            Shape::Roof(r) => roof(&mut b, r, material)?,
        }
        b.finish(ctx)
    }
}

/// Polygon soup with shared vertices and optional per-corner normals.
#[derive(Default)]
struct Builder { positions: Vec<[f64; 3]>, polygons: Vec<Polygon>, normals: Vec<Option<Vec<[f64; 3]>>> }
impl Builder {
    fn vertex(&mut self, p: [f64; 3]) -> u32 { self.positions.push(p); (self.positions.len() - 1) as u32 }
    fn face(&mut self, vertices: Vec<u32>, uvs: Vec<[f64; 2]>, normals: Option<Vec<[f64; 3]>>, material: u32) {
        self.polygons.push(Polygon { vertices, uvs, material });
        self.normals.push(normals);
    }
    /// Adds a planar face whose winding is flipped if needed so its geometric
    /// normal agrees with `outward`. UVs come from a planar projection.
    fn oriented(&mut self, mut vertices: Vec<u32>, outward: [f64; 3], material: u32, uv: &dyn Fn([f64; 3]) -> [f64; 2]) {
        if dot(self.newell(&vertices), outward) < 0. { vertices.reverse(); }
        let uvs = vertices.iter().map(|&i| uv(self.positions[i as usize])).collect();
        self.face(vertices, uvs, None, material);
    }
    fn newell(&self, vertices: &[u32]) -> [f64; 3] {
        let mut n = [0.; 3];
        for i in 0..vertices.len() {
            let a = self.positions[vertices[i] as usize]; let b = self.positions[vertices[(i + 1) % vertices.len()] as usize];
            n = add(n, [(a[1] - b[1]) * (a[2] + b[2]), (a[2] - b[2]) * (a[0] + b[0]), (a[0] - b[0]) * (a[1] + b[1])]);
        }
        n
    }
    /// Planar face with an automatic in-plane UV basis (metres).
    fn planar(&mut self, vertices: Vec<u32>, outward: [f64; 3], material: u32) {
        let (e1, e2) = plane_axes(outward);
        self.oriented(vertices, outward, material, &|p| [dot(p, e1), dot(p, e2)]);
    }
    fn finish(self, ctx: &mut Context<'_>) -> Result<Mesh> {
        let corners = self.polygons.iter().map(|p| p.vertices.len()).sum::<usize>();
        admit(self.positions.len(), self.polygons.len(), corners, ctx)?;
        let mut mesh = Mesh::from_polygons(&self.positions, &self.polygons, ctx)?;
        if self.normals.iter().any(Option::is_some) {
            let mut values = Vec::with_capacity(corners);
            // Faces and their corners keep input order in a fresh mesh.
            for (face, normals) in mesh.faces().iter().zip(&self.normals) {
                ctx.checkpoint(1)?;
                if let Some(normals) = normals {
                    for (corner, n) in mesh.face_corners(face.id)?.iter().zip(normals) {
                        values.push((corner.id, Some(unit(*n).unwrap_or([0., 1., 0.]))));
                    }
                }
            }
            mesh.set_corner_normals_bulk(&values, ctx)?;
        }
        Ok(mesh)
    }
}
fn plane_axes(n: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let n = unit(n).unwrap_or([0., 1., 0.]);
    let e1 = if n[1].abs() > 0.9 { [1., 0., 0.] } else { unit(cross([0., 1., 0.], n)).unwrap_or([1., 0., 0.]) };
    (e1, cross(n, e1))
}

fn rounded_box(b: &mut Builder, size: [f64; 3], radius: f64, segments: u32, material: u32) -> Result<()> {
    let h = size.map(|s| s * 0.5);
    let r = radius.min(h[0]).min(h[1]).min(h[2]);
    // Grid coordinates per axis. The rounded band uses tan spacing so the
    // projected arc samples are evenly spaced in angle; the flat middle is a
    // single span, keeping big flat faces to one quad row.
    let m = segments.div_ceil(2).max(1) as usize;
    let coords = |a: usize| -> Vec<f64> {
        if r <= 1e-9 { return vec![-h[a], h[a]]; }
        let inner = h[a] - r;
        let mut out: Vec<f64> = (0..=m).rev().map(|k| -(inner + r * (FRAC_PI_4 * k as f64 / m as f64).ptan())).collect();
        for k in 0..=m { out.push(inner + r * (FRAC_PI_4 * k as f64 / m as f64).ptan()); }
        if inner <= 1e-9 { out.remove(m + 1); }
        out
    };
    let grid = [coords(0), coords(1), coords(2)];
    let mut ids = std::collections::HashMap::<[usize; 3], u32>::new();
    let mut normal_of = std::collections::HashMap::<u32, [f64; 3]>::new();
    for a in 0..3 {
        let (bx, cx) = ((a + 1) % 3, (a + 2) % 3);
        for side in [0usize, grid[a].len() - 1] {
            let sign = if side == 0 { -1. } else { 1. };
            for i in 0..grid[bx].len() - 1 {
                for j in 0..grid[cx].len() - 1 {
                    let mut corners = Vec::with_capacity(4);
                    for (di, dj) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                        let mut key = [0; 3]; key[a] = side; key[bx] = i + di; key[cx] = j + dj;
                        let id = *ids.entry(key).or_insert_with(|| {
                            let p = [grid[0][key[0]], grid[1][key[1]], grid[2][key[2]]];
                            let (pos, n) = if r <= 1e-9 {
                                (p, [0.; 3])
                            } else {
                                let inner: [f64; 3] = std::array::from_fn(|d| p[d].clamp(-(h[d] - r), h[d] - r));
                                let n = unit(sub(p, inner)).unwrap_or([0., 1., 0.]);
                                (add(inner, mul(n, r)), n)
                            };
                            let id = b.vertex(pos);
                            normal_of.insert(id, n);
                            id
                        });
                        corners.push(id);
                    }
                    if sign < 0. { corners.reverse(); }
                    let uvs = corners.iter().map(|&v| { let p = b.positions[v as usize]; [p[bx] * sign, p[cx]] }).collect();
                    let normals = (r > 1e-9).then(|| corners.iter().map(|v| normal_of[v]).collect());
                    b.face(corners, uvs, normals, material);
                }
            }
        }
    }
    Ok(())
}

/// Surface of revolution about Y. Rings are [radius, y, normal_radial,
/// normal_y]; a zero radius is a pole. Returns the vertex ids per ring.
fn revolve(b: &mut Builder, rings: &[[f64; 4]], n: usize, material: u32, smooth: bool) -> Vec<Vec<u32>> {
    let angle = |j: f64| TAU * j / n as f64;
    let ids: Vec<Vec<u32>> = rings.iter().map(|&[r, y, ..]| {
        if r <= 1e-12 { vec![b.vertex([0., y, 0.])] } else { (0..n).map(|j| { let t = angle(j as f64); b.vertex([r * t.psin(), y, r * t.pcos()]) }).collect() }
    }).collect();
    let circumference = TAU * rings.iter().map(|r| r[0]).fold(0., f64::max);
    let mut v_along = vec![0.];
    for i in 1..rings.len() { v_along.push(v_along[i - 1] + (rings[i][0] - rings[i - 1][0]).phypot(rings[i][1] - rings[i - 1][1])); }
    let normal = |ring: &[f64; 4], t: f64| [ring[2] * t.psin(), ring[3], ring[2] * t.pcos()];
    for i in 0..rings.len() - 1 {
        let (a, c) = (&ids[i], &ids[i + 1]);
        if a.len() == 1 && c.len() == 1 { continue; }
        for j in 0..n {
            let k = (j + 1) % n;
            let (u0, u1) = (j as f64 / n as f64 * circumference, (j + 1) as f64 / n as f64 * circumference);
            let (t0, t1, tm) = (angle(j as f64), angle(j as f64 + 1.), angle(j as f64 + 0.5));
            let (v0, v1) = (v_along[i], v_along[i + 1]);
            let (verts, uvs, normals) = if a.len() == 1 {
                (vec![a[0], c[k], c[j]], vec![[(u0 + u1) * 0.5, v0], [u1, v1], [u0, v1]], vec![normal(&rings[i], tm), normal(&rings[i + 1], t1), normal(&rings[i + 1], t0)])
            } else if c.len() == 1 {
                (vec![a[j], a[k], c[0]], vec![[u0, v0], [u1, v0], [(u0 + u1) * 0.5, v1]], vec![normal(&rings[i], t0), normal(&rings[i], t1), normal(&rings[i + 1], tm)])
            } else {
                (vec![a[j], a[k], c[k], c[j]], vec![[u0, v0], [u1, v0], [u1, v1], [u0, v1]], vec![normal(&rings[i], t0), normal(&rings[i], t1), normal(&rings[i + 1], t1), normal(&rings[i + 1], t0)])
            };
            b.face(verts, uvs, smooth.then_some(normals), material);
        }
    }
    ids
}
fn disc(b: &mut Builder, ring: &[u32], down: bool, material: u32) {
    let mut vertices = ring.to_vec();
    if down { vertices.reverse(); }
    let uvs = vertices.iter().map(|&v| { let p = b.positions[v as usize]; [p[0], if down { -p[2] } else { p[2] }] }).collect();
    let n = if down { [0., -1., 0.] } else { [0., 1., 0.] };
    b.face(vertices.clone(), uvs, Some(vec![n; vertices.len()]), material);
}
fn capsule(b: &mut Builder, radius: f64, height: f64, n: usize, rings: usize, material: u32) {
    let half = (height * 0.5 - radius).max(0.);
    let mut profile = Vec::new();
    for i in 0..=rings {
        let phi = -FRAC_PI_2 + FRAC_PI_2 * i as f64 / rings as f64;
        profile.push([radius * phi.pcos(), -half + radius * phi.psin(), phi.pcos(), phi.psin()]);
    }
    for i in 0..=rings {
        if i == 0 && half <= 1e-9 { continue; }
        let phi = FRAC_PI_2 * i as f64 / rings as f64;
        profile.push([radius * phi.pcos(), half + radius * phi.psin(), phi.pcos(), phi.psin()]);
    }
    revolve(b, &profile, n, material, true);
}
fn cone(b: &mut Builder, bottom: f64, top: f64, height: f64, n: usize, smooth: bool, material: u32) {
    let slope = unit([height, 0., bottom - top]).unwrap();
    let ids = revolve(b, &[[bottom, -height * 0.5, slope[0], slope[2]], [top, height * 0.5, slope[0], slope[2]]], n, material, smooth);
    if ids[0].len() > 1 { disc(b, &ids[0], true, material); }
    if ids[1].len() > 1 { disc(b, &ids[1], false, material); }
}
fn torus(b: &mut Builder, radius: f64, tube: f64, n: usize, m: usize, material: u32) {
    let dir = |i: usize, j: usize| { let (t, p) = (TAU * i as f64 / n as f64, TAU * j as f64 / m as f64); [t.psin() * p.pcos(), p.psin(), t.pcos() * p.pcos()] };
    let ids: Vec<Vec<u32>> = (0..n).map(|i| (0..m).map(|j| {
        let d = dir(i, j); let t = TAU * i as f64 / n as f64;
        b.vertex(add([radius * t.psin(), 0., radius * t.pcos()], mul(d, tube)))
    }).collect()).collect();
    let (lu, lv) = (TAU * radius, TAU * tube);
    for i in 0..n {
        for j in 0..m {
            let (i1, j1) = ((i + 1) % n, (j + 1) % m);
            let uv = |a: usize, c: usize| [a as f64 / n as f64 * lu, c as f64 / m as f64 * lv];
            b.face(vec![ids[i][j], ids[i1][j], ids[i1][j1], ids[i][j1]], vec![uv(i, j), uv(i + 1, j), uv(i + 1, j + 1), uv(i, j + 1)],
                Some(vec![dir(i, j), dir(i1, j), dir(i1, j1), dir(i, j1)]), material);
        }
    }
}
/// A ramp: full height along the back (-Z) edge, falling to zero at +Z.
fn wedge(b: &mut Builder, size: [f64; 3], material: u32) {
    let [x, y, z] = size.map(|s| s * 0.5);
    let p = [[-x, -y, -z], [x, -y, -z], [x, -y, z], [-x, -y, z], [-x, y, -z], [x, y, -z]].map(|p| b.vertex(p));
    let slope = unit([0., size[2], size[1]]).unwrap();
    for (face, n) in [(vec![p[0], p[1], p[2], p[3]], [0., -1., 0.]), (vec![p[0], p[1], p[5], p[4]], [0., 0., -1.]), (vec![p[4], p[5], p[2], p[3]], slope),
        (vec![p[0], p[4], p[3]], [-1., 0., 0.]), (vec![p[1], p[5], p[2]], [1., 0., 0.])] {
        b.planar(face, n, material);
    }
}

#[derive(Clone, Copy)]
struct Frame { origin: [f64; 3], u: [f64; 3], v: [f64; 3], w: [f64; 3] }
impl Frame {
    fn at(&self, u: f64, v: f64, w: f64) -> [f64; 3] { add(self.origin, add(add(mul(self.u, u), mul(self.v, v)), mul(self.w, w))) }
    fn dir(&self, u: f64, v: f64, w: f64) -> [f64; 3] { add(add(mul(self.u, u), mul(self.v, v)), mul(self.w, w)) }
    fn mirrored(&self) -> bool { dot(cross(self.u, self.v), self.w) < 0. }
}
/// Profile plane and extrusion direction. Along Y the profile is given in
/// plan (X, Z), which is how floor slabs and footprints are written.
fn axis_basis(axis: usize) -> ([f64; 3], [f64; 3], [f64; 3]) {
    match axis { 0 => ([0., 0., 1.], [0., 1., 0.], [1., 0., 0.]), 1 => ([1., 0., 0.], [0., 0., 1.], [0., 1., 0.]), _ => ([1., 0., 0.], [0., 1., 0.], [0., 0., 1.]) }
}
fn area2(p: &[[f64; 2]]) -> f64 { (0..p.len()).map(|i| { let (a, b) = (p[i], p[(i + 1) % p.len()]); a[0] * b[1] - b[0] * a[1] }).sum::<f64>() * 0.5 }
fn clean_profile(profile: &[[f64; 2]]) -> Result<Vec<[f64; 2]>> {
    let mut p: Vec<[f64; 2]> = Vec::with_capacity(profile.len());
    for &q in profile {
        if q.iter().any(|v| !v.is_finite() || v.abs() > 1e5) { return Err(Error::Invalid("profile point out of range")); }
        if p.last().is_none_or(|l: &[f64; 2]| (l[0] - q[0]).phypot(l[1] - q[1]) > 1e-7) { p.push(q); }
    }
    while p.len() > 1 && (p[0][0] - p[p.len() - 1][0]).phypot(p[0][1] - p[p.len() - 1][1]) <= 1e-7 { p.pop(); }
    if p.len() < 3 { return Err(Error::Invalid("profile needs three distinct points")); }
    let a = area2(&p);
    if a.abs() < 1e-9 { return Err(Error::Invalid("profile encloses no area")); }
    if a < 0. { p.reverse(); }
    Ok(p)
}
/// Fillets every corner of a closed 2D profile with circular arcs.
pub(super) fn round_corners(profile: &[[f64; 2]], radius: f64, segments: u32) -> Result<Vec<[f64; 2]>> {
    let p = clean_profile(profile)?;
    if radius <= 1e-9 { return Ok(p); }
    let n = p.len();
    let mut out = Vec::with_capacity(n * (segments as usize + 1));
    for i in 0..n {
        let (prev, cur, next) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
        let (l1, l2) = ((prev[0] - cur[0]).phypot(prev[1] - cur[1]), (next[0] - cur[0]).phypot(next[1] - cur[1]));
        let e1 = [(prev[0] - cur[0]) / l1, (prev[1] - cur[1]) / l1];
        let e2 = [(next[0] - cur[0]) / l2, (next[1] - cur[1]) / l2];
        let cos = (e1[0] * e2[0] + e1[1] * e2[1]).clamp(-1., 1.);
        let theta = cos.pacos();
        if theta > std::f64::consts::PI - 1e-3 || theta < 1e-3 { out.push(cur); continue; }
        // Tangent distance, limited so neighbouring fillets never overlap.
        let t = (radius / (theta * 0.5).ptan()).min(0.49 * l1.min(l2));
        let r = t * (theta * 0.5).ptan();
        let bis = { let s = [e1[0] + e2[0], e1[1] + e2[1]]; let l = s[0].phypot(s[1]); [s[0] / l, s[1] / l] };
        let d = r / (theta * 0.5).psin();
        let c = [cur[0] + bis[0] * d, cur[1] + bis[1] * d];
        let (a, bpt) = ([cur[0] + e1[0] * t, cur[1] + e1[1] * t], [cur[0] + e2[0] * t, cur[1] + e2[1] * t]);
        let (a0, mut a1) = ((a[1] - c[1]).patan2(a[0] - c[0]), (bpt[1] - c[1]).patan2(bpt[0] - c[0]));
        while a1 - a0 > std::f64::consts::PI { a1 -= TAU; }
        while a0 - a1 > std::f64::consts::PI { a1 += TAU; }
        for k in 0..=segments {
            let t = a0 + (a1 - a0) * k as f64 / segments as f64;
            out.push([c[0] + r * t.pcos(), c[1] + r * t.psin()]);
        }
    }
    clean_profile(&out)
}
/// Closed prism of a CCW profile between w0 and w1 along the frame's W axis,
/// optionally with rounded cap edges. Sharp profile corners split normals;
/// corners under ~40 degrees (fillets, arcs) shade smoothly.
fn prism(b: &mut Builder, profile: &[[f64; 2]], frame: Frame, w0: f64, w1: f64, bevel: f64, bevel_segments: u32, side_material: &dyn Fn([f64; 2], [f64; 2]) -> u32, cap_material: u32) -> Result<()> {
    let p = clean_profile(profile)?;
    let n = p.len();
    let edge_normal: Vec<[f64; 2]> = (0..n).map(|i| { let (a, c) = (p[i], p[(i + 1) % n]); let (dx, dy) = (c[0] - a[0], c[1] - a[1]); let l = dx.phypot(dy); [dy / l, -dx / l] }).collect();
    let prev = |i: usize| edge_normal[(i + n - 1) % n];
    let smooth: Vec<bool> = (0..n).map(|i| { let (a, c) = (prev(i), edge_normal[i]); a[0] * c[0] + a[1] * c[1] > 0.766 }).collect();
    let vertex_normal: Vec<[f64; 2]> = (0..n).map(|i| { let s = [prev(i)[0] + edge_normal[i][0], prev(i)[1] + edge_normal[i][1]]; let l = s[0].phypot(s[1]).max(1e-12); [s[0] / l, s[1] / l] }).collect();
    let miter: Vec<[f64; 2]> = (0..n).map(|i| { let (a, c) = (prev(i), edge_normal[i]); let d = (1. + a[0] * c[0] + a[1] * c[1]).max(0.25); [(a[0] + c[0]) / d, (a[1] + c[1]) / d] }).collect();
    let bevel = bevel.min((w1 - w0) * 0.45);
    // Ring layers: (inset, w, theta from cap normal). theta=90deg is the side wall.
    let mut layers: Vec<(f64, f64, f64, f64)> = Vec::new();
    if bevel > 1e-9 {
        for k in 0..=bevel_segments { let t = FRAC_PI_2 * k as f64 / bevel_segments as f64; layers.push((bevel * (1. - t.psin()), w0 + bevel * (1. - t.pcos()), t, -1.)); }
        for k in (0..=bevel_segments).rev() { let t = FRAC_PI_2 * k as f64 / bevel_segments as f64; layers.push((bevel * (1. - t.psin()), w1 - bevel * (1. - t.pcos()), t, 1.)); }
    } else {
        layers.push((0., w0, FRAC_PI_2, -1.));
        layers.push((0., w1, FRAC_PI_2, 1.));
    }
    let rings: Vec<Vec<u32>> = layers.iter().map(|&(inset, w, ..)| (0..n).map(|i| b.vertex(frame.at(p[i][0] - miter[i][0] * inset, p[i][1] - miter[i][1] * inset, w))).collect()).collect();
    let mut along = vec![0.];
    for i in 0..n { along.push(along[i] + (p[(i + 1) % n][0] - p[i][0]).phypot(p[(i + 1) % n][1] - p[i][1])); }
    let flip = frame.mirrored();
    let push = |b: &mut Builder, mut verts: Vec<u32>, mut uvs: Vec<[f64; 2]>, mut normals: Vec<[f64; 3]>, material: u32| {
        if flip { verts.reverse(); uvs.reverse(); normals.reverse(); }
        b.face(verts, uvs, Some(normals), material);
    };
    for k in 0..layers.len() - 1 {
        let ((_, wa, ta, sa), (_, wb, tb, sb)) = (layers[k], layers[k + 1]);
        for i in 0..n {
            let j = (i + 1) % n;
            let side_n = |vi: usize, t: f64, s: f64| { let n2 = if smooth[vi] { vertex_normal[vi] } else { edge_normal[i] }; frame.dir(n2[0] * t.psin(), n2[1] * t.psin(), s * t.pcos()) };
            push(b, vec![rings[k][i], rings[k][j], rings[k + 1][j], rings[k + 1][i]], vec![[along[i], wa], [along[i + 1], wa], [along[i + 1], wb], [along[i], wb]],
                vec![side_n(i, ta, sa), side_n(j, ta, sa), side_n(j, tb, sb), side_n(i, tb, sb)], side_material(p[i], p[j]));
        }
    }
    let (first, last) = (&rings[0], &rings[rings.len() - 1]);
    let uv = |b: &Builder, v: u32| { let q = sub(b.positions[v as usize], frame.origin); [dot(q, frame.u), dot(q, frame.v)] };
    let back: Vec<u32> = first.iter().rev().copied().collect();
    let (bu, fu) = (back.iter().map(|&v| uv(b, v)).collect(), last.iter().map(|&v| uv(b, v)).collect());
    push(b, back, bu, vec![frame.dir(0., 0., -1.); n], cap_material);
    push(b, last.clone(), fu, vec![frame.dir(0., 0., 1.); n], cap_material);
    Ok(())
}
fn tube(b: &mut Builder, path: &[[f64; 3]], radius: f64, segments: usize, caps: bool, material: u32) -> Result<()> {
    let mut path: Vec<[f64; 3]> = path.to_vec();
    path.dedup_by(|a, c| length(sub(*a, *c)) < 1e-7);
    if path.len() < 2 { return Err(Error::Invalid("tube path needs two distinct points")); }
    let mut rings: Vec<Vec<u32>> = Vec::new(); let mut dirs: Vec<Vec<[f64; 3]>> = Vec::new();
    let mut normal: Option<[f64; 3]> = None;
    let mut along = vec![0.];
    for i in 0..path.len() {
        let tangent = unit(sub(path[(i + 1).min(path.len() - 1)], path[i.saturating_sub(1)]))?;
        let nrm = match normal {
            Some(prev) => { let q = sub(prev, mul(tangent, dot(prev, tangent))); if length(q) > 1e-8 { unit(q)? } else { frame_normal(tangent)? } }
            None => frame_normal(tangent)?,
        };
        normal = Some(nrm);
        let binormal = cross(tangent, nrm);
        let ring_dirs: Vec<[f64; 3]> = (0..segments).map(|j| { let t = TAU * j as f64 / segments as f64; add(mul(nrm, t.pcos()), mul(binormal, t.psin())) }).collect();
        rings.push(ring_dirs.iter().map(|d| b.vertex(add(path[i], mul(*d, radius)))).collect());
        dirs.push(ring_dirs);
        if i > 0 { along.push(along[i - 1] + length(sub(path[i], path[i - 1]))); }
    }
    let lu = TAU * radius;
    for i in 0..path.len() - 1 {
        for j in 0..segments {
            let k = (j + 1) % segments;
            let (u0, u1) = (j as f64 / segments as f64 * lu, (j + 1) as f64 / segments as f64 * lu);
            // Around the ring first, then along the path: (binormal x
            // tangent) is the outward radial direction.
            b.face(vec![rings[i][j], rings[i][k], rings[i + 1][k], rings[i + 1][j]], vec![[u0, along[i]], [u1, along[i]], [u1, along[i + 1]], [u0, along[i + 1]]],
                Some(vec![dirs[i][j], dirs[i][k], dirs[i + 1][k], dirs[i + 1][j]]), material);
        }
    }
    if caps {
        let t0 = unit(sub(path[1], path[0]))?; let t1 = unit(sub(path[path.len() - 1], path[path.len() - 2]))?;
        let start: Vec<u32> = rings[0].clone(); let end: Vec<u32> = rings[rings.len() - 1].clone();
        b.planar(start, mul(t0, -1.), material);
        b.planar(end, t1, material);
    }
    Ok(())
}

fn wall(b: &mut Builder, w: &Wall, material: u32) -> Result<()> {
    let pts = &w.path;
    let segs = if w.closed { pts.len() } else { pts.len() - 1 };
    let half = w.thickness * 0.5;
    // For a closed plan, `material` faces outward; for an open path it is on
    // the walker's left (+X walking toward -Z has left at -X... i.e. (dz,-dx)).
    let outer_left = !w.closed || area2(pts) > 0.;
    let (outer, inner, trim) = (material, w.inner_material.unwrap_or(material), w.trim_material.unwrap_or(material));
    let dir = |s: usize| -> Result<[f64; 2]> { let (a, c) = (pts[s], pts[(s + 1) % pts.len()]); let l = (c[0] - a[0]).phypot(c[1] - a[1]); if l < 1e-6 { return Err(Error::Invalid("wall path has a zero-length segment")); } Ok([(c[0] - a[0]) / l, (c[1] - a[1]) / l]) };
    let seg_len = |s: usize| { let (a, c) = (pts[s], pts[(s + 1) % pts.len()]); (c[0] - a[0]).phypot(c[1] - a[1]) };
    let left = |d: [f64; 2]| [d[1], -d[0]];
    let cross2 = |a: [f64; 2], c: [f64; 2]| a[0] * c[1] - a[1] * c[0];
    // Miter offset along this segment's direction where the offset line meets
    // the neighbouring segment's offset line at the shared joint.
    let miter = |d: [f64; 2], other: [f64; 2], offset: f64| -> f64 {
        let (nl, no) = (left(d), left(other));
        let delta = [(no[0] - nl[0]) * offset, (no[1] - nl[1]) * offset];
        let c = cross2(d, other);
        if c.abs() < 1e-9 { 0. } else { (cross2(delta, other) / c).clamp(-4. * w.thickness, 4. * w.thickness) }
    };
    let mut path_start = 0.;
    let mut opening_used = vec![false; w.openings.len()];
    for s in 0..segs {
        let d = dir(s)?; let len = seg_len(s);
        let p0 = pts[s];
        let has_prev = w.closed || s > 0; let has_next = w.closed || s + 1 < segs;
        let dp = if has_prev { dir((s + segs - 1) % segs)? } else { d };
        let dn = if has_next { dir((s + 1) % segs)? } else { d };
        let starts = [if has_prev { miter(d, dp, half) } else { 0. }, if has_prev { miter(d, dp, -half) } else { 0. }];
        let ends = [len + if has_next { miter(d, dn, half) } else { 0. }, len + if has_next { miter(d, dn, -half) } else { 0. }];
        // Openings on this segment, in segment-local along/height coordinates.
        let mut local: Vec<(usize, f64, f64, f64, f64)> = Vec::new();
        for (oi, o) in w.openings.iter().enumerate() {
            let (a0, a1) = (o.at - o.width * 0.5 - path_start, o.at + o.width * 0.5 - path_start);
            if a1 <= 0. || a0 >= len { continue; }
            let lo = starts[0].max(starts[1]).max(0.) + half * 0.5; let hi = ends[0].min(ends[1]).min(len) - half * 0.5;
            if a0 < lo || a1 > hi { return Err(Error::Invalid("wall opening crosses a corner or the wall end; move `at` or narrow it")); }
            if o.sill + o.height > w.height - 0.01 { return Err(Error::Invalid("wall opening must stay below the wall top (sill + height < height)")); }
            opening_used[oi] = true;
            local.push((oi, a0, a1, o.sill, o.sill + o.height));
        }
        for (i, a) in local.iter().enumerate() {
            for c in &local[i + 1..] { if a.1 < c.2 && c.1 < a.2 && a.3 < c.4 && c.3 < a.4 { return Err(Error::Invalid("wall openings overlap")); } }
        }
        let mut xs: Vec<f64> = local.iter().flat_map(|o| [o.1, o.2]).collect();
        xs.sort_by(f64::total_cmp); xs.dedup_by(|a, c| (*a - *c).abs() < 1e-7);
        let mut ys: Vec<f64> = vec![0., w.height];
        ys.extend(local.iter().flat_map(|o| [o.3, o.4]));
        ys.sort_by(f64::total_cmp); ys.dedup_by(|a, c| (*a - *c).abs() < 1e-7);
        let cols = |side: usize| -> Vec<f64> { let mut v = vec![starts[side]]; v.extend(&xs); v.push(ends[side]); v };
        let columns = [cols(0), cols(1)];
        let nx = columns[0].len(); let ny = ys.len();
        let normal3 = |sign: f64| { let n = left(d); [n[0] * sign, 0., n[1] * sign] };
        let ids: Vec<Vec<Vec<u32>>> = (0..2).map(|side| {
            let off = if side == 0 { half } else { -half };
            (0..nx).map(|k| (0..ny).map(|j| {
                let x = columns[side][k];
                b.vertex([p0[0] + d[0] * x + left(d)[0] * off, w.base + ys[j], p0[1] + d[1] * x + left(d)[1] * off])
            }).collect()).collect()
        }).collect();
        let open = |k: usize, j: usize| local.iter().any(|o| o.1 <= columns[0][k] + 1e-7 && columns[0][k + 1] <= o.2 + 1e-7 && o.3 <= ys[j] + 1e-7 && ys[j + 1] <= o.4 + 1e-7 && k > 0 && k + 1 < nx);
        let along = |p: [f64; 3]| path_start + (p[0] - p0[0]) * d[0] + (p[2] - p0[1]) * d[1];
        let across = |p: [f64; 3]| (p[0] - p0[0]) * left(d)[0] + (p[2] - p0[1]) * left(d)[1];
        let (left_mat, right_mat) = if outer_left { (outer, inner) } else { (inner, outer) };
        for k in 0..nx - 1 {
            for j in 0..ny - 1 {
                if open(k, j) { continue; }
                for (side, mat, sign) in [(0usize, left_mat, 1.), (1, right_mat, -1.)] {
                    let v = vec![ids[side][k][j], ids[side][k + 1][j], ids[side][k + 1][j + 1], ids[side][k][j + 1]];
                    b.oriented(v, normal3(sign), mat, &|p| [along(p) * sign, p[1]]);
                }
            }
        }
        let top = ny - 1;
        for k in 0..nx - 1 {
            b.oriented(vec![ids[0][k][top], ids[0][k + 1][top], ids[1][k + 1][top], ids[1][k][top]], [0., 1., 0.], trim, &|p| [along(p), across(p)]);
            if !open(k, 0) { b.oriented(vec![ids[0][k][0], ids[0][k + 1][0], ids[1][k + 1][0], ids[1][k][0]], [0., -1., 0.], trim, &|p| [along(p), across(p)]); }
        }
        let d3 = [d[0], 0., d[1]];
        for (k, sign) in [(0usize, -1.), (nx - 1, 1.)] {
            for j in 0..ny - 1 {
                b.oriented(vec![ids[0][k][j], ids[0][k][j + 1], ids[1][k][j + 1], ids[1][k][j]], mul(d3, sign), trim, &|p| [across(p), p[1]]);
            }
        }
        for &(oi, a0, a1, y0, y1) in &local {
            let ki = |x: f64| columns[0].iter().position(|c| (c - x).abs() < 1e-7).unwrap();
            let ji = |y: f64| ys.iter().position(|c| (c - y).abs() < 1e-7).unwrap();
            let (k0, k1, j0, j1) = (ki(a0), ki(a1), ji(y0), ji(y1));
            for j in j0..j1 {
                b.oriented(vec![ids[0][k0][j], ids[0][k0][j + 1], ids[1][k0][j + 1], ids[1][k0][j]], d3, trim, &|p| [across(p), p[1]]);
                b.oriented(vec![ids[0][k1][j], ids[0][k1][j + 1], ids[1][k1][j + 1], ids[1][k1][j]], mul(d3, -1.), trim, &|p| [across(p), p[1]]);
            }
            for k in k0..k1 {
                b.oriented(vec![ids[0][k][j1], ids[0][k + 1][j1], ids[1][k + 1][j1], ids[1][k][j1]], [0., -1., 0.], trim, &|p| [along(p), across(p)]);
                if j0 > 0 { b.oriented(vec![ids[0][k][j0], ids[0][k + 1][j0], ids[1][k + 1][j0], ids[1][k][j0]], [0., 1., 0.], trim, &|p| [along(p), across(p)]); }
            }
            let o = &w.openings[oi];
            if o.frame > 1e-6 {
                // Frame bars line the reveals; the glass pane sits mid-wall.
                let depth = if o.frame_depth > 1e-6 { o.frame_depth } else { w.thickness * 0.6 }.min(w.thickness * 1.5);
                let fr = o.frame.min((a1 - a0) * 0.3).min((y1 - y0) * 0.3);
                let frame = Frame { origin: [p0[0], w.base, p0[1]], u: d3, v: [0., 1., 0.], w: [left(d)[0], 0., left(d)[1]] };
                let mut bars = vec![(a0, a0 + fr, y0, y1), (a1 - fr, a1, y0, y1), (a0 + fr, a1 - fr, y1 - fr, y1)];
                if y0 > 1e-6 { bars.push((a0 + fr, a1 - fr, y0, y0 + fr)); }
                if o.mullions {
                    let (mx, my) = ((a0 + a1) * 0.5, (y0 + y1) * 0.5); let m = fr * 0.6;
                    bars.push((mx - m * 0.5, mx + m * 0.5, y0 + if y0 > 1e-6 { fr } else { 0. }, y1 - fr));
                    if y0 > 1e-6 { bars.push((a0 + fr, mx - m * 0.5, my - m * 0.5, my + m * 0.5)); bars.push((mx + m * 0.5, a1 - fr, my - m * 0.5, my + m * 0.5)); }
                }
                for (u0, u1, v0, v1) in bars {
                    if u1 - u0 > 1e-6 && v1 - v0 > 1e-6 { prism(b, &[[u0, v0], [u1, v0], [u1, v1], [u0, v1]], frame, -depth * 0.5, depth * 0.5, 0., 1, &|_, _| o.frame_material, o.frame_material)?; }
                }
                if let Some(glass) = o.glass {
                    let g = 0.012f64.min(depth * 0.3);
                    let bottom = if y0 > 1e-6 { y0 + fr } else { y0 };
                    prism(b, &[[a0 + fr, bottom], [a1 - fr, bottom], [a1 - fr, y1 - fr], [a0 + fr, y1 - fr]], frame, -g * 0.5, g * 0.5, 0., 1, &|_, _| glass, glass)?;
                }
            } else if let Some(glass) = o.glass {
                let frame = Frame { origin: [p0[0], w.base, p0[1]], u: d3, v: [0., 1., 0.], w: [left(d)[0], 0., left(d)[1]] };
                prism(b, &[[a0, y0], [a1, y0], [a1, y1], [a0, y1]], frame, -0.006, 0.006, 0., 1, &|_, _| glass, glass)?;
            }
        }
        path_start += len;
    }
    if opening_used.iter().any(|u| !u) { return Err(Error::Invalid("wall opening `at` lies beyond the wall path")); }
    Ok(())
}
fn stair(b: &mut Builder, from: [f64; 3], to: [f64; 3], width: f64, steps: usize, thickness: f64, material: u32) -> Result<()> {
    let run = (to[0] - from[0]).phypot(to[2] - from[2]);
    let rise = to[1] - from[1];
    let u = [(to[0] - from[0]) / run, 0., (to[2] - from[2]) / run];
    let (g, r) = (run / steps as f64, rise / steps as f64);
    // One convex block per step, from the stringer underside (parallel to
    // the pitch, `thickness` below the nosings) up to its tread. Blocks meet
    // back to back; no long collinear sawtooth polygon has to be clipped.
    let slope = rise / run;
    let under = |x: f64| (x * slope + r - thickness * (1. + slope * slope).sqrt()).max(0.);
    let frame = Frame { origin: from, u, v: [0., 1., 0.], w: cross(u, [0., 1., 0.]) };
    for i in 0..steps {
        let (x0, x1, top) = (i as f64 * g, (i + 1) as f64 * g, (i + 1) as f64 * r);
        let (u0, u1) = (under(x0).min(top - r * 0.5), under(x1).min(top - r * 0.5));
        prism(b, &[[x0, u0], [x1, u1], [x1, top], [x0, top]], frame, -width * 0.5, width * 0.5, 0., 1, &|_, _| material, material)?;
    }
    Ok(())
}
fn roof(b: &mut Builder, r: &Roof, material: u32) -> Result<()> {
    let trim = r.trim_material.unwrap_or(material);
    let slope = r.pitch.to_radians().ptan();
    match r.kind {
        RoofKind::Flat => {
            let (hx, hz) = (r.size[0] * 0.5 + r.overhang, r.size[1] * 0.5 + r.overhang);
            let frame = Frame { origin: r.center, u: [1., 0., 0.], v: [0., 0., 1.], w: [0., 1., 0.] };
            prism(b, &[[-hx, -hz], [hx, -hz], [hx, hz], [-hx, hz]], frame, 0., r.thickness, 0., 1, &|_, _| trim, material)
        }
        RoofKind::Gable => {
            let (across, along) = if r.ridge == 0 { (r.size[1] * 0.5, r.size[0] * 0.5) } else { (r.size[0] * 0.5, r.size[1] * 0.5) };
            let span = across + r.overhang;
            let rise = across * slope; let eave = -r.overhang * slope;
            let vt = r.thickness / r.pitch.to_radians().pcos();
            let (u, w) = if r.ridge == 0 { ([0., 0., 1.], [1., 0., 0.]) } else { ([1., 0., 0.], [0., 0., 1.]) };
            let frame = Frame { origin: r.center, u, v: [0., 1., 0.], w };
            let profile = [[-span, eave], [0., rise], [span, eave], [span, eave + vt], [0., rise + vt], [-span, eave + vt]];
            // Only the two upper slope edges carry the roofing; soffit and
            // fascia take the trim.
            let top = |a: [f64; 2], c: [f64; 2]| { let line = |p: [f64; 2]| eave + (span - p[0].abs()) * slope; a[1] > line(a) + vt * 0.5 && c[1] > line(c) + vt * 0.5 };
            prism(b, &profile, frame, -(along + r.overhang), along + r.overhang, 0., 1, &|a, c| if top(a, c) { material } else { trim }, trim)?;
            if let Some(gable) = r.gable_material {
                let gt = if r.gable_thickness > 1e-6 { r.gable_thickness } else { 0.2 };
                for side in [-1., 1.] {
                    let c = side * (along - gt * 0.5);
                    prism(b, &[[-across, 0.], [across, 0.], [0., rise]], frame, c - gt * 0.5, c + gt * 0.5, 0., 1, &|_, _| gable, gable)?;
                }
            }
            Ok(())
        }
        RoofKind::Hip => {
            // Long side along the ridge; flat soffit, vertical fascia, four slopes.
            let (ax, az) = (r.size[0] * 0.5 + r.overhang, r.size[1] * 0.5 + r.overhang);
            let long_x = ax >= az;
            let (a, l) = if long_x { (az, ax) } else { (ax, az) };
            let y0 = r.center[1] - r.overhang * slope; let y1 = y0 + r.thickness; let ridge_y = y1 + a * slope; let half_ridge = (l - a).max(0.);
            let at = |along: f64, across: f64, y: f64| if long_x { [r.center[0] + along, y, r.center[2] + across] } else { [r.center[0] + across, y, r.center[2] + along] };
            let bottom = [at(-l, -a, y0), at(l, -a, y0), at(l, a, y0), at(-l, a, y0)].map(|p| b.vertex(p));
            let top = [at(-l, -a, y1), at(l, -a, y1), at(l, a, y1), at(-l, a, y1)].map(|p| b.vertex(p));
            let centroid = [r.center[0], (y0 + ridge_y) * 0.5, r.center[2]];
            let out = |b: &Builder, v: &[u32]| { let c = mul(v.iter().fold([0.; 3], |s, &i| add(s, b.positions[i as usize])), 1. / v.len() as f64); sub(c, centroid) };
            b.planar(bottom.to_vec(), [0., -1., 0.], trim);
            for i in 0..4 { let j = (i + 1) % 4; let v = vec![bottom[i], bottom[j], top[j], top[i]]; let n = out(b, &v); b.planar(v, [n[0], 0., n[2]], trim); }
            if half_ridge > 1e-6 {
                let (r0, r1) = (b.vertex(at(-half_ridge, 0., ridge_y)), b.vertex(at(half_ridge, 0., ridge_y)));
                for v in [vec![top[0], top[1], r1, r0], vec![top[2], top[3], r0, r1], vec![top[1], top[2], r1], vec![top[3], top[0], r0]] { let n = out(b, &v); b.planar(v, n, material); }
            } else {
                let apex = b.vertex(at(0., 0., ridge_y));
                for i in 0..4 { let v = vec![top[i], top[(i + 1) % 4], apex]; let n = out(b, &v); b.planar(v, n, material); }
            }
            Ok(())
        }
    }
}

pub(super) fn write_shape(w:&mut Writer,value:&Shape)->Result<()>{match value{
    Shape::Box{size,radius,segments}=>{w.u8(0)?;for item in size{w.f64(*item)?;}w.f64(*radius)?;w.u32(*segments)?;},
    Shape::Capsule{radius,height,segments,rings}=>{w.u8(1)?;w.f64(*radius)?;w.f64(*height)?;w.u32(*segments)?;w.u32(*rings)?;},
    Shape::Cone{bottom,top,height,segments,smooth}=>{w.u8(2)?;w.f64(*bottom)?;w.f64(*top)?;w.f64(*height)?;w.u32(*segments)?;w.u8(*smooth as u8)?;},
    Shape::Torus{radius,tube,segments,sides}=>{w.u8(3)?;w.f64(*radius)?;w.f64(*tube)?;w.u32(*segments)?;w.u32(*sides)?;},
    Shape::Wedge{size}=>{w.u8(4)?;for item in size{w.f64(*item)?;}},
    Shape::Extrude{profile,depth,axis,center,corner_radius,corner_segments,bevel,bevel_segments}=>{w.u8(5)?;w.count(profile.len())?;for item in profile{for item in item{w.f64(*item)?;}}w.f64(*depth)?;w.u32((*axis).try_into().map_err(|_|Error::Budget("count"))?)?;w.u8(*center as u8)?;w.f64(*corner_radius)?;w.u32(*corner_segments)?;w.f64(*bevel)?;w.u32(*bevel_segments)?;},
    Shape::Tube{path,radius,segments,caps}=>{w.u8(6)?;w.count(path.len())?;for item in path{for item in item{w.f64(*item)?;}}w.f64(*radius)?;w.u32(*segments)?;w.u8(*caps as u8)?;},
    Shape::Wall(value)=>{w.u8(7)?;write_wall(w,value)?;},
    Shape::Stair{from,to,width,steps,thickness}=>{w.u8(8)?;for item in from{w.f64(*item)?;}for item in to{w.f64(*item)?;}w.f64(*width)?;w.u32(*steps)?;w.f64(*thickness)?;},
    Shape::Roof(value)=>{w.u8(9)?;write_roof(w,value)?;},
}Ok(())}
pub(super) fn read_shape(r:&mut Reader,limits:&Limits)->Result<Shape>{let _=limits;Ok(match r.u8()?{
    0=>Shape::Box{size:[r.f64()?,r.f64()?,r.f64()?],radius:r.f64()?,segments:r.u32()?},
    1=>Shape::Capsule{radius:r.f64()?,height:r.f64()?,segments:r.u32()?,rings:r.u32()?},
    2=>Shape::Cone{bottom:r.f64()?,top:r.f64()?,height:r.f64()?,segments:r.u32()?,smooth:read_bool(r)?},
    3=>Shape::Torus{radius:r.f64()?,tube:r.f64()?,segments:r.u32()?,sides:r.u32()?},
    4=>Shape::Wedge{size:[r.f64()?,r.f64()?,r.f64()?]},
    5=>Shape::Extrude{profile:{let mut values=Vec::new();for _ in 0..r.count(MAX_PROFILE)?{values.push([r.f64()?,r.f64()?]);}values},depth:r.f64()?,axis:r.u32()? as usize,center:read_bool(r)?,corner_radius:r.f64()?,corner_segments:r.u32()?,bevel:r.f64()?,bevel_segments:r.u32()?},
    6=>Shape::Tube{path:{let mut values=Vec::new();for _ in 0..r.count(MAX_PROFILE)?{values.push([r.f64()?,r.f64()?,r.f64()?]);}values},radius:r.f64()?,segments:r.u32()?,caps:read_bool(r)?},
    7=>Shape::Wall(read_wall(r,limits)?),
    8=>Shape::Stair{from:[r.f64()?,r.f64()?,r.f64()?],to:[r.f64()?,r.f64()?,r.f64()?],width:r.f64()?,steps:r.u32()?,thickness:r.f64()?},
    9=>Shape::Roof(read_roof(r,limits)?),
    _=>return Err(Error::Corrupt("Shape tag")),
})}
fn write_roofkind(w:&mut Writer,value:&RoofKind)->Result<()>{match value{
    RoofKind::Gable=>{w.u8(0)?;},
    RoofKind::Hip=>{w.u8(1)?;},
    RoofKind::Flat=>{w.u8(2)?;},
}Ok(())}
fn read_roofkind(r:&mut Reader,limits:&Limits)->Result<RoofKind>{let _=limits;Ok(match r.u8()?{
    0=>RoofKind::Gable,
    1=>RoofKind::Hip,
    2=>RoofKind::Flat,
    _=>return Err(Error::Corrupt("RoofKind tag")),
})}
fn write_opening(w:&mut Writer,value:&Opening)->Result<()>{w.f64(value.at)?;w.f64(value.width)?;w.f64(value.height)?;w.f64(value.sill)?;w.f64(value.frame)?;w.f64(value.frame_depth)?;w.u32(value.frame_material)?;if let Some(item)=&value.glass{w.u8(1)?;w.u32(*item)?;}else{w.u8(0)?;}w.u8(value.mullions as u8)?;Ok(())}
fn read_opening(r:&mut Reader,limits:&Limits)->Result<Opening>{let _=limits;Ok(Opening{at:r.f64()?,width:r.f64()?,height:r.f64()?,sill:r.f64()?,frame:r.f64()?,frame_depth:r.f64()?,frame_material:r.u32()?,glass:if read_bool(r)?{Some(r.u32()?)}else{None},mullions:read_bool(r)?})}
fn write_wall(w:&mut Writer,value:&Wall)->Result<()>{w.count(value.path.len())?;for item in &value.path{for item in item{w.f64(*item)?;}}w.u8(value.closed as u8)?;w.f64(value.height)?;w.f64(value.thickness)?;w.f64(value.base)?;w.count(value.openings.len())?;for item in &value.openings{write_opening(w,item)?;}if let Some(item)=&value.inner_material{w.u8(1)?;w.u32(*item)?;}else{w.u8(0)?;}if let Some(item)=&value.trim_material{w.u8(1)?;w.u32(*item)?;}else{w.u8(0)?;}Ok(())}
fn read_wall(r:&mut Reader,limits:&Limits)->Result<Wall>{let _=limits;Ok(Wall{path:{let mut values=Vec::new();for _ in 0..r.count(MAX_PROFILE)?{values.push([r.f64()?,r.f64()?]);}values},closed:read_bool(r)?,height:r.f64()?,thickness:r.f64()?,base:r.f64()?,openings:{let mut values=Vec::new();for _ in 0..r.count(MAX_OPENINGS)?{values.push(read_opening(r,limits)?);}values},inner_material:if read_bool(r)?{Some(r.u32()?)}else{None},trim_material:if read_bool(r)?{Some(r.u32()?)}else{None}})}
fn write_roof(w:&mut Writer,value:&Roof)->Result<()>{write_roofkind(w,&value.kind)?;for item in &value.center{w.f64(*item)?;}for item in &value.size{w.f64(*item)?;}w.f64(value.pitch)?;w.f64(value.overhang)?;w.f64(value.thickness)?;w.u32((value.ridge).try_into().map_err(|_|Error::Budget("count"))?)?;if let Some(item)=&value.trim_material{w.u8(1)?;w.u32(*item)?;}else{w.u8(0)?;}if let Some(item)=&value.gable_material{w.u8(1)?;w.u32(*item)?;}else{w.u8(0)?;}w.f64(value.gable_thickness)?;Ok(())}
fn read_roof(r:&mut Reader,limits:&Limits)->Result<Roof>{let _=limits;Ok(Roof{kind:read_roofkind(r,limits)?,center:[r.f64()?,r.f64()?,r.f64()?],size:[r.f64()?,r.f64()?],pitch:r.f64()?,overhang:r.f64()?,thickness:r.f64()?,ridge:r.u32()? as usize,trim_material:if read_bool(r)?{Some(r.u32()?)}else{None},gable_material:if read_bool(r)?{Some(r.u32()?)}else{None},gable_thickness:r.f64()?})}

impl Shape {
    pub fn check(&self,_limits:&Limits)->Result<()>{
        match self{
            Self::Box{size,radius,segments}=>{finite(*size)?;for s in size{positive(*s,"box size must be positive")?;}finite([*radius])?;if *radius<0.{return Err(Error::Invalid("box radius"));}in_range(*segments,1,16,"box segments 1..16")?;},
            Self::Capsule{radius,height,segments,rings}=>{finite([*radius,*height])?;positive(*radius,"capsule radius")?;if *height<radius*2.-1e-9{return Err(Error::Invalid("capsule height must be at least twice its radius"));}in_range(*segments,3,128,"capsule segments")?;in_range(*rings,1,64,"capsule rings")?;},
            Self::Cone{bottom,top,height,segments,..}=>{finite([*bottom,*top,*height])?;if *bottom<0.||*top<0.||bottom+top<=0.{return Err(Error::Invalid("cone radii"));}positive(*height,"cone height")?;in_range(*segments,3,128,"cone segments")?;},
            Self::Torus{radius,tube,segments,sides}=>{finite([*radius,*tube])?;positive(*radius,"torus radius")?;positive(*tube,"torus tube")?;if tube>=radius{return Err(Error::Invalid("torus tube must be thinner than its radius"));}in_range(*segments,3,128,"torus segments")?;in_range(*sides,3,64,"torus sides")?;},
            Self::Wedge{size}=>{finite(*size)?;for s in size{positive(*s,"wedge size must be positive")?;}},
            Self::Extrude{profile,depth,axis,corner_radius,corner_segments,bevel,bevel_segments,..}=>{check_points2(profile)?;if *axis>2{return Err(Error::Invalid("extrude axis must be 0, 1 or 2"));}finite([*depth,*corner_radius,*bevel])?;if *corner_radius<0.||*bevel<0.{return Err(Error::Invalid("extrude corner_radius/bevel"));}positive(*depth,"extrude depth")?;in_range(*corner_segments,1,32,"extrude corner_segments")?;in_range(*bevel_segments,1,16,"extrude bevel_segments")?;},
            Self::Tube{path,radius,segments,..}=>{if !(2..=MAX_PROFILE).contains(&path.len()){return Err(Error::Invalid("tube path needs 2..512 points"));}finite(path.iter().flatten().copied())?;finite([*radius])?;positive(*radius,"tube radius")?;in_range(*segments,3,64,"tube segments")?;},
            Self::Wall(w)=>{check_points2(&w.path)?;if w.path.len()<if w.closed{3}else{2}{return Err(Error::Invalid("wall path needs two points (three when closed)"));}finite([w.height,w.thickness,w.base])?;positive(w.height,"wall height")?;positive(w.thickness,"wall thickness")?;
                if w.openings.len()>MAX_OPENINGS{return Err(Error::Invalid("wall openings must be an array of at most 64"));}
                for o in &w.openings{finite([o.at,o.width,o.height,o.sill,o.frame,o.frame_depth])?;if o.sill<0.||o.frame<0.||o.frame_depth<0.{return Err(Error::Invalid("opening sill/frame"));}positive(o.width,"opening width")?;positive(o.height,"opening height")?;}
            },
            Self::Stair{from,to,width,steps,thickness}=>{finite(*from)?;finite(*to)?;let rise=to[1]-from[1];if !(rise.is_finite()&&rise>0.05)||(to[0]-from[0]).phypot(to[2]-from[2])<0.1{return Err(Error::Invalid("stair must climb from `from` up to a higher, horizontally distant `to`"));}finite([*width,*thickness])?;positive(*width,"stair width")?;in_range(*steps,1,256,"stair steps 1..256")?;positive(*thickness,"stair thickness")?;},
            Self::Roof(r)=>{finite(r.center)?;finite(r.size)?;for s in r.size{positive(s,"roof size")?;}finite([r.pitch,r.overhang,r.thickness,r.gable_thickness])?;
                if !(0. ..=75.).contains(&r.pitch)||(r.kind!=RoofKind::Flat&&r.pitch<1.){return Err(Error::Invalid("roof pitch must be 1..75 degrees"));}if r.overhang<0.{return Err(Error::Invalid("roof overhang"));}if r.ridge!=0&&r.ridge!=2{return Err(Error::Invalid("roof ridge must be \"x\" or \"z\""));}positive(r.thickness,"roof thickness")?;if r.gable_thickness<0.{return Err(Error::Invalid("roof gable_thickness"));}
            },
        }Ok(())
    }
}
fn check_points2(v:&[[f64;2]])->Result<()>{if !(2..=MAX_PROFILE).contains(&v.len()){return Err(Error::Invalid("2D point list needs 2..512 points"));}finite(v.iter().flatten().copied())}

#[cfg(test)]
mod typed_tests {
    use super::*;
    #[test]
    fn shape_literals_cover_variants_and_optional_fields(){
        let limits=Limits::default();let path=vec![[0.,0.],[2.,0.],[2.,2.]];
        let mut shapes=vec![
            Shape::Box{size:[1.,2.,3.],radius:0.1,segments:3},Shape::Capsule{radius:0.5,height:2.,segments:24,rings:6},
            Shape::Cone{bottom:1.,top:0.,height:2.,segments:16,smooth:true},Shape::Torus{radius:2.,tube:0.25,segments:32,sides:12},Shape::Wedge{size:[1.;3]},
            Shape::Extrude{profile:path.clone(),depth:0.5,axis:2,center:true,corner_radius:0.1,corner_segments:4,bevel:0.01,bevel_segments:3},
            Shape::Tube{path:vec![[0.;3],[1.,0.,0.]],radius:0.1,segments:12,caps:true},
            Shape::Wall(Wall{path:path.clone(),closed:true,height:3.,thickness:0.2,base:0.1,openings:vec![Opening{at:0.5,width:0.5,height:1.,sill:0.3,frame:0.05,frame_depth:0.1,frame_material:2,glass:Some(3),mullions:true}],inner_material:Some(4),trim_material:None}),
            Shape::Wall(Wall{path,closed:false,height:3.,thickness:0.2,base:0.,openings:vec![Opening{at:0.5,width:0.5,height:1.,sill:0.,frame:0.,frame_depth:0.,frame_material:0,glass:None,mullions:false}],inner_material:None,trim_material:Some(4)}),
            Shape::Stair{from:[0.;3],to:[1.,1.,0.],width:1.,steps:6,thickness:0.15},
        ];for kind in [RoofKind::Gable,RoofKind::Hip,RoofKind::Flat]{shapes.push(Shape::Roof(Roof{kind,center:[0.,3.,0.],size:[4.,6.],pitch:35.,overhang:0.3,thickness:0.12,ridge:2,trim_material:Some(2),gable_material:Some(3),gable_thickness:0.2}));}
        for shape in shapes{shape.check(&limits).unwrap();let mut w=Writer::new(usize::MAX);write_shape(&mut w,&shape).unwrap();let mut r=Reader::new(&w.bytes);let decoded=read_shape(&mut r,&limits).unwrap();decoded.check(&limits).unwrap();assert_eq!(decoded,shape);r.end().unwrap();}
    }
    #[test]
    fn invalid_shape_literals(){
        let limits=Limits::default();for shape in [
            Shape::Box{size:[1.;3],radius:-1.,segments:3},Shape::Capsule{radius:1.,height:1.,segments:24,rings:6},Shape::Cone{bottom:0.,top:0.,height:1.,segments:12,smooth:false},
            Shape::Torus{radius:1.,tube:1.,segments:12,sides:8},Shape::Wedge{size:[1.,f64::INFINITY,1.]},
            Shape::Extrude{profile:vec![[0.;2];3],depth:1.,axis:3,center:false,corner_radius:0.,corner_segments:4,bevel:0.,bevel_segments:3},
            Shape::Tube{path:vec![],radius:1.,segments:12,caps:false},
            Shape::Wall(Wall{path:vec![[0.;2];2],closed:true,height:3.,thickness:0.2,base:0.,openings:vec![],inner_material:None,trim_material:None}),
            Shape::Stair{from:[0.;3],to:[0.;3],width:1.,steps:1,thickness:0.2},
            Shape::Roof(Roof{kind:RoofKind::Gable,center:[0.;3],size:[2.;2],pitch:0.,overhang:0.3,thickness:0.1,ridge:0,trim_material:None,gable_material:None,gable_thickness:0.}),
        ]{assert!(shape.check(&limits).is_err());}
        assert!(matches!(read_shape(&mut Reader::new(&[255]),&limits),Err(Error::Corrupt("Shape tag"))));
    }
}
