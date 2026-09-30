//! Mesh building blocks for the character generator: a weighted polygon
//! soup that exports as one `ImportMesh`, ring tubes (limbs, torso, hair
//! locks, clothing shells) and deformed spheres (heads, eyes, pads).
//!
//! Every generator surface is built from rings: a ring is a closed cross
//! section in a local frame, and a tube joins successive rings with quads.
//! Weights are per vertex, so a clothing shell built from the same rings as
//! the body underneath deforms with it exactly.
use makepad_csg_math::portable::PortableFloat;
use crate::{mesh, transform::*, Error, Operation, Result};
use std::f64::consts::TAU;

pub(crate) type W = Vec<mesh::JointWeight>;
pub(crate) fn w1(j: u32) -> W { vec![mesh::JointWeight { joint: j, weight: 1. }] }
/// Blend two weight sets (t = 0 → a, 1 → b), merging equal joints.
pub(crate) fn wmix(a: &W, b: &W, t: f64) -> W {
    let t = t.clamp(0., 1.);
    let mut out: W = Vec::new();
    for (list, k) in [(a, 1. - t), (b, t)] {
        if k <= 1e-6 { continue; }
        for x in list {
            match out.iter_mut().find(|o| o.joint == x.joint) { Some(o) => o.weight += x.weight * k, None => out.push(mesh::JointWeight { joint: x.joint, weight: x.weight * k }) }
        }
    }
    prune(out)
}
/// Keep the four strongest influences, normalized.
pub(crate) fn prune(mut w: W) -> W {
    w.retain(|x| x.weight > 1e-4);
    w.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    w.truncate(4);
    let s: f64 = w.iter().map(|x| x.weight).sum();
    if s > 0. { for x in &mut w { x.weight /= s; } }
    w.sort_by_key(|x| x.joint);
    w
}
pub(crate) fn smooth01(a: f64, b: f64, x: f64) -> f64 { let t = ((x - a) / (b - a)).clamp(0., 1.); t * t * (3. - 2. * t) }
pub(crate) fn mix(a: f64, b: f64, t: f64) -> f64 { a + (b - a) * t }
pub(crate) fn lerp3(a: [f64; 3], b: [f64; 3], t: f64) -> [f64; 3] { std::array::from_fn(|i| mix(a[i], b[i], t)) }
pub(crate) fn norm(v: [f64; 3]) -> [f64; 3] { let l = length(v); if l < 1e-12 { [0., 1., 0.] } else { mul(v, 1. / l) } }

/// One exported object under construction.
pub(crate) struct Part {
    pub name: String,
    pub positions: Vec<[f64; 3]>,
    pub weights: Vec<W>,
    pub colors: Vec<[f64; 4]>,
    pub polys: Vec<mesh::Polygon>,
    /// Corner normals are split where adjacent faces differ by more than
    /// this angle (radians); PI shades everything smooth.
    pub crease: f64,
}
impl Part {
    pub fn new(name: &str) -> Self { Self { name: name.into(), positions: Vec::new(), weights: Vec::new(), colors: Vec::new(), polys: Vec::new(), crease: std::f64::consts::PI } }
    pub fn hard(mut self, degrees: f64) -> Self { self.crease = degrees.to_radians(); self }
    pub fn vertex(&mut self, p: [f64; 3], w: W) -> u32 { self.positions.push(p); self.weights.push(w); self.colors.push([1.; 4]); (self.positions.len() - 1) as u32 }
    pub fn poly(&mut self, v: Vec<u32>, uvs: Vec<[f64; 2]>, material: u32) {
        if v.len() < 3 { return; }
        // Drop faces collapsed to a line (pole fans can produce them).
        let p: Vec<[f64; 3]> = v.iter().map(|&i| self.positions[i as usize]).collect();
        let mut n = [0.; 3];
        for i in 1..p.len() - 1 { n = add(n, cross(sub(p[i], p[0]), sub(p[i + 1], p[0]))); }
        if length(n) < 1e-14 { return; }
        // A strongly twisted quad (tight lock bends, posed shells) would be
        // a bow-tie face; split it into two triangles instead.
        if v.len() == 4 {
            let t1 = cross(sub(p[1], p[0]), sub(p[2], p[0]));
            let t2 = cross(sub(p[2], p[0]), sub(p[3], p[0]));
            if length(t1) < 1e-14 || length(t2) < 1e-14 || dot(norm(t1), norm(t2)) < 0.6 {
                let uv = |i: usize| uvs.get(i).copied().unwrap_or([0.; 2]);
                for (a, b, c) in [(0, 1, 2), (0, 2, 3)] {
                    let q = [p[a], p[b], p[c]];
                    if length(cross(sub(q[1], q[0]), sub(q[2], q[0]))) < 1e-14 { continue; }
                    self.polys.push(mesh::Polygon { vertices: vec![v[a], v[b], v[c]], uvs: if uvs.is_empty() { Vec::new() } else { vec![uv(a), uv(b), uv(c)] }, material });
                }
                return;
            }
        }
        self.polys.push(mesh::Polygon { vertices: v, uvs, material });
    }
    pub fn is_empty(&self) -> bool { self.polys.is_empty() }
    /// Several parts as one editable object; each part keeps its own
    /// crease rule for normals.
    pub fn export_merged(name: &str, parts: &[Part], limits: &crate::Limits) -> Result<(Operation, Vec<crate::json::Value>)> {
        let mut all = Part::new(name);
        let mut normals: Vec<[f64; 3]> = Vec::new();
        for p in parts.iter().filter(|p| !p.is_empty()) {
            let base = all.positions.len() as u32;
            normals.extend(p.corner_normals());
            all.positions.extend_from_slice(&p.positions); all.weights.extend(p.weights.iter().cloned()); all.colors.extend_from_slice(&p.colors);
            all.polys.extend(p.polys.iter().map(|q| mesh::Polygon { vertices: q.vertices.iter().map(|v| v + base).collect(), uvs: q.uvs.clone(), material: q.material }));
        }
        all.export_with(Some(normals), limits)
    }
    /// Area-weighted corner normals, split at the crease angle, in polygon order.
    pub fn corner_normals(&self) -> Vec<[f64; 3]> {
        let face_n: Vec<[f64; 3]> = self.polys.iter().map(|p| { let q: Vec<[f64; 3]> = p.vertices.iter().map(|&i| self.positions[i as usize]).collect(); let mut n = [0.; 3]; for i in 1..q.len() - 1 { n = add(n, cross(sub(q[i], q[0]), sub(q[i + 1], q[0]))); } n }).collect();
        let mut adjacent: Vec<Vec<usize>> = vec![Vec::new(); self.positions.len()];
        for (f, p) in self.polys.iter().enumerate() { for &v in &p.vertices { adjacent[v as usize].push(f); } }
        let cos = self.crease.pcos();
        let smooth = self.crease >= std::f64::consts::PI - 1e-6;
        let mut out = Vec::new();
        for (f, p) in self.polys.iter().enumerate() {
            let own = norm(face_n[f]);
            for &v in &p.vertices {
                let mut n = [0.; 3];
                for &g in &adjacent[v as usize] { if g == f || smooth || dot(norm(face_n[g]), own) >= cos { n = add(n, face_n[g]); } }
                out.push(if length(n) < 1e-14 { own } else { norm(n) });
            }
        }
        out
    }
    /// An unskinned object (every vertex unbound).
    pub fn export_static(&self, limits: &crate::Limits) -> Result<(Operation, Vec<crate::json::Value>)> { self.export_with(None, limits) }
    /// The editable mesh plus the vertex-paint ops that carry `colors`.
    fn export_with(&self, normals: Option<Vec<[f64; 3]>>, limits: &crate::Limits) -> Result<(Operation, Vec<crate::json::Value>)> {
        if self.polys.is_empty() { return Err(Error::Invalid("character part has no faces")); }
        // Only referenced vertices are exported; unreferenced ones would
        // be loose points in the editable mesh.
        let mut used = vec![u32::MAX; self.positions.len()];
        let (mut positions, mut weights, mut colors) = (Vec::new(), Vec::new(), Vec::new());
        for p in &self.polys { for &v in &p.vertices { if used[v as usize] == u32::MAX { used[v as usize] = positions.len() as u32; positions.push(self.positions[v as usize]); weights.push(self.weights[v as usize].clone()); colors.push(self.colors[v as usize]); } } }
        let polys: Vec<mesh::Polygon> = self.polys.iter().map(|p| mesh::Polygon { vertices: p.vertices.iter().map(|&v| used[v as usize]).collect(), uvs: p.uvs.clone(), material: p.material }).collect();
        let corner_normals = normals.unwrap_or_else(|| self.corner_normals());
        let mut ctx = mesh::Context::new(limits.mesh.clone(), None);
        let mut m = mesh::Mesh::from_weighted_polygons(&positions, &weights, &polys, &mut ctx)?;
        let values: Vec<_> = m.corners().iter().zip(&corner_normals).map(|(c, n)| (c.id, Some(*n))).collect();
        m.set_corner_normals_bulk(&values, &mut ctx)?;
        // Vertex IDs are assigned in input order starting at 1.
        let ids: Vec<u64> = m.vertices().iter().map(|v| v.id.0).collect();
        // One bulk colour op for every vertex, white ones included: a paint
        // op per distinct colour cost ~25 ms each on the document, and a
        // vertex without an entry is later filled in by a closest-point
        // search at compile (seconds for a character).
        let (mut vids, mut cols) = (Vec::new(), Vec::new());
        for (i, c) in colors.iter().enumerate() {
            vids.push(crate::json::s(ids[i].to_string()));
            cols.push(crate::json::Value::Arr(c.iter().map(|v| crate::json::Value::F64(v.clamp(0., 1.))).collect()));
        }
        let paint = if vids.is_empty() { Vec::new() } else {
            vec![crate::json::obj(vec![("op", crate::json::s("surface_vertex_colors")), ("object", crate::json::s(&self.name)), ("vertices", crate::json::Value::Arr(vids)), ("colors", crate::json::Value::Arr(cols))])]
        };
        Ok((Operation::ImportMesh { object: self.name.clone(), source: m.to_bytes(&mut ctx)? }, paint))
    }
}

/// A closed cross section. The ring lies in the plane spanned by `x` and
/// `z` through `c`; angle 0 points along +x, PI/2 along +z. Radii may
/// differ on each half-axis, and `exp` squares the section off (2 = ellipse).
#[derive(Clone, Debug)]
pub(crate) struct Ring {
    pub c: [f64; 3],
    pub x: [f64; 3],
    pub z: [f64; 3],
    pub rx: [f64; 2],
    pub rz: [f64; 2],
    pub exp: f64,
    pub w: W,
    /// Extra radial offset as a function of angle, for sculpted details.
    pub bumps: Vec<(f64, f64, f64)>,
}
impl Ring {
    pub fn new(c: [f64; 3], x: [f64; 3], z: [f64; 3], rx: f64, rz: f64, w: W) -> Self { Self { c, x, z, rx: [rx; 2], rz: [rz; 2], exp: 2., w, bumps: Vec::new() } }
    /// A ring perpendicular to `axis`, with `side` a hint for the x direction.
    pub fn around(c: [f64; 3], axis: [f64; 3], side: [f64; 3], rx: f64, rz: f64, w: W) -> Self {
        let a = norm(axis);
        let x = norm(sub(side, mul(a, dot(side, a))));
        let z = cross(a, x);
        Self::new(c, x, z, rx, rz, w)
    }
    pub fn point(&self, theta: f64) -> [f64; 3] {
        let (c, s) = (theta.pcos(), theta.psin());
        let e = 2. / self.exp;
        let px = c.signum() * c.abs().ppowf(e) * if c >= 0. { self.rx[0] } else { self.rx[1] };
        let pz = s.signum() * s.abs().ppowf(e) * if s >= 0. { self.rz[0] } else { self.rz[1] };
        let mut p = add(self.c, add(mul(self.x, px), mul(self.z, pz)));
        if !self.bumps.is_empty() {
            let mut d = 0.;
            for &(at, width, h) in &self.bumps { let mut da = (theta - at).rem_euclid(TAU); if da > TAU / 2. { da -= TAU; } d += h * (-(da / width).powi(2)).pexp(); }
            p = add(p, mul(norm(sub(p, self.c)), d));
        }
        p
    }
    pub fn scaled(&self, k: f64, add_r: f64) -> Ring { let mut r = self.clone(); r.rx = r.rx.map(|v| v * k + add_r); r.rz = r.rz.map(|v| v * k + add_r); r }
    pub fn perimeter(&self) -> f64 { let n = 24; (0..n).map(|i| length(sub(self.point(TAU * (i + 1) as f64 / n as f64), self.point(TAU * i as f64 / n as f64)))).sum() }
}

#[derive(Clone, Copy)]
pub(crate) enum Cap { Open, Point([f64; 3]) }

/// Per-face material chooser: (ring index of the quad's first ring, angle).
pub(crate) type MatFn<'a> = &'a dyn Fn(usize, f64) -> u32;

/// Joins rings with quads; `segments` samples per ring. The winding is
/// chosen so faces point away from the ring centres. Returns the vertex
/// index grid (ring-major) so callers can post-process.
pub(crate) fn tube(part: &mut Part, rings: &[Ring], segments: usize, start: Cap, end: Cap, material: MatFn, uv_scale: f64, theta0: f64, weight_fn: Option<&dyn Fn([f64; 3], usize) -> W>) -> Vec<Vec<u32>> {
    if rings.len() < 2 { return Vec::new(); }
    let n = segments.max(3);
    let mut grid = Vec::with_capacity(rings.len());
    for (i, r) in rings.iter().enumerate() {
        grid.push((0..n).map(|k| { let p = r.point(theta0 + TAU * k as f64 / n as f64); let w = weight_fn.map_or_else(|| r.w.clone(), |f| f(p, i)); part.vertex(p, w) }).collect::<Vec<u32>>());
    }
    // Metre UVs: u around (average perimeter), v along the centre line.
    let perim = rings.iter().map(Ring::perimeter).sum::<f64>() / rings.len() as f64;
    let mut vs = vec![0.; rings.len()];
    for i in 1..rings.len() { vs[i] = vs[i - 1] + length(sub(rings[i].c, rings[i - 1].c)).max(1e-4); }
    // Orientation: sum of face normals against the radial direction.
    let mut score = 0.;
    for i in 0..rings.len() - 1 { for k in 0..n {
        let (a, b, c) = (part.positions[grid[i][k] as usize], part.positions[grid[i][(k + 1) % n] as usize], part.positions[grid[i + 1][k] as usize]);
        score += dot(cross(sub(b, a), sub(c, a)), sub(a, rings[i].c));
    } }
    let flip = score > 0.;
    let uv = |k: usize, i: usize| [perim * k as f64 / n as f64 * uv_scale, vs[i] * uv_scale];
    for i in 0..rings.len() - 1 {
        for k in 0..n {
            let k1 = k + 1;
            let theta = theta0 + TAU * (k as f64 + 0.5) / n as f64;
            let m = material(i, theta);
            let (mut v, mut t) = (vec![grid[i][k], grid[i][k1 % n], grid[i + 1][k1 % n], grid[i + 1][k]], vec![uv(k, i), uv(k1, i), uv(k1, i + 1), uv(k, i + 1)]);
            if !flip { v.reverse(); t.reverse(); }
            part.poly(v, t, m);
        }
    }
    let mut cap = |cap: Cap, i: usize, outward: [f64; 3]| {
        if let Cap::Point(p) = cap {
            let w = weight_fn.map_or_else(|| rings[i].w.clone(), |f| f(p, i));
            let centre = part.vertex(p, w);
            let theta = theta0;
            let m = material(if i == 0 { 0 } else { i - 1 }, theta);
            for k in 0..n {
                let (a, b) = (grid[i][k], grid[i][(k + 1) % n]);
                let (pa, pb) = (part.positions[a as usize], part.positions[b as usize]);
                let mut v = vec![a, b, centre];
                let nrm = cross(sub(pb, pa), sub(p, pa));
                if dot(nrm, outward) < 0. { v.reverse(); }
                let uvs: Vec<[f64; 2]> = v.iter().map(|&x| { let q = part.positions[x as usize]; let d = sub(q, p); [d[0] * uv_scale + 0.5, d[2] * uv_scale + 0.5] }).collect();
                part.poly(v, uvs, m);
            }
        }
    };
    let last = rings.len() - 1;
    cap(start, 0, sub(rings[0].c, rings[1].c));
    cap(end, last, sub(rings[last].c, rings[last - 1].c));
    grid
}

/// A deformed sphere: `f(dir)` maps a unit direction (pole along `axis`)
/// to a surface point. Rings run from the -axis pole to the +axis pole.
pub(crate) fn blob(part: &mut Part, centre: [f64; 3], axis: [f64; 3], side: [f64; 3], segments: usize, rings: usize, f: &dyn Fn([f64; 3]) -> [f64; 3], wf: &dyn Fn([f64; 3]) -> W, material: MatFn, uv_scale: f64) -> Vec<Vec<u32>> {
    let a = norm(axis);
    let x = norm(sub(side, mul(a, dot(side, a))));
    let z = cross(a, x);
    let dir = |phi: f64, theta: f64| add(add(mul(a, -phi.pcos()), mul(x, phi.psin() * theta.pcos())), mul(z, phi.psin() * theta.psin()));
    let n = segments.max(3);
    let mut grid: Vec<Vec<u32>> = Vec::new();
    let south = f(mul(a, -1.)); let north = f(a);
    let s_i = part.vertex(south, wf(south));
    for r in 1..rings {
        let phi = std::f64::consts::PI * r as f64 / rings as f64;
        grid.push((0..n).map(|k| { let p = f(dir(phi, TAU * k as f64 / n as f64)); part.vertex(p, wf(p)) }).collect());
    }
    let n_i = part.vertex(north, wf(north));
    let uv = |k: usize, r: usize| [k as f64 / n as f64 * uv_scale, r as f64 / rings as f64 * uv_scale];
    let _ = centre;
    // Faces: outward orientation by construction order; verify with the
    // first band and flip everything if needed.
    let mut faces: Vec<(Vec<u32>, Vec<[f64; 2]>, u32)> = Vec::new();
    for k in 0..n {
        let k1 = (k + 1) % n;
        let theta = TAU * (k as f64 + 0.5) / n as f64;
        faces.push((vec![s_i, grid[0][k1], grid[0][k]], vec![uv(k, 0), uv(k + 1, 1), uv(k, 1)], material(0, theta)));
        for r in 0..grid.len() - 1 {
            faces.push((vec![grid[r][k], grid[r][k1], grid[r + 1][k1], grid[r + 1][k]], vec![uv(k, r + 1), uv(k + 1, r + 1), uv(k + 1, r + 2), uv(k, r + 2)], material(r + 1, theta)));
        }
        let l = grid.len() - 1;
        faces.push((vec![grid[l][k], grid[l][k1], n_i], vec![uv(k, l + 1), uv(k + 1, l + 1), uv(k, rings)], material(rings - 1, theta)));
    }
    // Orientation check on the equator band.
    let mid = grid.len() / 2;
    let (p0, p1, p2) = (part.positions[grid[mid][0] as usize], part.positions[grid[mid][1] as usize], part.positions[grid[(mid + 1).min(grid.len() - 1)][0] as usize]);
    let c = mul(add(south, north), 0.5);
    let flip = dot(cross(sub(p1, p0), sub(p2, p0)), sub(p0, c)) < 0.;
    for (mut v, mut t, m) in faces { if flip { v.reverse(); t.reverse(); } part.poly(v, t, m); }
    let mut all = vec![vec![s_i]]; all.extend(grid); all.push(vec![n_i]);
    all
}

/// A superellipsoid (rounded box) with half extents `h` in the frame
/// (x, y, z) rotated by `rot`, bound rigidly. `round` 2 = ellipsoid,
/// larger values square it off.
pub(crate) fn rounded_box(part: &mut Part, c: [f64; 3], h: [f64; 3], rot: [[f64; 3]; 3], round: f64, w: W, material: u32, segments: usize) {
    let e = 2. / round;
    let sp = |v: f64| v.signum() * v.abs().ppowf(e);
    let f = |d: [f64; 3]| {
        let l = [sp(d[0]) * h[0], sp(d[1]) * h[1], sp(d[2]) * h[2]];
        add(c, add(add(mul(rot[0], l[0]), mul(rot[1], l[1])), mul(rot[2], l[2])))
    };
    let wf = |_: [f64; 3]| w.clone();
    let mat = |_: usize, _: f64| material;
    // The builder's local axis is world Y; rotate via f.
    blob(part, c, [0., 1., 0.], [1., 0., 0.], segments, (segments / 2).max(4), &f, &wf, &mat, 1.);
}
pub(crate) fn rot_y(deg: f64) -> [[f64; 3]; 3] { let (s, c) = deg.to_radians().psin_cos(); [[c, 0., -s], [0., 1., 0.], [s, 0., c]] }
pub(crate) fn rot_x(deg: f64) -> [[f64; 3]; 3] { let (s, c) = deg.to_radians().psin_cos(); [[1., 0., 0.], [0., c, s], [0., -s, c]] }
pub(crate) fn rot_mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    // Columns are basis vectors: result basis = a applied to b's basis.
    let ap = |v: [f64; 3]| add(add(mul(a[0], v[0]), mul(a[1], v[1])), mul(a[2], v[2]));
    [ap(b[0]), ap(b[1]), ap(b[2])]
}
pub(crate) const ID3: [[f64; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
