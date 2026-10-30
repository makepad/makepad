//! Building a graph's nodes into meshes, each node once per key.

use crate::*;
use makepad_csg_cad as cad;
use makepad_csg_cad::Region;
use makepad_csg_math::{dvec3, Vec3d};
use std::collections::HashMap;

const SANS: &[u8] = include_bytes!("../../../../widgets/resources/IBMPlexSans-SemiBold.ttf");
const MONO: &[u8] = include_bytes!("../../../../widgets/resources/LiberationMono-Regular.ttf");

/// A node that could not be built.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub node: NodeId,
    pub op: &'static str,
    pub message: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.op, self.message)
    }
}

/// Rounds a mesh stays cached after its last use.
const KEEP_ROUNDS: u64 = 4;

/// The most holes one Hole drills (each is a boolean).
const MAX_HOLES: usize = 256;
/// The most triangles a node's result may hold: past it every op on it is minutes of work
/// and the plate is unprintable anyway.
const MAX_TRIANGLES: usize = 4_000_000;

/// Builds nodes into meshes and keeps every mesh it built by key (and, for
/// meshed fields, by draft), dropping those no evaluation used for two rounds.
pub struct Evaluator {
    pub tolerance: Tolerance,
    /// Field and shell cells are multiplied by this (a live drag's coarse
    /// preview: 2 meshes an eighth of the samples).
    pub draft: f64,
    /// The finest default field cell.
    pub finest_cell: f64,
    cache: HashMap<(Key, u64), (Arc<TriMesh>, u64)>,
    round: u64,
    /// Meshes built (not found in the cache) since the last `begin`.
    pub built: usize,
}

impl Default for Evaluator {
    fn default() -> Self {
        Evaluator { tolerance: Tolerance::default(), draft: 1.0, finest_cell: 0.25, cache: HashMap::new(), round: 0, built: 0 }
    }
}

/// The frame of a sketch plane: (u, v, w).
pub fn plane_frame(p: Plane) -> (Vec3d, Vec3d, Vec3d) {
    match p {
        Plane::Xz => (dvec3(1.0, 0.0, 0.0), dvec3(0.0, 0.0, -1.0), dvec3(0.0, 1.0, 0.0)),
        Plane::Xy => (dvec3(1.0, 0.0, 0.0), dvec3(0.0, 1.0, 0.0), dvec3(0.0, 0.0, 1.0)),
        Plane::Yz => (dvec3(0.0, 0.0, 1.0), dvec3(0.0, 1.0, 0.0), dvec3(-1.0, 0.0, 0.0)),
    }
}

/// Rotation of `deg` about X, then Y, then Z (as rows).
pub fn rotation_xyz(deg: P3) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul3(&rz, &mul3(&ry, &rx))
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

/// The affine matrix (3 rows of 4) of a transform node.
pub fn transform_matrix(at: P3, rotate: P3, scale: P3) -> [[f64; 4]; 3] {
    let r = rotation_xyz(rotate);
    std::array::from_fn(|i| [r[i][0] * scale[0], r[i][1] * scale[1], r[i][2] * scale[2], at[i]])
}

fn place(mut m: TriMesh, frame: (Vec3d, Vec3d, Vec3d)) -> TriMesh {
    let (u, v, w) = frame;
    for p in &mut m.vertices {
        *p = u * p.x + v * p.y + w * p.z;
    }
    m
}

fn nonempty(m: TriMesh, what: &str) -> Result<TriMesh, String> {
    if m.triangles.is_empty() {
        return Err(format!("the {what} is empty"));
    }
    Ok(m)
}

fn v3(p: P3) -> Vec3d {
    dvec3(p[0], p[1], p[2])
}

/// The gear a node is (through transforms).
fn gear_of(g: &Graph, id: NodeId) -> Option<&GearSpec> {
    match &g.node(id).op {
        Op::Gear { gear, .. } => Some(gear),
        Op::Transform { source, .. } => gear_of(g, *source),
        _ => None,
    }
}

fn cad_gear(g: &GearSpec) -> cad::Gear {
    cad::Gear { module: g.module, teeth: g.teeth as usize, pressure_deg: g.pressure, backlash: g.backlash, shift: g.shift, bore: g.bore }
}

impl Evaluator {
    pub fn new() -> Evaluator {
        Evaluator::default()
    }

    /// Start an evaluation round: meshes none of the last few rounds used are dropped (the
    /// ones before stay for an undo and for a value dragged back).
    pub fn begin(&mut self) {
        self.round += 1;
        let keep = self.round.saturating_sub(KEEP_ROUNDS);
        self.cache.retain(|_, (_, used)| *used >= keep);
        self.built = 0;
    }

    /// The mesh of `id` (model frame).
    pub fn mesh(&mut self, g: &Graph, id: NodeId, cancelled: Option<&dyn Fn() -> bool>) -> Result<Arc<TriMesh>, Failure> {
        let node = g.node(id);
        let draft = if self.drafted(g, id) { self.draft.to_bits() } else { 0 };
        if let Some((m, used)) = self.cache.get_mut(&(node.key, draft)) {
            *used = self.round;
            return Ok(m.clone());
        }
        if cancelled.is_some_and(|c| c()) {
            return Err(Failure { node: id, op: node.op.name(), message: "cancelled".into() });
        }
        let mut inputs = Vec::new();
        for i in node.op.inputs() {
            inputs.push(self.mesh(g, i, cancelled)?);
        }
        if let Some(big) = inputs.iter().find(|m| m.triangles.len() > MAX_TRIANGLES) {
            return Err(Failure { node: id, op: node.op.name(), message: format!("an input has {} triangles (at most {MAX_TRIANGLES}): use a coarser cell or fewer segments", big.triangles.len()) });
        }
        let m = self.build(g, &node.op, &inputs, cancelled).map_err(|message| Failure { node: id, op: node.op.name(), message })?;
        if m.triangles.len() > MAX_TRIANGLES {
            return Err(Failure { node: id, op: node.op.name(), message: format!("the result has {} triangles (at most {MAX_TRIANGLES}): use a coarser cell or fewer segments", m.triangles.len()) });
        }
        let m = Arc::new(m);
        self.built += 1;
        self.cache.insert((node.key, draft), (m.clone(), self.round));
        Ok(m)
    }

    /// Whether the node's mesh depends on the draft (a field or a shell below it).
    fn drafted(&self, g: &Graph, id: NodeId) -> bool {
        match &g.node(id).op {
            Op::Field { .. } | Op::Shell { .. } => true,
            op => op.inputs().into_iter().any(|i| self.drafted(g, i)),
        }
    }

    fn regions(&self, s: &Sketch) -> Result<Vec<Region>, String> {
        s.regions(&self.tolerance)
    }

    fn build(&mut self, g: &Graph, op: &Op, inputs: &[Arc<TriMesh>], cancelled: Option<&dyn Fn() -> bool>) -> Result<TriMesh, String> {
        let tol = self.tolerance;
        match op {
            Op::Box { size, radius } => {
                if !(size[0] > 0.0 && size[1] > 0.0 && size[2] > 0.0) {
                    return Err("a box needs three positive sizes".into());
                }
                if *radius <= 0.0 {
                    return Ok(makepad_csg_primitives::cube(v3(*size), true));
                }
                if 2.0 * radius >= size[0].min(size[1]).min(size[2]) {
                    return Err(format!("a {radius} rounding does not fit a box of {:?}", size));
                }
                Ok(rounded_box(*size, *radius, tol.segments(*radius, std::f64::consts::FRAC_PI_2).max(2)))
            }
            Op::Sphere { radius, segments, rings } => {
                if !(*radius > 0.0) {
                    return Err("a sphere needs a positive radius".into());
                }
                Ok(makepad_csg_primitives::sphere(*radius, *segments, *rings))
            }
            Op::Cylinder { radius, top_radius, height, segments } => {
                if !(*height > 0.0 && *radius >= 0.0 && *top_radius >= 0.0 && radius.max(*top_radius) > 0.0) {
                    return Err("a cylinder needs a positive height and radius".into());
                }
                if radius == top_radius {
                    Ok(makepad_csg_primitives::cylinder(*radius, *height, *segments, true))
                } else {
                    Ok(makepad_csg_primitives::tapered_cylinder(*radius, *top_radius, *height, *segments, true))
                }
            }
            Op::Torus { radius, tube, segments, sides } => {
                if !(*tube > 0.0 && radius > tube) {
                    return Err("a torus needs a tube thinner than its radius".into());
                }
                Ok(makepad_csg_primitives::torus(*radius, *tube, *segments, *sides))
            }
            Op::Extrude { sketch, plane, height, draft, both } => Ok(place(cad::extrude(&self.regions(sketch)?, *height, *draft, *both)?, plane_frame(*plane))),
            // revolve() builds about its Y axis with x the radius: the model frame already.
            Op::Revolve { sketch, angle } => cad::revolve(&self.regions(sketch)?, *angle, &tol),
            Op::Sweep { sketch, path, twist, scale } => {
                let (points, closed) = match path {
                    // Helix about +Y.
                    Path3::Helix { radius, pitch, turns } => (cad::helix(*radius, *pitch, *turns, &tol).into_iter().map(|p| dvec3(p.x, p.z, -p.y)).collect::<Vec<_>>(), false),
                    Path3::Points { points, closed } => (points.iter().map(|p| v3(*p)).collect(), *closed),
                };
                cad::sweep(&self.regions(sketch)?, &points, closed, *twist, *scale)
            }
            Op::Loft { sections, plane } => {
                let mut out = Vec::new();
                for (i, s) in sections.iter().enumerate() {
                    let regions = self.regions(&s.sketch).map_err(|e| format!("section {i}: {e}"))?;
                    if regions.len() != 1 || !regions[0].holes.is_empty() {
                        return Err(format!("section {i}: a loft section is one outline without holes"));
                    }
                    out.push(cad::Section { outline: regions[0].outer.clone(), z: s.at });
                }
                Ok(place(cad::loft(&out, &tol)?, plane_frame(*plane)))
            }
            Op::Text { text, size, depth, spacing, align, font, plane } => {
                let font = match font {
                    Font::Sans => SANS,
                    Font::Mono => MONO,
                };
                let align = match align {
                    Align::Left => 0.0,
                    Align::Center => 0.5,
                    Align::Right => 1.0,
                };
                let style = cad::TextStyle { cap_height: *size, spacing: *spacing, line_height: 1.6, align, valign: if align == 0.5 { 0.5 } else { 0.0 } };
                let regions = cad::text_regions(font, text, &style, &tol)?;
                Ok(place(cad::extrude(&regions, *depth, 0.0, false)?, plane_frame(*plane)))
            }
            Op::Gear { gear, plane, mesh_with } => {
                let cg = cad_gear(gear);
                let mut m = cad::extrude(&[cg.region(&tol)?], gear.thickness, 0.0, false)?;
                let mut offset = dvec3(0.0, 0.0, 0.0);
                if let Some((other, angle)) = mesh_with {
                    let og = gear_of(g, *other).ok_or("'mesh_with' is not a gear")?;
                    if (og.module - gear.module).abs() > 1e-9 {
                        return Err(format!("gears mesh only with the same module ({} vs {})", og.module, gear.module));
                    }
                    let og = cad_gear(og);
                    // The other gear's centre: its vertices are evenly spread round it.
                    let om = &inputs[0];
                    let n = om.vertices.len().max(1) as f64;
                    let (sx, sz) = om.vertices.iter().fold((0.0, 0.0), |(x, z), p| (x + p.x, z + p.z));
                    let oc = dvec3(sx / n, om.bounding_box().min.y, sz / n);
                    let dir = angle.to_radians();
                    let d = cg.center_distance(&og);
                    offset = oc + dvec3(d * dir.cos(), 0.0, -d * dir.sin());
                    // Teeth in the gaps: the other gear has a tooth on its +X, so `dir` lies
                    // `phi` past one of its teeth; this gear (a tooth on its own +X) turns to
                    // put a gap toward it, and follows phi by the ratio.
                    let pitch_o = std::f64::consts::TAU / og.teeth as f64;
                    let phi = dir - (dir / pitch_o).round() * pitch_o;
                    let turn = std::f64::consts::PI + dir + std::f64::consts::PI / cg.teeth as f64 + phi * og.teeth as f64 / cg.teeth as f64;
                    let (s, c) = turn.sin_cos();
                    for p in &mut m.vertices {
                        let (x, y) = (p.x, p.y);
                        p.x = x * c - y * s;
                        p.y = x * s + y * c;
                    }
                }
                let mut m = place(m, plane_frame(*plane));
                for p in &mut m.vertices {
                    *p = *p + offset;
                }
                Ok(m)
            }
            Op::Thread { diameter, pitch, length, clearance, left } => {
                let pitch = match pitch {
                    Some(p) => *p,
                    None => cad::iso_coarse_pitch(*diameter).ok_or_else(|| format!("no ISO coarse pitch for {diameter}: give 'pitch'"))?,
                };
                let t = cad::Thread { diameter: *diameter, pitch, length: *length, clearance: *clearance, left: *left };
                // Along +Y.
                Ok(place(t.solid(&tol)?, (dvec3(1.0, 0.0, 0.0), dvec3(0.0, 0.0, -1.0), dvec3(0.0, 1.0, 0.0))))
            }
            Op::Fillet { radius, edges, .. } | Op::Chamfer { size: radius, edges, .. } => {
                let source = &inputs[0];
                let chains = cad::feature_edges(source, 30.0);
                let picked = edges.select(source, &chains)?;
                let edge_op = if matches!(op, Op::Fillet { .. }) { cad::EdgeOp::Fillet(*radius) } else { cad::EdgeOp::Chamfer(*radius) };
                cad::apply_edges(source, &picked, edge_op, &tol, cancelled)
            }
            Op::Hole { kind, diameter, depth, at, dir, .. } => hole(&inputs[0], *kind, *diameter, *depth, at, *dir, &tol, cancelled),
            Op::Mirror { plane, center, keep, .. } => {
                let source = &inputs[0];
                let mut m = (**source).clone();
                for p in &mut m.vertices {
                    match plane {
                        MirrorPlane::Yz => p.x = 2.0 * center[0] - p.x,
                        MirrorPlane::Xz => p.y = 2.0 * center[1] - p.y,
                        MirrorPlane::Xy => p.z = 2.0 * center[2] - p.z,
                    }
                }
                for t in &mut m.triangles {
                    t.swap(1, 2);
                }
                Ok(if *keep { cad::union(source, &m) } else { m })
            }
            Op::Shell { thickness, open, cell, .. } => shell(&inputs[0], *thickness, *open, cell.map(|c| c * self.draft.max(1.0)), self.draft.max(1.0)),
            Op::Boolean { mode, .. } => {
                let mut out = (*inputs[0]).clone();
                for other in &inputs[1..] {
                    if cancelled.is_some_and(|c| c()) {
                        return Err("cancelled".into());
                    }
                    // The shared booleans: part-scaled tolerance, touching and coplanar faces fused.
                    out = match mode {
                        BoolMode::Union => cad::union(&out, other),
                        BoolMode::Difference => cad::difference(&out, other),
                        BoolMode::Intersection => cad::intersection(&out, other),
                    };
                }
                nonempty(out, op.name())
            }
            Op::Transform { at, rotate, scale, .. } => {
                let m = transform_matrix(*at, *rotate, *scale);
                let mut out = (*inputs[0]).clone();
                for p in &mut out.vertices {
                    let q = [p.x, p.y, p.z];
                    *p = dvec3(
                        m[0][0] * q[0] + m[0][1] * q[1] + m[0][2] * q[2] + m[0][3],
                        m[1][0] * q[0] + m[1][1] * q[1] + m[1][2] * q[2] + m[1][3],
                        m[2][0] * q[0] + m[2][1] * q[1] + m[2][2] * q[2] + m[2][3],
                    );
                }
                // A mirroring scale turns the triangles inside out.
                if scale[0] * scale[1] * scale[2] < 0.0 {
                    for t in &mut out.triangles {
                        t.swap(1, 2);
                    }
                }
                Ok(out)
            }
            Op::Field { shape, cell } => {
                let (min, max) = (v3(shape.min), v3(shape.max));
                if !(max.x > min.x && max.y > min.y && max.z > min.z) {
                    return Err("an SDF's bounds need min below max on every axis".into());
                }
                let cell = cell.unwrap_or_else(|| makepad_csg_sdf::default_cell(min, max, self.finest_cell)) * self.draft.max(1.0);
                nonempty(makepad_csg_sdf::mesh_field(shape.field(), min, max, cell), "shape (nothing inside its surface)")
            }
            Op::Mesh { mesh } => Ok((**mesh).clone()),
        }
    }
}

/// A box centred on the origin with every edge and corner rounded by `r`: each face a
/// grid whose lines include the rounding's arcs, every grid point pushed onto the
/// rounded surface (the inner box's nearest point plus `r` toward it), the faces
/// welded where they meet. `arc` grid steps round each quarter.
fn rounded_box(size: P3, r: f64, arc: usize) -> TriMesh {
    let half = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
    let inner = [half[0] - r, half[1] - r, half[2] - r];
    // Grid coordinates along each axis: the arc at each end and the flat between.
    let coords = |a: usize| -> Vec<f64> {
        let mut c: Vec<f64> = (0..=arc).map(|j| -inner[a] - r * (std::f64::consts::FRAC_PI_2 * (1.0 - j as f64 / arc as f64)).sin()).collect();
        let mirrored: Vec<f64> = c.iter().rev().map(|x| -x).collect();
        if inner[a] > 1e-12 {
            c.extend(mirrored);
        } else {
            c.extend(mirrored.into_iter().skip(1));
        }
        c
    };
    let axes = [coords(0), coords(1), coords(2)];
    let mut m = TriMesh::new();
    let mut ids: HashMap<[u64; 3], u32> = HashMap::new();
    let mut vertex = |m: &mut TriMesh, p: P3| -> u32 {
        let q = [p[0].clamp(-inner[0], inner[0]), p[1].clamp(-inner[1], inner[1]), p[2].clamp(-inner[2], inner[2])];
        let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-300);
        let s = [q[0] + d[0] / l * r, q[1] + d[1] / l * r, q[2] + d[2] / l * r];
        // Weld by the grid point (faces share their border points exactly).
        *ids.entry([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]).or_insert_with(|| m.add_vertex(dvec3(s[0], s[1], s[2])))
    };
    // Each face: the normal axis n at side +-1, spanned by axes u, v (u x v = +n).
    for (n, u, v) in [(0usize, 1usize, 2usize), (1, 2, 0), (2, 0, 1)] {
        for side in [-1.0f64, 1.0] {
            let (cu, cv) = (&axes[u], &axes[v]);
            let mut grid = vec![vec![0u32; cv.len()]; cu.len()];
            for (i, a) in cu.iter().enumerate() {
                for (j, b) in cv.iter().enumerate() {
                    let mut p = [0.0; 3];
                    p[n] = side * half[n];
                    p[u] = *a;
                    p[v] = *b;
                    grid[i][j] = vertex(&mut m, p);
                }
            }
            for i in 0..cu.len() - 1 {
                for j in 0..cv.len() - 1 {
                    let (a, b, c, d) = (grid[i][j], grid[i + 1][j], grid[i + 1][j + 1], grid[i][j + 1]);
                    if side > 0.0 {
                        m.add_triangle(a, b, c);
                        m.add_triangle(a, c, d);
                    } else {
                        m.add_triangle(a, c, b);
                        m.add_triangle(a, d, c);
                    }
                }
            }
        }
    }
    // Arc points that coincide (a rounding as large as a half side) leave slivers.
    m.triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]);
    m
}

fn hole(source: &TriMesh, kind: HoleKind, d: f64, depth: Option<f64>, points: &[P3], dir: P3, tol: &Tolerance, cancelled: Option<&dyn Fn() -> bool>) -> Result<TriMesh, String> {
    if points.len() > MAX_HOLES {
        return Err(format!("{} holes in one Hole (at most {MAX_HOLES})", points.len()));
    }
    let dir = v3(dir);
    if dir.length() < 1e-9 {
        return Err("'dir' must not be zero".into());
    }
    let dir = dir.normalize();
    if points.is_empty() {
        return Err("'at' is missing: where the hole starts".into());
    }
    let bb = source.bounding_box();
    let through = (bb.max - bb.min).length() + 2.0;
    let depth = depth.unwrap_or(through);
    let tool = match kind {
        HoleKind::Tapped { pitch, clearance } => {
            let pitch = match pitch {
                Some(p) => p,
                None => cad::iso_coarse_pitch(d).ok_or_else(|| format!("no ISO coarse pitch for {d}: give 'pitch'"))?,
            };
            let len = depth.min(through) + 1.0;
            let t = cad::Thread { diameter: d, pitch, length: len, clearance, left: false };
            let mut m = t.solid(tol)?;
            // Along +Z from 0: turned half round X (a rotation, so the hand stays) and
            // raised, it runs from 1 above the surface down.
            for p in &mut m.vertices {
                p.y = -p.y;
                p.z = 1.0 - p.z;
            }
            m
        }
        HoleKind::Simple => cad::hole_tool(&cad::Hole::Simple { diameter: d, depth }, 1.0, tol)?,
        HoleKind::Counterbore { bore_diameter, bore_depth } => cad::hole_tool(&cad::Hole::Counterbore { diameter: d, depth, bore_diameter, bore_depth }, 1.0, tol)?,
        HoleKind::Countersink { sink_diameter, angle } => cad::hole_tool(&cad::Hole::Countersink { diameter: d, depth, sink_diameter, angle_deg: angle }, 1.0, tol)?,
    };
    // Tool axis: -Z into the material. Map -Z to `dir`.
    let w = dir * -1.0;
    let helper = if w.x.abs() < 0.9 { dvec3(1.0, 0.0, 0.0) } else { dvec3(0.0, 1.0, 0.0) };
    let u = helper.cross(w).normalize();
    let vv = w.cross(u);
    let mut all = TriMesh::new();
    for p in points {
        if cancelled.is_some_and(|c| c()) {
            return Err("cancelled".into());
        }
        let o = v3(*p);
        let mut t = tool.clone();
        for q in &mut t.vertices {
            *q = o + u * q.x + vv * q.y + w * q.z;
        }
        all = cad::union(&all, &t);
    }
    let out = cad::difference(source, &all);
    if out.triangles.is_empty() {
        return Err("the holes removed the whole object".into());
    }
    Ok(out)
}

/// The cavity of a shell: points deeper than `t` inside the source.
struct Cavity {
    bvh: cad::measure::Bvh,
    t: f64,
}

impl makepad_csg_sdf::Sdf3 for Cavity {
    fn distance(&self, p: Vec3d) -> f64 {
        self.bvh.signed_distance(p) + self.t
    }
}

/// Hollow `source` to walls of `t`: the outer skin stays exactly as it was; the
/// cavity (everything deeper than the wall) is meshed from the solid's distance
/// field at `cell` and cut away, opened through the top or bottom face when asked.
fn shell(source: &TriMesh, t: f64, open: Open, cell: Option<f64>, draft: f64) -> Result<TriMesh, String> {
    if !(t > 0.0) {
        return Err("'thickness' must be positive".into());
    }
    let bb = source.bounding_box();
    let ext = bb.max - bb.min;
    let smallest = ext.x.min(ext.y).min(ext.z);
    if 2.0 * t >= smallest {
        return Err(format!("a {t} wall fills the object (its thinnest extent is {smallest:.2})"));
    }
    let cell = cell.unwrap_or((t / 3.0).clamp(0.1, 2.0) * draft);
    let largest = ext.x.max(ext.y).max(ext.z);
    let depth = ((largest / cell).log2().ceil() as usize).clamp(4, 8);
    let field = Cavity { bvh: cad::measure::Bvh::build(source), t };
    let pad = dvec3(cell, cell, cell);
    let mut cavity = makepad_csg_sdf::sdf_to_mesh(field, bb.min - pad, bb.max + pad, depth);
    if cavity.triangles.is_empty() {
        return Err("the walls fill the object: nothing left to hollow".into());
    }
    if cad::signed_volume(&cavity) < 0.0 {
        for tri in &mut cavity.triangles {
            tri.swap(1, 2);
        }
    }
    // Open it: the cavity's outline just inside the open face, carried out past it.
    let face = match open {
        Open::Top => Some(true),
        Open::Bottom => Some(false),
        Open::Closed => None,
    };
    if let Some(top) = face {
        let cut_y = if top { bb.max.y - t - cell } else { bb.min.y + t + cell };
        let regions = cad::measure::section(&cavity, 1, cut_y);
        if regions.is_empty() {
            return Err("the cavity does not reach the open face: a thinner wall or a smaller cell".into());
        }
        let (from, to) = if top { (cut_y - cell, bb.max.y + 1.0) } else { (bb.min.y - 1.0, cut_y + cell) };
        let mut prism = cad::extrude(&regions, to - from, 0.0, false)?;
        // Section (u, v) = model (x, z); extrude +Z = model +Y.
        for p in &mut prism.vertices {
            *p = dvec3(p.x, from + p.z, p.y);
        }
        if cad::signed_volume(&prism) < 0.0 {
            for tri in &mut prism.triangles {
                tri.swap(1, 2);
            }
        }
        cavity = cad::union(&cavity, &prism);
    }
    let out = cad::difference(source, &cavity);
    if out.triangles.is_empty() {
        return Err("the shell removed the whole object".into());
    }
    Ok(out)
}
