//! A body as a watertight triangle mesh, to a chord tolerance.
//!
//! Every edge is sampled once in 3D and each face it bounds reuses those points, so
//! neighbouring faces share their vertices exactly. A face is triangulated in its surface
//! parameters: its trim loops are the constraints of a Delaunay triangulation, points
//! inside come from a quadtree that splits where the surface leaves its chord, then
//! triangles still off the surface get their centres added. All of it scales with
//! the tolerance, so a finer view asks again with a smaller one.
//! Each corner carries its face's exact surface normal.

use crate::guard::BrepError;
use crate::ops::Body;
use makepad_csg_boolean::cdt::{Point2 as CdtPoint, CDT};
use makepad_csg_brep::curve::curve::Curve;
use makepad_csg_brep::kernel::Kernel;
use makepad_csg_brep::limits;
use makepad_csg_brep::math::{Point2, Point3, Vec3};
use makepad_csg_brep::nurbs::b_spline_surface_n_lib::p2::OrientType;
use makepad_csg_brep::surface::surface::{Surface, SurfaceKind};
use makepad_csg_brep::topology::edgeuse::p1 as eu;
use makepad_csg_brep::topology::edgeuse::p3 as eu3;
use makepad_csg_brep::topology::{edge, face, EdgeId, EdgeuseId, FaceId, FaceuseId, VertexId};
use makepad_csg_mesh::mesh::TriMesh;
use makepad_csg_math::{dvec3, orient2d};
use std::collections::HashMap;

/// What a tessellation aims for.
#[derive(Clone, Copy, Debug)]
pub struct TessOptions {
    /// Largest distance between the surface and its triangles (mm).
    pub chord: f64,
    /// Above this many triangles the chord doubles and it starts again, and the result says so.
    pub max_triangles: usize,
}

impl Default for TessOptions {
    fn default() -> TessOptions {
        TessOptions { chord: 0.02, max_triangles: 4_000_000 }
    }
}

/// A tessellated body.
#[derive(Clone, Debug)]
pub struct TessMesh {
    pub mesh: TriMesh,
    /// The face each triangle came from.
    pub tri_face: Vec<u32>,
    /// The chord it reached: larger than asked when the triangle budget ran out.
    pub chord: f64,
}

/// An edge shorter than this is a point (mm).
const COLLAPSED: f64 = 1e-7;

/// The side of a face that bounds material, and whether its outward normal is Su x Sv
/// (the material-side use runs against the surface when its orientation is `Same`).
/// A face has two uses; a boolean can leave the material on either.
fn material_use(b: &Body, f: FaceId) -> Result<(FaceuseId, bool), BrepError> {
    let up = face::p2::upward_faceuse(b, f)?.ok_or_else(|| BrepError::Kernel("a face without a use".into()))?;
    let mate = b.faceuse(up)?.mate.ok_or_else(|| BrepError::Kernel("a face use without a mate".into()))?;
    let is_void = |fu: FaceuseId| -> Result<bool, BrepError> {
        let shell = eu::faceuse_region(b, fu)?.ok_or_else(|| BrepError::Kernel("a face use outside any region".into()))?;
        Ok(b.region(shell)?.is_void() != 0)
    };
    let fu = if is_void(up)? && !is_void(mate)? { mate } else { up };
    Ok((fu, matches!(b.faceuse(fu)?.orientation, OrientType::Opposite)))
}

/// One loop of a face, as points in the surface's parameters with their mesh vertices.
struct LoopPoints {
    uv: Vec<[f64; 2]>,
    vertex: Vec<u32>,
}

struct Tess<'a> {
    k: &'a Kernel,
    chord: f64,
    out: TriMesh,
    vertex_of: HashMap<VertexId, u32>,
    /// An edge's samples: parameters and their mesh vertices.
    edge_samples: HashMap<EdgeId, (Vec<f64>, Vec<u32>)>,
}

fn p(a: Point3) -> makepad_csg_math::Vec3d {
    dvec3(a.x, a.y, a.z)
}

/// Parameters along `curve` within `chord`, by bisection, ends included.
fn sample_params(k: &Kernel, curve: &Curve, lo: f64, hi: f64, chord: f64) -> Result<Vec<f64>, BrepError> {
    const MAX_DEPTH: u32 = 14;
    fn dev(a: Point3, b: Point3, q: Point3) -> f64 {
        let ab = dvec3(b.x - a.x, b.y - a.y, b.z - a.z);
        let aq = dvec3(q.x - a.x, q.y - a.y, q.z - a.z);
        let t = (aq.x * ab.x + aq.y * ab.y + aq.z * ab.z) / (ab.x * ab.x + ab.y * ab.y + ab.z * ab.z).max(1e-30);
        let t = t.clamp(0.0, 1.0);
        let d = dvec3(aq.x - ab.x * t, aq.y - ab.y * t, aq.z - ab.z * t);
        (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
    }
    fn split(k: &Kernel, c: &Curve, t0: f64, a: Point3, t1: f64, b: Point3, chord: f64, depth: u32, out: &mut Vec<f64>) -> Result<(), BrepError> {
        limits::probe();
        let tm = 0.5 * (t0 + t1);
        let m = c.evaluate_point(k, tm)?;
        let q1 = c.evaluate_point(k, 0.5 * (t0 + tm))?;
        let q3 = c.evaluate_point(k, 0.5 * (tm + t1))?;
        let far = dev(a, b, m).max(dev(a, b, q1)).max(dev(a, b, q3));
        // A closed edge starts and ends at one point: it must split at the top.
        if depth < MAX_DEPTH && (far > chord || (depth == 0 && dist3(a, b) < COLLAPSED)) {
            split(k, c, t0, a, tm, m, chord, depth + 1, out)?;
            out.push(tm);
            split(k, c, tm, m, t1, b, chord, depth + 1, out)?;
        }
        Ok(())
    }
    let (a, b) = (curve.evaluate_point(k, lo)?, curve.evaluate_point(k, hi)?);
    let mut out = vec![lo];
    split(k, curve, lo, a, hi, b, chord, 0, &mut out)?;
    out.push(hi);
    Ok(out)
}

impl Tess<'_> {
    fn vertex(&mut self, v: Option<VertexId>, at: Point3) -> u32 {
        match v {
            Some(id) => *self.vertex_of.entry(id).or_insert_with(|| self.out.add_vertex(p(at))),
            None => self.out.add_vertex(p(at)),
        }
    }

    /// The samples of an edge in its own direction.
    fn edge(&mut self, b: &Body, e: EdgeId) -> Result<(Vec<f64>, Vec<u32>), BrepError> {
        if let Some(s) = self.edge_samples.get(&e) {
            return Ok(s.clone());
        }
        let (v0, v1) = edge::start_end_vertices(b, e)?;
        let curve = edge::curve(b, e)?.ok_or_else(|| BrepError::Kernel("an edge without a curve".into()))?;
        let interval = b.edge(e)?.interval;
        let (lo, hi) = (interval.min(), interval.max());
        let mut params = sample_params(self.k, curve, lo, hi, self.chord)?;
        let start = curve.evaluate_point(self.k, lo)?;
        let mut collapsed = true;
        for t in &params {
            collapsed &= dist3(curve.evaluate_point(self.k, *t)?, start) < COLLAPSED;
        }
        if collapsed {
            // A pole: one point in space, a line in the face's parameters.
            let id = self.vertex(v0.or(v1), start);
            params = vec![lo, 0.5 * (lo + hi), hi];
            let vertices = vec![id; 3];
            self.edge_samples.insert(e, (params.clone(), vertices.clone()));
            return Ok((params, vertices));
        }
        let mut vertices = Vec::with_capacity(params.len());
        for (i, t) in params.iter().enumerate() {
            let at = curve.evaluate_point(self.k, *t)?;
            let id = if i == 0 {
                self.vertex(v0, at)
            } else if i + 1 == params.len() {
                self.vertex(v1, at)
            } else {
                self.out.add_vertex(p(at))
            };
            vertices.push(id);
        }
        self.edge_samples.insert(e, (params.clone(), vertices.clone()));
        Ok((params, vertices))
    }

    /// One loop's points in the face's parameters.
    fn loop_points(&mut self, b: &mut Body, uses: &[EdgeuseId]) -> Result<LoopPoints, BrepError> {
        let mut out = LoopPoints { uv: Vec::new(), vertex: Vec::new() };
        for use_id in uses {
            let use_id = *use_id;
            let e = eu::edge(b, use_id)?.ok_or_else(|| BrepError::Kernel("an edge use without an edge".into()))?;
            let same = !matches!(eu::orientation(b, use_id)?, OrientType::Opposite);
            eu3::get_or_create_uv_trim_curve(self.k, b, use_id, false, false, true)?;
            let trim = eu::uv_trim_curve(b, use_id)?.0.cloned().ok_or_else(|| BrepError::Kernel("an edge use without a trim curve".into()))?;
            if edge::curve(b, e)?.is_none() {
                // A collapsed edge (a pole): one vertex, many parameters along the trim.
                let v = eu::vertexuse(b, use_id)?;
                let vertex = match v {
                    Some(vu) => b.vertexuse(vu)?.vertex,
                    None => None,
                };
                let range = trim.natural_interval();
                let first = trim.evaluate_point(self.k, range.min())?;
                let at = {
                    let face_id = eu::face(b, use_id)?.ok_or_else(|| BrepError::Kernel("a loop edge use without a face".into()))?;
                    let surface = b.face(face_id)?.surface.as_ref().ok_or_else(|| BrepError::Kernel("a face without a surface".into()))?;
                    surface.evaluate_point(self.k, Point2::new(first.x, first.y))?
                };
                let id = self.vertex(vertex, at);
                let n = 2;
                for i in 0..n {
                    let s = if same { i as f64 / n as f64 } else { 1.0 - i as f64 / n as f64 };
                    let q = trim.evaluate_point(self.k, range.min() + (range.max() - range.min()) * s)?;
                    out.uv.push([q.x, q.y]);
                    out.vertex.push(id);
                }
                continue;
            }
            let (params, vertices) = self.edge(b, e)?;
            let range = trim.natural_interval();
            let n = params.len();
            // Every point but the last: the next use starts where this one ends.
            for i in 0..n - 1 {
                let j = if same { i } else { n - 1 - i };
                let t = range.clamp_value(params[j]);
                let q = trim.evaluate_point(self.k, t)?;
                out.uv.push([q.x, q.y]);
                out.vertex.push(vertices[j]);
            }
        }
        Ok(out)
    }

    fn face(&mut self, b: &mut Body, f: FaceId, tri_face: &mut Vec<u32>, index: u32) -> Result<(), BrepError> {
        limits::probe();
        let (fu, same) = material_use(b, f)?;
        let mut loops = Vec::new();
        for lu in eu::faceuse_loopuses(b, fu, false, None)? {
            let uses = eu::loopuse_edgeuses(b, lu)?;
            let l = self.loop_points(b, &uses)?;
            if l.uv.len() >= 2 {
                loops.push(l);
            }
        }
        if loops.is_empty() {
            return Err(BrepError::Kernel(format!("face {} has no boundary", f.index())));
        }
        let surface = b.face(f)?.surface.as_ref().ok_or_else(|| BrepError::Kernel("a face without a surface".into()))?.clone_for_evaluation()?;
        let flip = !same;
        triangulate_face(self.k, &surface, &loops, self.chord, flip, &mut self.out, tri_face, index)
    }
}

fn eval(k: &Kernel, s: &Surface, uv: [f64; 2]) -> Result<Point3, BrepError> {
    Ok(s.evaluate_point(k, Point2::new(uv[0], uv[1]))?)
}

fn dist3(a: Point3, b: Point3) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

fn mid3(a: Point3, b: Point3) -> Point3 {
    Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z))
}

/// Points inside the face: a cell splits along the directions in which the surface leaves
/// its chord, so a cylinder grows strips, not a grid.
fn interior_points(k: &Kernel, s: &Surface, lo: [f64; 2], hi: [f64; 2], chord: f64, out: &mut Vec<[f64; 2]>) -> Result<(), BrepError> {
    /// Cells deeper than this are left to the triangle refinement.
    const MAX_DEPTH: u32 = 18;
    fn cell(k: &Kernel, s: &Surface, lo: [f64; 2], hi: [f64; 2], chord: f64, depth: u32, out: &mut Vec<[f64; 2]>) -> Result<(), BrepError> {
        limits::probe();
        let (mu, mv) = (0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1]));
        let c00 = eval(k, s, lo)?;
        let c11 = eval(k, s, hi)?;
        let c10 = eval(k, s, [hi[0], lo[1]])?;
        let c01 = eval(k, s, [lo[0], hi[1]])?;
        let e0 = eval(k, s, [mu, lo[1]])?;
        let e1 = eval(k, s, [mu, hi[1]])?;
        let e2 = eval(k, s, [lo[0], mv])?;
        let e3 = eval(k, s, [hi[0], mv])?;
        let along_u = dist3(e0, mid3(c00, c10)).max(dist3(e1, mid3(c01, c11)));
        let along_v = dist3(e2, mid3(c00, c01)).max(dist3(e3, mid3(c10, c11)));
        // The mixed term: zero on a cylinder, whatever the cell's size.
        let twist = dist3(mid3(c00, c11), mid3(c10, c01));
        // The first levels always split: a full turn can look flat at its corners.
        let start = depth < 4;
        let split_u = depth < MAX_DEPTH && (start || along_u > chord || twist > chord);
        let split_v = depth < MAX_DEPTH && (start || along_v > chord || twist > chord);
        match (split_u, split_v) {
            (true, true) => {
                out.push([mu, mv]);
                out.push([mu, lo[1]]);
                out.push([mu, hi[1]]);
                out.push([lo[0], mv]);
                out.push([hi[0], mv]);
                cell(k, s, lo, [mu, mv], chord, depth + 2, out)?;
                cell(k, s, [mu, lo[1]], [hi[0], mv], chord, depth + 2, out)?;
                cell(k, s, [lo[0], mv], [mu, hi[1]], chord, depth + 2, out)?;
                cell(k, s, [mu, mv], hi, chord, depth + 2, out)
            }
            (true, false) => {
                out.push([mu, lo[1]]);
                out.push([mu, hi[1]]);
                cell(k, s, lo, [mu, hi[1]], chord, depth + 1, out)?;
                cell(k, s, [mu, lo[1]], hi, chord, depth + 1, out)
            }
            (false, true) => {
                out.push([lo[0], mv]);
                out.push([hi[0], mv]);
                cell(k, s, lo, [hi[0], mv], chord, depth + 1, out)?;
                cell(k, s, [lo[0], mv], hi, chord, depth + 1, out)
            }
            (false, false) => Ok(()),
        }
    }
    cell(k, s, lo, hi, chord, 0, out)
}

fn is_flat(s: &Surface) -> bool {
    matches!(s.kind(), SurfaceKind::Plane(_))
}

/// The trim loops as segments in triangulation space, binned so that "is this inside" and
/// "is this next to the boundary" stay cheap for faces with thousands of boundary points.
struct Boundary {
    segs: Vec<([f64; 2], [f64; 2])>,
    origin: [f64; 2],
    cell: [f64; 2],
    n: usize,
    /// For each row: the segments crossing it.
    rows: Vec<Vec<u32>>,
    /// For each grid cell: the segments near it.
    grid: Vec<Vec<u32>>,
    /// How close to a segment a point may come: half its length, up to `reach`. Closer, the
    /// point would take the segment out of the Delaunay triangulation, and enforcing it costs.
    reach: f64,
    floor: f64,
}

impl Boundary {
    fn new(segs: Vec<([f64; 2], [f64; 2])>, size: [f64; 2], floor: f64, reach: f64) -> Boundary {
        let n = ((segs.len() as f64).sqrt() as usize).clamp(4, 512);
        let cell = [size[0].max(1e-12) / n as f64, size[1].max(1e-12) / n as f64];
        let origin = [0.0, 0.0];
        let mut b = Boundary { segs: Vec::new(), origin, cell, n, rows: vec![Vec::new(); n], grid: vec![Vec::new(); n * n], reach, floor };
        for (i, (s, e)) in segs.iter().enumerate() {
            let (r0, r1) = (b.row(s[1].min(e[1])), b.row(s[1].max(e[1])));
            for r in r0..=r1 {
                b.rows[r].push(i as u32);
            }
            let near = b.near_of(*s, *e);
            let (c0, c1) = (b.col(s[0].min(e[0]) - near), b.col(s[0].max(e[0]) + near));
            let (g0, g1) = (b.row(s[1].min(e[1]) - near), b.row(s[1].max(e[1]) + near));
            for r in g0..=g1 {
                for c in c0..=c1 {
                    b.grid[r * n + c].push(i as u32);
                }
            }
        }
        b.segs = segs;
        b
    }
    fn near_of(&self, a: [f64; 2], b: [f64; 2]) -> f64 {
        (0.5 * (a[0] - b[0]).hypot(a[1] - b[1])).min(self.reach).max(self.floor)
    }
    fn row(&self, y: f64) -> usize {
        (((y - self.origin[1]) / self.cell[1]).floor().max(0.0) as usize).min(self.n - 1)
    }
    fn col(&self, x: f64) -> usize {
        (((x - self.origin[0]) / self.cell[0]).floor().max(0.0) as usize).min(self.n - 1)
    }
    /// Even-odd over the loops.
    fn inside(&self, q: [f64; 2]) -> bool {
        let mut odd = false;
        for i in &self.rows[self.row(q[1])] {
            let (a, c) = self.segs[*i as usize];
            if (a[1] > q[1]) != (c[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) / (c[1] - a[1]) * (c[0] - a[0]) {
                odd = !odd;
            }
        }
        odd
    }
    fn is_near(&self, q: [f64; 2]) -> bool {
        self.grid[self.row(q[1]) * self.n + self.col(q[0])].iter().any(|i| {
            let (b, c) = self.segs[*i as usize];
            let (dx, dy) = (c[0] - b[0], c[1] - b[1]);
            let t = (((q[0] - b[0]) * dx + (q[1] - b[1]) * dy) / (dx * dx + dy * dy).max(1e-30)).clamp(0.0, 1.0);
            (q[0] - b[0] - dx * t).hypot(q[1] - b[1] - dy * t) < self.near_of(b, c)
        })
    }
}

/// Rounds a face refines before it settles for what it has.
const REFINE_ROUNDS: usize = 8;

#[allow(clippy::too_many_arguments)]
fn triangulate_face(
    k: &Kernel,
    s: &Surface,
    loops: &[LoopPoints],
    chord: f64,
    flip: bool,
    out: &mut TriMesh,
    tri_face: &mut Vec<u32>,
    index: u32,
) -> Result<(), BrepError> {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for q in loops.iter().flat_map(|l| l.uv.iter()) {
        for i in 0..2 {
            lo[i] = lo[i].min(q[i]);
            hi[i] = hi[i].max(q[i]);
        }
    }
    let span = [(hi[0] - lo[0]).max(1e-12), (hi[1] - lo[1]).max(1e-12)];
    // Delaunay in about the surface's own metric: parameters may be radians against millimetres.
    let mid = [0.5 * (lo[0] + hi[0]), 0.5 * (lo[1] + hi[1])];
    let scale = {
        let c = eval(k, s, mid)?;
        let (du, dv) = (eval(k, s, [mid[0] + span[0] * 0.25, mid[1]])?, eval(k, s, [mid[0], mid[1] + span[1] * 0.25])?);
        [(dist3(c, du) / (span[0] * 0.25)).max(1e-6), (dist3(c, dv) / (span[1] * 0.25)).max(1e-6)]
    };
    let to_cdt = |q: [f64; 2]| [(q[0] - lo[0]) * scale[0], (q[1] - lo[1]) * scale[1]];
    let size = [span[0] * scale[0], span[1] * scale[1]];
    let mut segs = Vec::new();
    for l in loops {
        let n = l.uv.len();
        segs.extend((0..n).map(|i| (to_cdt(l.uv[i]), to_cdt(l.uv[(i + 1) % n]))));
    }
    let boundary = Boundary::new(segs, size, 1e-6 * (size[0] + size[1]), chord * 3.0);
    let mut extra: Vec<[f64; 2]> = Vec::new();
    if !is_flat(s) {
        interior_points(k, s, lo, hi, chord, &mut extra)?;
    }
    extra.retain(|q| {
        let a = to_cdt(*q);
        boundary.inside(a) && !boundary.is_near(a)
    });
    let mut seen: std::collections::HashSet<(u64, u64)> = std::collections::HashSet::with_capacity(extra.len());
    extra.retain(|q| seen.insert((q[0].to_bits(), q[1].to_bits())));
    // Parameters are cached between rounds: only what's new costs surface evaluations.
    let mut positions: HashMap<(u64, u64), Point3> = HashMap::with_capacity(extra.len());
    let mut passed: std::collections::HashSet<[u32; 3]> = std::collections::HashSet::new();
    let mut round = 0;
    loop {
        limits::probe();
        let mut cdt = CDT::new(CdtPoint { x: 0.0, y: 0.0 }, CdtPoint { x: size[0], y: size[1] });
        let mut uv: Vec<[f64; 2]> = Vec::new();
        let mut vertex: Vec<Option<u32>> = Vec::new();
        // One triangulation point per distinct parameter: two uses can meet at one.
        let mut point_of: HashMap<(u64, u64), (u32, u32)> = HashMap::new();
        let mut boundary_edges: Vec<(u32, u32)> = Vec::new();
        for l in loops {
            let mut ids = Vec::with_capacity(l.uv.len());
            for (q, v) in l.uv.iter().zip(&l.vertex) {
                let key = (q[0].to_bits(), q[1].to_bits());
                // (the triangulation's own id, the index its triangles use)
                let id = *point_of.entry(key).or_insert_with(|| {
                    let a = to_cdt(*q);
                    let id = cdt.insert_point(a[0], a[1]);
                    uv.push(*q);
                    vertex.push(Some(*v));
                    (id, uv.len() as u32 - 1)
                });
                ids.push(id);
            }
            let n = ids.len();
            for i in 0..n {
                let (a, b) = (ids[i], ids[(i + 1) % n]);
                if a != b {
                    cdt.add_constraint(a.0, b.0);
                    boundary_edges.push((a.1, b.1));
                }
            }
        }
        for q in &extra {
            let a = to_cdt(*q);
            cdt.insert_point(a[0], a[1]);
            uv.push(*q);
            vertex.push(None);
        }
        cdt.finalize();
        let mut on_surface: Vec<Point3> = Vec::with_capacity(uv.len());
        for q in &uv {
            let key = (q[0].to_bits(), q[1].to_bits());
            on_surface.push(match positions.get(&key) {
                Some(at) => *at,
                None => {
                    let at = eval(k, s, *q)?;
                    positions.insert(key, at);
                    at
                }
            });
        }
        let mut keep: Vec<[u32; 3]> = Vec::new();
        let mut grow: Vec<[f64; 2]> = Vec::new();
        for t in cdt.get_triangles() {
            let (ia, ib, ic) = (t[0] as usize, t[1] as usize, t[2] as usize);
            let (a, b, c) = (uv[ia], uv[ib], uv[ic]);
            let centre = [(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0];
            let (ca, cb, cc) = (to_cdt(a), to_cdt(b), to_cdt(c));
            if !boundary.inside([(ca[0] + cb[0] + cc[0]) / 3.0, (ca[1] + cb[1] + cc[1]) / 3.0]) {
                continue;
            }
            let cross = orient2d(ca[0], ca[1], cb[0], cb[1], cc[0], cc[1]);
            if cross == 0.0 {
                continue;
            }
            keep.push(if cross > 0.0 { t } else { [t[0], t[2], t[1]] });
            let mut sorted = t;
            sorted.sort_unstable();
            if !is_flat(s) && round < REFINE_ROUNDS && !passed.contains(&sorted) {
                let (pa, pb, pc) = (on_surface[ia], on_surface[ib], on_surface[ic]);
                let flat = Point3::new((pa.x + pb.x + pc.x) / 3.0, (pa.y + pb.y + pc.y) / 3.0, (pa.z + pb.z + pc.z) / 3.0);
                let mut worst = (dist3(eval(k, s, centre)?, flat), centre);
                for (qa, qb, ma, mb) in [(a, b, pa, pb), (b, c, pb, pc), (c, a, pc, pa)] {
                    let m = [0.5 * (qa[0] + qb[0]), 0.5 * (qa[1] + qb[1])];
                    let d = dist3(eval(k, s, m)?, mid3(ma, mb));
                    if d > worst.0 {
                        worst = (d, m);
                    }
                }
                let at = to_cdt(worst.1);
                if worst.0 <= chord {
                    passed.insert(sorted);
                } else if boundary.inside(at) && !boundary.is_near(at) && seen.insert((worst.1[0].to_bits(), worst.1[1].to_bits())) {
                    grow.push(worst.1);
                }
            }
        }
        if !grow.is_empty() && round < REFINE_ROUNDS {
            extra.extend(grow);
            round += 1;
            continue;
        }
        // Every boundary edge must have survived, or the neighbour's side stays open.
        let mut present: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::with_capacity(keep.len() * 3);
        for t in &keep {
            for (x, y) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                present.insert((x.min(y), x.max(y)));
            }
        }
        let missing = boundary_edges.iter().filter(|(x, y)| !present.contains(&((*x).min(*y), (*x).max(*y)))).count();
        if missing > 0 {
            return Err(BrepError::Kernel(format!("{missing} of {} boundary edges did not survive triangulation", boundary_edges.len())));
        }
        // One surface normal per triangulation point.
        let mut normals = Vec::with_capacity(uv.len());
        for q in &uv {
            let (_, du, dv) = s.evaluate_1st_derivatives(k, Point2::new(q[0], q[1]), false, false)?;
            let n = du.cross(dv);
            let len = n.length();
            let scale = du.length() * dv.length();
            normals.push(if len.is_finite() && scale.is_finite() && len > 0.0 && len >= 1e-12 * scale {
                let n = if flip { -n } else { n };
                Some(dvec3(n.x / len, n.y / len, n.z / len))
            } else {
                None
            });
        }
        let mut ids = vec![u32::MAX; uv.len()];
        for t in &keep {
            let corners = if flip { [t[0], t[2], t[1]] } else { *t };
            let mut mapped = [0u32; 3];
            for (j, c) in t.iter().enumerate() {
                let i = *c as usize;
                mapped[j] = match vertex[i] {
                    Some(v) => v,
                    None => {
                        if ids[i] == u32::MAX {
                            ids[i] = out.add_vertex(p(on_surface[i]));
                        }
                        ids[i]
                    }
                };
            }
            // A pole collapses a triangle to a line.
            if mapped[0] == mapped[1] || mapped[1] == mapped[2] || mapped[2] == mapped[0] {
                continue;
            }
            if flip {
                mapped.swap(1, 2);
            }
            let [a, b, c] = mapped.map(|i| out.vertices[i as usize]);
            let geometric = (b - a).cross(c - a);
            let len = geometric.length();
            // A sliver with no area of its own takes its corners' surface normals as they are.
            let geometric = (len.is_finite() && len > 0.0).then(|| geometric / len);
            // Triangles and their exact normals are appended together.
            out.triangles.push(mapped);
            out.corner_normals.extend(corners.map(|c| match (normals[c as usize], geometric) {
                (Some(n), Some(g)) if n.dot(g) < 0.0 => g,
                (Some(n), _) => n,
                (None, Some(g)) => g,
                (None, None) => dvec3(0.0, 0.0, 1.0),
            }));
            tri_face.push(index);
        }
        return Ok(());
    }
}

fn once(k: &Kernel, b: &mut Body, chord: f64) -> Result<TessMesh, BrepError> {
    let mut t = Tess { k, chord, out: TriMesh::new(), vertex_of: HashMap::new(), edge_samples: HashMap::new() };
    let mut tri_face = Vec::new();
    let faces = makepad_csg_brep::topology::brep::p1::faces(b, None)?;
    for f in faces {
        t.face(b, f, &mut tri_face, f.index() as u32).map_err(|e| match e {
            BrepError::Kernel(m) => BrepError::Kernel(format!("tessellating face {}: {m}", f.index())),
            other => other,
        })?;
    }
    Ok(TessMesh { mesh: t.out, tri_face, chord })
}

/// The body's mesh. Over the triangle budget the chord doubles: coarser, never a failure.
pub fn tessellate(k: &Kernel, b: &mut Body, options: &TessOptions) -> Result<TessMesh, BrepError> {
    let mut chord = options.chord.max(1e-5);
    loop {
        let m = once(k, b, chord)?;
        if m.mesh.triangle_count() <= options.max_triangles {
            return Ok(m);
        }
        chord *= 2.0;
    }
}

/// The outward normal of a face at parameters.
fn oriented_normal(k: &Kernel, b: &Body, f: FaceId, uv: Point2) -> Result<Vec3, BrepError> {
    let surface = b.face(f)?.surface.as_ref().ok_or_else(|| BrepError::Kernel("a face without a surface".into()))?;
    let (_, du, dv) = surface.evaluate_1st_derivatives(k, uv, false, false)?;
    let n = du.cross(dv);
    Ok(if material_use(b, f)?.1 { n } else { -n })
}

/// Whether the solid's angle across an edge is under 180 degrees, and whether it is smooth.
pub fn edge_is_convex(k: &Kernel, b: &mut Body, e: EdgeId) -> Result<(bool, bool), BrepError> {
    let uses = edge::edgeuses(b, e, None, None)?;
    // Each face holds an edge through two uses (its two sides): only the outward side's.
    let mut loop_uses: Vec<EdgeuseId> = Vec::new();
    for u in uses {
        if !eu::is_loop_edgeuse(b, u)? {
            continue;
        }
        let Some(f) = eu::face(b, u)? else { continue };
        if eu::faceuse(b, u)? == Some(material_use(b, f)?.0) {
            loop_uses.push(u);
        }
    }
    let interval = b.edge(e)?.interval;
    let t = interval.mid();
    let curve = edge::curve(b, e)?.ok_or_else(|| BrepError::Kernel("an edge without a curve".into()))?.clone();
    let tangent = curve.evaluate(k, t, 1, false, true)?;
    let tangent = tangent.get(1).copied().ok_or_else(|| BrepError::Kernel("an edge without a tangent".into()))?;
    let mut seen: Vec<(Vec3, bool)> = Vec::new();
    for u in loop_uses {
        let Some(f) = eu::face(b, u)? else { continue };
        if seen.len() == 2 {
            break;
        }
        eu3::get_or_create_uv_trim_curve(k, b, u, false, false, true)?;
        let Some(trim) = eu::uv_trim_curve(b, u)?.0.cloned() else { continue };
        let q = trim.evaluate_point(k, trim.natural_interval().clamp_value(t))?;
        let n = oriented_normal(k, b, f, Point2::new(q.x, q.y))?;
        seen.push((n, !matches!(eu::orientation(b, u)?, OrientType::Opposite)));
    }
    let [(n1, same1), (n2, _)] = seen[..] else { return Ok((true, false)) };
    let (l1, l2) = (n1.length().max(1e-30), n2.length().max(1e-30));
    let cos = n1.dot(n2) / (l1 * l2);
    let smooth = cos > 1.0 - 1e-6;
    let dir = if same1 { tangent } else { -tangent };
    // The material-side use keeps its face on the right of n x t, so into the face is
    // t x n. On a convex edge going that way dips under the other face.
    Ok((dir.cross(n1).dot(n2) < 0.0, smooth))
}

/// A face's surface kind, and its outward normal when it is flat.
pub fn face_kind(k: &Kernel, b: &mut Body, f: FaceId) -> Result<(&'static str, Option<[f64; 3]>), BrepError> {
    let surface = b.face(f)?.surface.as_ref().ok_or_else(|| BrepError::Kernel("a face without a surface".into()))?;
    let kind = match surface.kind() {
        SurfaceKind::Plane(_) => "plane",
        SurfaceKind::Cone(_) => "cone",
        SurfaceKind::Sphere(_) => "sphere",
        SurfaceKind::Torus(_) => "torus",
        SurfaceKind::SurfOfRevolution(_) => "revolution",
        SurfaceKind::SurfOfExtrusion(_) => "extrusion",
        SurfaceKind::OffsetSurface(_) => "offset",
        _ => "nurbs",
    };
    if kind != "plane" {
        return Ok((kind, None));
    }
    let domain = b.face(f)?.uv_domain;
    let n = oriented_normal(k, b, f, domain.mid())?;
    let l = n.length().max(1e-30);
    Ok((kind, Some([n.x / l, n.y / l, n.z / l])))
}
