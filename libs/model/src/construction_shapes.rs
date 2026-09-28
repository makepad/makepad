//! Parametric solids: analytic primitives and architectural pieces. Every
//! generator emits closed, outward-wound shells with metre-scale UVs (one UV
//! unit is one metre of surface) and explicit corner normals, so tiling
//! materials read at real size and curved surfaces shade smoothly without a
//! separate normals pass. Openings, stairs and roofs are built constructively,
//! never by boolean cuts, so they stay exact and cheap at any size.
use super::*;
use crate::service::float;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU};

pub(super) const MAX_PROFILE: usize = 512;
const MAX_OPENINGS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Opening {
    at: f64, width: f64, height: f64, sill: f64,
    /// Frame bar width (0 = bare opening) and depth; glass fills a framed window.
    frame: f64, frame_depth: f64, frame_material: u32, glass: Option<u32>, mullions: bool,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Wall {
    path: Vec<[f64; 2]>, closed: bool, height: f64, thickness: f64, base: f64,
    openings: Vec<Opening>, inner_material: Option<u32>, trim_material: Option<u32>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum RoofKind { Gable, Hip, Flat }
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Roof {
    kind: RoofKind, center: [f64; 3], size: [f64; 2], pitch: f64, overhang: f64, thickness: f64,
    /// Gable ridge runs along X (0) or Z (2). Hip roofs ridge along the longer side.
    ridge: usize, trim_material: Option<u32>, gable_material: Option<u32>, gable_thickness: f64,
}
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Shape {
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

fn opt_float(v: &Value, key: &str, default: f64) -> Result<f64> { v.get(key).map(float).transpose().map(|x| x.unwrap_or(default)) }
fn opt_int(v: &Value, key: &str, default: u32) -> Result<u32> { v.get(key).map(integer).transpose().map(|x| x.unwrap_or(default)) }
fn opt_bool(v: &Value, key: &str, default: bool) -> Result<bool> {
    v.get(key).map(|b| b.as_bool().ok_or(Error::Invalid("expected boolean"))).transpose().map(|x| x.unwrap_or(default))
}
fn opt_material(v: &Value, key: &str) -> Result<Option<u32>> {
    match v.get(key) { None | Some(Value::Null) => Ok(None), Some(m) => integer(m).map(Some) }
}
fn positive(v: f64, what: &'static str) -> Result<f64> { if v.is_finite() && v > 0. && v <= 1e5 { Ok(v) } else { Err(Error::Invalid(what)) } }
fn in_range(v: u32, lo: u32, hi: u32, what: &'static str) -> Result<u32> { if (lo..=hi).contains(&v) { Ok(v) } else { Err(Error::Invalid(what)) } }
fn f(v: f64) -> Value { Value::F64(v) }
fn int(v: u32) -> Value { Value::Int(v as i64) }
fn vec2s(v: &[[f64; 2]]) -> Value { Value::Arr(v.iter().map(|p| Value::Arr(p.iter().copied().map(Value::F64).collect())).collect()) }
fn vec3v(p: [f64; 3]) -> Value { Value::Arr(p.iter().copied().map(Value::F64).collect()) }
fn material_value(m: Option<u32>) -> Value { m.map_or(Value::Null, int) }

pub(super) const SHAPE_OPS: [&str; 10] = ["box", "capsule", "cone", "torus", "wedge", "extrusion", "tube", "wall", "stair", "roof"];

impl Shape {
    pub(super) fn parse(op: &str, v: &Value) -> Result<Option<Self>> {
        Ok(Some(match op {
            "box" => {
                fields(v, &["op", "object", "size", "radius", "segments", "material"])?;
                let size: [f64; 3] = array(need(v, "size")?)?;
                for s in size { positive(s, "box size must be positive")?; }
                let radius = opt_float(v, "radius", 0.)?;
                if !radius.is_finite() || radius < 0. { return Err(Error::Invalid("box radius")); }
                Shape::Box { size, radius, segments: in_range(opt_int(v, "segments", 3)?, 1, 16, "box segments 1..16")? }
            }
            "capsule" => {
                fields(v, &["op", "object", "radius", "height", "segments", "rings", "material"])?;
                let radius = positive(float(need(v, "radius")?)?, "capsule radius")?;
                let height = float(need(v, "height")?)?;
                if !height.is_finite() || height < radius * 2. - 1e-9 { return Err(Error::Invalid("capsule height must be at least twice its radius")); }
                Shape::Capsule { radius, height, segments: in_range(opt_int(v, "segments", 24)?, 3, 128, "capsule segments")?, rings: in_range(opt_int(v, "rings", 6)?, 1, 64, "capsule rings")? }
            }
            "cone" => {
                fields(v, &["op", "object", "radius", "top_radius", "height", "segments", "smooth", "material"])?;
                let bottom = float(need(v, "radius")?)?;
                let top = opt_float(v, "top_radius", 0.)?;
                if !(bottom.is_finite() && top.is_finite()) || bottom < 0. || top < 0. || bottom + top <= 0. { return Err(Error::Invalid("cone radii")); }
                Shape::Cone { bottom, top, height: positive(float(need(v, "height")?)?, "cone height")?, segments: in_range(opt_int(v, "segments", 24)?, 3, 128, "cone segments")?, smooth: opt_bool(v, "smooth", true)? }
            }
            "torus" => {
                fields(v, &["op", "object", "radius", "tube", "segments", "sides", "material"])?;
                let radius = positive(float(need(v, "radius")?)?, "torus radius")?;
                let tube = positive(float(need(v, "tube")?)?, "torus tube")?;
                if tube >= radius { return Err(Error::Invalid("torus tube must be thinner than its radius")); }
                Shape::Torus { radius, tube, segments: in_range(opt_int(v, "segments", 32)?, 3, 128, "torus segments")?, sides: in_range(opt_int(v, "sides", 12)?, 3, 64, "torus sides")? }
            }
            "wedge" => {
                fields(v, &["op", "object", "size", "material"])?;
                let size: [f64; 3] = array(need(v, "size")?)?;
                for s in size { positive(s, "wedge size must be positive")?; }
                Shape::Wedge { size }
            }
            "extrusion" => {
                fields(v, &["op", "object", "profile", "depth", "axis", "center", "corner_radius", "corner_segments", "bevel", "bevel_segments", "material"])?;
                let profile = points2(need(v, "profile")?)?;
                let axis = opt_int(v, "axis", 2)? as usize;
                if axis > 2 { return Err(Error::Invalid("extrude axis must be 0, 1 or 2")); }
                let corner_radius = opt_float(v, "corner_radius", 0.)?;
                let bevel = opt_float(v, "bevel", 0.)?;
                if !(corner_radius.is_finite() && bevel.is_finite()) || corner_radius < 0. || bevel < 0. { return Err(Error::Invalid("extrude corner_radius/bevel")); }
                Shape::Extrude { profile, depth: positive(float(need(v, "depth")?)?, "extrude depth")?, axis, center: opt_bool(v, "center", true)?,
                    corner_radius, corner_segments: in_range(opt_int(v, "corner_segments", 4)?, 1, 32, "extrude corner_segments")?,
                    bevel, bevel_segments: in_range(opt_int(v, "bevel_segments", 3)?, 1, 16, "extrude bevel_segments")? }
            }
            "tube" => {
                fields(v, &["op", "object", "path", "radius", "segments", "caps", "material"])?;
                let path = need(v, "path")?.as_arr().filter(|a| (2..=MAX_PROFILE).contains(&a.len())).ok_or(Error::Invalid("tube path needs 2..512 points"))?
                    .iter().map(array).collect::<Result<Vec<[f64; 3]>>>()?;
                Shape::Tube { path, radius: positive(float(need(v, "radius")?)?, "tube radius")?, segments: in_range(opt_int(v, "segments", 12)?, 3, 64, "tube segments")?, caps: opt_bool(v, "caps", true)? }
            }
            "wall" => {
                fields(v, &["op", "object", "path", "closed", "height", "thickness", "base", "openings", "material", "inner_material", "trim_material"])?;
                let path = points2(need(v, "path")?)?;
                let closed = opt_bool(v, "closed", false)?;
                if path.len() < if closed { 3 } else { 2 } { return Err(Error::Invalid("wall path needs two points (three when closed)")); }
                let height = positive(float(need(v, "height")?)?, "wall height")?;
                let thickness = positive(float(need(v, "thickness")?)?, "wall thickness")?;
                let base = opt_float(v, "base", 0.)?;
                if !base.is_finite() { return Err(Error::Invalid("wall base")); }
                let openings = match v.get("openings") {
                    None => Vec::new(),
                    Some(list) => {
                        let list = list.as_arr().filter(|a| a.len() <= MAX_OPENINGS).ok_or(Error::Invalid("wall openings must be an array of at most 64"))?;
                        list.iter().map(|o| {
                            fields(o, &["at", "width", "height", "sill", "frame", "frame_depth", "frame_material", "glass", "mullions"])?;
                            let sill = opt_float(o, "sill", 0.)?;
                            let frame = opt_float(o, "frame", 0.)?;
                            let frame_depth = opt_float(o, "frame_depth", 0.)?;
                            if !(sill.is_finite() && frame.is_finite() && frame_depth.is_finite()) || sill < 0. || frame < 0. || frame_depth < 0. { return Err(Error::Invalid("opening sill/frame")); }
                            Ok(Opening { at: float(need(o, "at")?)?, width: positive(float(need(o, "width")?)?, "opening width")?, height: positive(float(need(o, "height")?)?, "opening height")?,
                                sill, frame, frame_depth, frame_material: opt_int(o, "frame_material", 0)?, glass: opt_material(o, "glass")?, mullions: opt_bool(o, "mullions", false)? })
                        }).collect::<Result<Vec<_>>>()?
                    }
                };
                Shape::Wall(Wall { path, closed, height, thickness, base, openings, inner_material: opt_material(v, "inner_material")?, trim_material: opt_material(v, "trim_material")? })
            }
            "stair" => {
                fields(v, &["op", "object", "from", "to", "width", "steps", "thickness", "material"])?;
                let from: [f64; 3] = array(need(v, "from")?)?;
                let to: [f64; 3] = array(need(v, "to")?)?;
                let rise = to[1] - from[1];
                if !(rise.is_finite() && rise > 0.05) || (to[0] - from[0]).hypot(to[2] - from[2]) < 0.1 { return Err(Error::Invalid("stair must climb from `from` up to a higher, horizontally distant `to`")); }
                let steps = opt_int(v, "steps", 0)?;
                let steps = if steps == 0 { ((rise / 0.18).round() as u32).max(2) } else { steps };
                Shape::Stair { from, to, width: positive(float(need(v, "width")?)?, "stair width")?, steps: in_range(steps, 1, 256, "stair steps 1..256")?, thickness: positive(opt_float(v, "thickness", 0.15)?, "stair thickness")? }
            }
            "roof" => {
                fields(v, &["op", "object", "kind", "center", "size", "pitch", "overhang", "thickness", "ridge", "material", "trim_material", "gable_material", "gable_thickness"])?;
                let kind = match text(v, "kind")? { "gable" => RoofKind::Gable, "hip" => RoofKind::Hip, "flat" => RoofKind::Flat, _ => return Err(Error::Invalid("roof kind must be gable, hip or flat")) };
                let size: [f64; 2] = array(need(v, "size")?)?;
                for s in size { positive(s, "roof size")?; }
                let pitch = opt_float(v, "pitch", 35.)?;
                if !(pitch.is_finite() && (0. ..=75.).contains(&pitch)) || (kind != RoofKind::Flat && pitch < 1.) { return Err(Error::Invalid("roof pitch must be 1..75 degrees")); }
                let overhang = opt_float(v, "overhang", 0.3)?;
                if !overhang.is_finite() || overhang < 0. { return Err(Error::Invalid("roof overhang")); }
                let ridge = match v.get("ridge").map(|r| r.as_str().ok_or(Error::Invalid("roof ridge must be \"x\" or \"z\""))).transpose()? {
                    None => if size[0] >= size[1] { 0 } else { 2 }, Some("x") => 0, Some("z") => 2, _ => return Err(Error::Invalid("roof ridge must be \"x\" or \"z\"")),
                };
                Shape::Roof(Roof { kind, center: array(need(v, "center")?)?, size, pitch, overhang, thickness: positive(opt_float(v, "thickness", 0.12)?, "roof thickness")?, ridge,
                    trim_material: opt_material(v, "trim_material")?, gable_material: opt_material(v, "gable_material")?, gable_thickness: opt_float(v, "gable_thickness", 0.)?.max(0.) })
            }
            _ => return Ok(None),
        }))
    }
    pub(super) fn value(&self) -> (&'static str, Vec<(&'static str, Value)>) {
        match self {
            Shape::Box { size, radius, segments } => ("box", vec![("size", vec3v(*size)), ("radius", f(*radius)), ("segments", int(*segments))]),
            Shape::Capsule { radius, height, segments, rings } => ("capsule", vec![("radius", f(*radius)), ("height", f(*height)), ("segments", int(*segments)), ("rings", int(*rings))]),
            Shape::Cone { bottom, top, height, segments, smooth } => ("cone", vec![("radius", f(*bottom)), ("top_radius", f(*top)), ("height", f(*height)), ("segments", int(*segments)), ("smooth", Value::Bool(*smooth))]),
            Shape::Torus { radius, tube, segments, sides } => ("torus", vec![("radius", f(*radius)), ("tube", f(*tube)), ("segments", int(*segments)), ("sides", int(*sides))]),
            Shape::Wedge { size } => ("wedge", vec![("size", vec3v(*size))]),
            Shape::Extrude { profile, depth, axis, center, corner_radius, corner_segments, bevel, bevel_segments } => ("extrusion", vec![("profile", vec2s(profile)), ("depth", f(*depth)), ("axis", int(*axis as u32)), ("center", Value::Bool(*center)),
                ("corner_radius", f(*corner_radius)), ("corner_segments", int(*corner_segments)), ("bevel", f(*bevel)), ("bevel_segments", int(*bevel_segments))]),
            Shape::Tube { path, radius, segments, caps } => ("tube", vec![("path", Value::Arr(path.iter().map(|p| vec3v(*p)).collect())), ("radius", f(*radius)), ("segments", int(*segments)), ("caps", Value::Bool(*caps))]),
            Shape::Wall(w) => ("wall", vec![("path", vec2s(&w.path)), ("closed", Value::Bool(w.closed)), ("height", f(w.height)), ("thickness", f(w.thickness)), ("base", f(w.base)),
                ("openings", Value::Arr(w.openings.iter().map(|o| json::obj(vec![("at", f(o.at)), ("width", f(o.width)), ("height", f(o.height)), ("sill", f(o.sill)), ("frame", f(o.frame)),
                    ("frame_depth", f(o.frame_depth)), ("frame_material", int(o.frame_material)), ("glass", material_value(o.glass)), ("mullions", Value::Bool(o.mullions))])).collect())),
                ("inner_material", material_value(w.inner_material)), ("trim_material", material_value(w.trim_material))]),
            Shape::Stair { from, to, width, steps, thickness } => ("stair", vec![("from", vec3v(*from)), ("to", vec3v(*to)), ("width", f(*width)), ("steps", int(*steps)), ("thickness", f(*thickness))]),
            Shape::Roof(r) => ("roof", vec![("kind", json::s(match r.kind { RoofKind::Gable => "gable", RoofKind::Hip => "hip", RoofKind::Flat => "flat" })), ("center", vec3v(r.center)),
                ("size", Value::Arr(vec![f(r.size[0]), f(r.size[1])])), ("pitch", f(r.pitch)), ("overhang", f(r.overhang)), ("thickness", f(r.thickness)), ("ridge", json::s(if r.ridge == 0 { "x" } else { "z" })),
                ("trim_material", material_value(r.trim_material)), ("gable_material", material_value(r.gable_material)), ("gable_thickness", f(r.gable_thickness))]),
        }
    }
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

fn points2(v: &Value) -> Result<Vec<[f64; 2]>> {
    v.as_arr().filter(|a| (2..=MAX_PROFILE).contains(&a.len())).ok_or(Error::Invalid("2D point list needs 2..512 points"))?.iter().map(array).collect()
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
        let mut out: Vec<f64> = (0..=m).rev().map(|k| -(inner + r * (FRAC_PI_4 * k as f64 / m as f64).tan())).collect();
        for k in 0..=m { out.push(inner + r * (FRAC_PI_4 * k as f64 / m as f64).tan()); }
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
        if r <= 1e-12 { vec![b.vertex([0., y, 0.])] } else { (0..n).map(|j| { let t = angle(j as f64); b.vertex([r * t.sin(), y, r * t.cos()]) }).collect() }
    }).collect();
    let circumference = TAU * rings.iter().map(|r| r[0]).fold(0., f64::max);
    let mut v_along = vec![0.];
    for i in 1..rings.len() { v_along.push(v_along[i - 1] + (rings[i][0] - rings[i - 1][0]).hypot(rings[i][1] - rings[i - 1][1])); }
    let normal = |ring: &[f64; 4], t: f64| [ring[2] * t.sin(), ring[3], ring[2] * t.cos()];
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
        profile.push([radius * phi.cos(), -half + radius * phi.sin(), phi.cos(), phi.sin()]);
    }
    for i in 0..=rings {
        if i == 0 && half <= 1e-9 { continue; }
        let phi = FRAC_PI_2 * i as f64 / rings as f64;
        profile.push([radius * phi.cos(), half + radius * phi.sin(), phi.cos(), phi.sin()]);
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
    let dir = |i: usize, j: usize| { let (t, p) = (TAU * i as f64 / n as f64, TAU * j as f64 / m as f64); [t.sin() * p.cos(), p.sin(), t.cos() * p.cos()] };
    let ids: Vec<Vec<u32>> = (0..n).map(|i| (0..m).map(|j| {
        let d = dir(i, j); let t = TAU * i as f64 / n as f64;
        b.vertex(add([radius * t.sin(), 0., radius * t.cos()], mul(d, tube)))
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
        if p.last().is_none_or(|l: &[f64; 2]| (l[0] - q[0]).hypot(l[1] - q[1]) > 1e-7) { p.push(q); }
    }
    while p.len() > 1 && (p[0][0] - p[p.len() - 1][0]).hypot(p[0][1] - p[p.len() - 1][1]) <= 1e-7 { p.pop(); }
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
        let (l1, l2) = ((prev[0] - cur[0]).hypot(prev[1] - cur[1]), (next[0] - cur[0]).hypot(next[1] - cur[1]));
        let e1 = [(prev[0] - cur[0]) / l1, (prev[1] - cur[1]) / l1];
        let e2 = [(next[0] - cur[0]) / l2, (next[1] - cur[1]) / l2];
        let cos = (e1[0] * e2[0] + e1[1] * e2[1]).clamp(-1., 1.);
        let theta = cos.acos();
        if theta > std::f64::consts::PI - 1e-3 || theta < 1e-3 { out.push(cur); continue; }
        // Tangent distance, limited so neighbouring fillets never overlap.
        let t = (radius / (theta * 0.5).tan()).min(0.49 * l1.min(l2));
        let r = t * (theta * 0.5).tan();
        let bis = { let s = [e1[0] + e2[0], e1[1] + e2[1]]; let l = s[0].hypot(s[1]); [s[0] / l, s[1] / l] };
        let d = r / (theta * 0.5).sin();
        let c = [cur[0] + bis[0] * d, cur[1] + bis[1] * d];
        let (a, bpt) = ([cur[0] + e1[0] * t, cur[1] + e1[1] * t], [cur[0] + e2[0] * t, cur[1] + e2[1] * t]);
        let (a0, mut a1) = ((a[1] - c[1]).atan2(a[0] - c[0]), (bpt[1] - c[1]).atan2(bpt[0] - c[0]));
        while a1 - a0 > std::f64::consts::PI { a1 -= TAU; }
        while a0 - a1 > std::f64::consts::PI { a1 += TAU; }
        for k in 0..=segments {
            let t = a0 + (a1 - a0) * k as f64 / segments as f64;
            out.push([c[0] + r * t.cos(), c[1] + r * t.sin()]);
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
    let edge_normal: Vec<[f64; 2]> = (0..n).map(|i| { let (a, c) = (p[i], p[(i + 1) % n]); let (dx, dy) = (c[0] - a[0], c[1] - a[1]); let l = dx.hypot(dy); [dy / l, -dx / l] }).collect();
    let prev = |i: usize| edge_normal[(i + n - 1) % n];
    let smooth: Vec<bool> = (0..n).map(|i| { let (a, c) = (prev(i), edge_normal[i]); a[0] * c[0] + a[1] * c[1] > 0.766 }).collect();
    let vertex_normal: Vec<[f64; 2]> = (0..n).map(|i| { let s = [prev(i)[0] + edge_normal[i][0], prev(i)[1] + edge_normal[i][1]]; let l = s[0].hypot(s[1]).max(1e-12); [s[0] / l, s[1] / l] }).collect();
    let miter: Vec<[f64; 2]> = (0..n).map(|i| { let (a, c) = (prev(i), edge_normal[i]); let d = (1. + a[0] * c[0] + a[1] * c[1]).max(0.25); [(a[0] + c[0]) / d, (a[1] + c[1]) / d] }).collect();
    let bevel = bevel.min((w1 - w0) * 0.45);
    // Ring layers: (inset, w, theta from cap normal). theta=90deg is the side wall.
    let mut layers: Vec<(f64, f64, f64, f64)> = Vec::new();
    if bevel > 1e-9 {
        for k in 0..=bevel_segments { let t = FRAC_PI_2 * k as f64 / bevel_segments as f64; layers.push((bevel * (1. - t.sin()), w0 + bevel * (1. - t.cos()), t, -1.)); }
        for k in (0..=bevel_segments).rev() { let t = FRAC_PI_2 * k as f64 / bevel_segments as f64; layers.push((bevel * (1. - t.sin()), w1 - bevel * (1. - t.cos()), t, 1.)); }
    } else {
        layers.push((0., w0, FRAC_PI_2, -1.));
        layers.push((0., w1, FRAC_PI_2, 1.));
    }
    let rings: Vec<Vec<u32>> = layers.iter().map(|&(inset, w, ..)| (0..n).map(|i| b.vertex(frame.at(p[i][0] - miter[i][0] * inset, p[i][1] - miter[i][1] * inset, w))).collect()).collect();
    let mut along = vec![0.];
    for i in 0..n { along.push(along[i] + (p[(i + 1) % n][0] - p[i][0]).hypot(p[(i + 1) % n][1] - p[i][1])); }
    let flip = frame.mirrored();
    let push = |b: &mut Builder, mut verts: Vec<u32>, mut uvs: Vec<[f64; 2]>, mut normals: Vec<[f64; 3]>, material: u32| {
        if flip { verts.reverse(); uvs.reverse(); normals.reverse(); }
        b.face(verts, uvs, Some(normals), material);
    };
    for k in 0..layers.len() - 1 {
        let ((_, wa, ta, sa), (_, wb, tb, sb)) = (layers[k], layers[k + 1]);
        for i in 0..n {
            let j = (i + 1) % n;
            let side_n = |vi: usize, t: f64, s: f64| { let n2 = if smooth[vi] { vertex_normal[vi] } else { edge_normal[i] }; frame.dir(n2[0] * t.sin(), n2[1] * t.sin(), s * t.cos()) };
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
        let ring_dirs: Vec<[f64; 3]> = (0..segments).map(|j| { let t = TAU * j as f64 / segments as f64; add(mul(nrm, t.cos()), mul(binormal, t.sin())) }).collect();
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
    let dir = |s: usize| -> Result<[f64; 2]> { let (a, c) = (pts[s], pts[(s + 1) % pts.len()]); let l = (c[0] - a[0]).hypot(c[1] - a[1]); if l < 1e-6 { return Err(Error::Invalid("wall path has a zero-length segment")); } Ok([(c[0] - a[0]) / l, (c[1] - a[1]) / l]) };
    let seg_len = |s: usize| { let (a, c) = (pts[s], pts[(s + 1) % pts.len()]); (c[0] - a[0]).hypot(c[1] - a[1]) };
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
    let run = (to[0] - from[0]).hypot(to[2] - from[2]);
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
    let slope = r.pitch.to_radians().tan();
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
            let vt = r.thickness / r.pitch.to_radians().cos();
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
