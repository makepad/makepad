//! The typed SDF shape: a tree of analytic distance nodes and compiled Splash
//! distance kernels, with a canonical binary encoding (its key) and an evaluator
//! that needs no script VM for the analytic nodes.
//!
//! Analytic nodes match the Splash SDF library of the kernel prelude
//! (`sdf_sphere`, `smooth_union`, `sdf_shell` ...): same formulas, same
//! conventions (shapes centred on the origin, Y up, full sizes). A
//! [`SdfKernel`] is a Splash `fn(p)` compiled by the kernel compiler: `key` is
//! its program identity and `field` the compiled field.

use crate::sdf::Sdf3;
use makepad_csg_math::Vec3d;
use std::sync::Arc;

pub type P3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdfOp {
    Union,
    Difference,
    Intersection,
}

/// A compiled Splash distance kernel.
#[derive(Clone)]
pub struct SdfKernel {
    /// The program's identity (its whole IR, constants included).
    pub key: u64,
    pub field: Arc<dyn Sdf3 + Send + Sync>,
}

impl std::fmt::Debug for SdfKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SdfKernel({:016x})", self.key)
    }
}

/// One node of a shape.
#[derive(Clone, Debug)]
pub enum SdfNode {
    Sphere { radius: f64 },
    /// `radius` rounds every edge inside `size`.
    Box { size: P3, radius: f64 },
    /// Along Y, `height` high: radius at the bottom, `top_radius` at the top.
    Cylinder { radius: f64, top_radius: f64, height: f64 },
    /// In the XZ plane.
    Torus { radius: f64, tube: f64 },
    Capsule { a: P3, b: P3, radius: f64 },
    Ellipsoid { radii: P3 },
    /// A gyroid sheet of period and wall.
    Gyroid { period: f64, wall: f64 },
    /// The first of `of` combined with each next one in order; `blend` 0 is sharp.
    Combine { op: SdfOp, blend: f64, of: Vec<SdfNode> },
    /// A skin `thickness` thick each side of the surface.
    Shell { thickness: f64, of: Box<SdfNode> },
    /// The surface grown outward by `radius`.
    Round { radius: f64, of: Box<SdfNode> },
    /// `of` scaled, then turned about X, Y, Z (degrees), then moved by `at`.
    Transform { at: P3, rotate: P3, scale: f64, of: Box<SdfNode> },
    Kernel(SdfKernel),
    /// A triangle mesh's signed distance (its own frame; place it with a Transform).
    Mesh(Arc<crate::mesh_field::MeshField>),
    /// `of` in material `slot` of [`SdfShape::materials`] (0 is the default);
    /// the innermost Material wins. Slots through the other nodes: see
    /// [`distance_slot`].
    Material { slot: u8, within: Option<Box<SdfNode>>, of: Box<SdfNode> },
    /// `of` placed at each of `places`: the nearest copy. `of` keeps its own
    /// key ([`SdfNode::key`]), so moving copies re-bakes nothing.
    Instance { places: Vec<SdfPlace>, of: Box<SdfNode> },
    /// `of` (object space, rest pose) bound to bone `id`, whose rest frame is
    /// `rest`; the pose is a renderer's runtime data.
    Bone { id: u32, rest: SdfPlace, of: Box<SdfNode> },
    /// `of` warped, its distance divided by `lipschitz` (made by
    /// [`SdfNode::deform`] from the warp and `of`'s bounds).
    Deform { deform: SdfDeform, lipschitz: f64, of: Box<SdfNode> },
    /// The picking id of one source part: the low 32 bits of the sha256 of
    /// its source id (`bolts[2]`, `left.body`), see [`part_id`].
    Id { id: u32, of: Box<SdfNode> },
}

/// A rigid placement with a uniform scale: scaled, then turned by the unit
/// quaternion `rotate` (x y z w), then moved by `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfPlace {
    pub at: P3,
    pub rotate: [f64; 4],
    pub scale: f64,
}

impl SdfPlace {
    pub const IDENTITY: SdfPlace = SdfPlace { at: [0.0; 3], rotate: [0.0, 0.0, 0.0, 1.0], scale: 1.0 };

    /// A point of the placed node into the outer frame.
    pub fn out_of(&self, q: P3) -> P3 {
        let r = quat_rotate(self.rotate, q.map(|x| x * self.scale));
        std::array::from_fn(|i| r[i] + self.at[i])
    }

    /// A point of the outer frame into the placed node.
    pub fn into(&self, p: P3) -> P3 {
        let [x, y, z, w] = self.rotate;
        quat_rotate([-x, -y, -z, w], std::array::from_fn(|i| p[i] - self.at[i])).map(|v| v / self.scale)
    }
}

/// `v` turned by the unit quaternion `q` (x y z w).
pub fn quat_rotate(q: [f64; 4], v: P3) -> P3 {
    let u = [q[0], q[1], q[2]];
    let cross = |a: P3, b: P3| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v).map(|c| c * 2.0);
    let ut = cross(u, t);
    std::array::from_fn(|i| v[i] + q[3] * t[i] + ut[i])
}

/// The unit quaternion of turns about X, then Y, then Z (degrees): the
/// rotation of [`SdfNode::Transform`].
pub fn quat_from_degrees(deg: P3) -> [f64; 4] {
    let h = deg.map(|d| d.to_radians() * 0.5);
    let (sx, cx) = h[0].sin_cos();
    let (sy, cy) = h[1].sin_cos();
    let (sz, cz) = h[2].sin_cos();
    // q = qz * qy * qx
    [sx * cy * cz - cx * sy * sz, cx * sy * cz + sx * cy * sz, cx * cy * sz - sx * sy * cz, cx * cy * cz + sx * sy * sz]
}

/// A warp of [`SdfNode::Deform`]. Each works about the node's own axes
/// through its origin; a Transform round it picks another axis and centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SdfDeform {
    /// Turned about Y by `rate` radians per unit of height.
    Twist { rate: f64 },
    /// Bent in the XY plane about Z: turned by `rate` radians per unit of X.
    Bend { rate: f64 },
    /// X and Z scaled by `1 + rate * y` (kept above zero over the node).
    Taper { rate: f64 },
    /// Mirrored across the plane through the origin normal to `axis` (0 x,
    /// 1 y, 2 z): the half on the positive side, twice.
    Mirror { axis: u8 },
}

/// The smallest taper scale used: a taper that reaches zero is clamped here.
const TAPER_MIN: f64 = 1e-3;

impl SdfDeform {
    /// A bound on how much the warp stretches distances over a node with
    /// bounds `of` (its own frame); `None` bounds count as a unit box.
    pub fn lipschitz(&self, of: Option<&crate::bounds::Bounds>) -> f64 {
        let b = of.copied().unwrap_or(([-1.0; 3], [1.0; 3]));
        let ext = |i: usize| b.0[i].abs().max(b.1[i].abs());
        // The largest stretch of a unit shear by `s`.
        let shear = |s: f64| (s.abs() + (s * s + 4.0).sqrt()) * 0.5;
        match *self {
            SdfDeform::Twist { rate } => shear(rate * ext(0).hypot(ext(2))),
            SdfDeform::Bend { rate } => shear(rate * ext(0).hypot(ext(1))),
            SdfDeform::Taper { rate } => {
                let (s0, s1) = (1.0 + rate * b.0[1], 1.0 + rate * b.1[1]);
                let smin = s0.min(s1).max(TAPER_MIN);
                (1.0 / smin).max(1.0) + rate.abs() * ext(0).hypot(ext(2)) / smin
            }
            SdfDeform::Mirror { .. } => 1.0,
        }
    }

    /// A point of the outer frame into the warped node.
    pub fn warp(&self, p: Vec3d) -> Vec3d {
        match *self {
            SdfDeform::Twist { rate } => {
                let (s, c) = (rate * p.y).sin_cos();
                Vec3d::new(c * p.x + s * p.z, p.y, -s * p.x + c * p.z)
            }
            SdfDeform::Bend { rate } => {
                let (s, c) = (rate * p.x).sin_cos();
                Vec3d::new(c * p.x + s * p.y, -s * p.x + c * p.y, p.z)
            }
            SdfDeform::Taper { rate } => {
                let k = 1.0 / (1.0 + rate * p.y).max(TAPER_MIN);
                Vec3d::new(p.x * k, p.y, p.z * k)
            }
            SdfDeform::Mirror { axis } => match axis {
                0 => Vec3d::new(p.x.abs(), p.y, p.z),
                1 => Vec3d::new(p.x, p.y.abs(), p.z),
                _ => Vec3d::new(p.x, p.y, p.z.abs()),
            },
        }
    }
}

impl SdfNode {
    /// `of` warped by `deform`, with its Lipschitz bound.
    pub fn deform(deform: SdfDeform, of: SdfNode) -> SdfNode {
        let lipschitz = deform.lipschitz(crate::bounds::node_bounds(&of).as_ref());
        SdfNode::Deform { deform, lipschitz, of: Box::new(of) }
    }

    /// The canonical encoding of this subtree.
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(Vec::new());
        o.node(self);
        o.0
    }

    /// This subtree's own identity (a renderer bakes an instanced node once
    /// by it).
    pub fn key(&self) -> [u8; 32] {
        makepad_core_util::sha256::sha256(&self.encode())
    }
}

/// The picking id of a source part id: the low 32 bits of its sha256.
pub fn part_id(source_id: &str) -> u32 {
    let h = makepad_core_util::sha256::sha256(source_id.as_bytes());
    u32::from_le_bytes([h[0], h[1], h[2], h[3]])
}

/// A material of [`SdfShape::materials`]; outside the shape's key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SdfMaterial {
    pub albedo: [f32; 3],
    pub fade: f32,
    pub metal: f32,
    pub rough: f32,
    pub emissive: f32,
    pub emissive_color: [f32; 3],
    pub unlit: bool,
    pub colorize: bool,
    pub splat: SdfSplat,
}

impl Default for SdfMaterial {
    fn default() -> Self {
        SdfMaterial { albedo: [0.8; 3], fade: 0.0, metal: 0.0, rough: 0.5, emissive: 0.0, emissive_color: [1.0; 3], unlit: false, colorize: false, splat: SdfSplat::default() }
    }
}

/// A material's splat look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SdfSplat {
    pub size: f32,
    pub opacity: f32,
    pub fluff: f32,
    pub jitter_size: f32,
    pub jitter_pos: f32,
    pub jitter_rot: f32,
    pub jitter_bump: f32,
    pub jitter_metal: f32,
    pub jitter_bright: f32,
    pub jitter_color: f32,
}

/// A shape, a box it fits in and its material table (slot `i` is
/// `materials[i]`; a missing slot is the default material).
#[derive(Clone, Debug)]
pub struct SdfShape {
    pub root: SdfNode,
    pub min: P3,
    pub max: P3,
    /// Outside the key: a colour change re-meshes and re-bakes nothing.
    pub materials: Vec<SdfMaterial>,
}

// ---- encoding

const MAGIC: &[u8; 4] = b"SDF1";

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f(&mut self, v: f64) {
        self.u64(if v == 0.0 { 0 } else { v.to_bits() });
    }
    fn p3(&mut self, p: P3) {
        p.iter().for_each(|x| self.f(*x));
    }
    fn place(&mut self, p: &SdfPlace) {
        self.p3(p.at);
        p.rotate.iter().for_each(|x| self.f(*x));
        self.f(p.scale);
    }
    fn node(&mut self, n: &SdfNode) {
        match n {
            SdfNode::Sphere { radius } => {
                self.u8(0);
                self.f(*radius);
            }
            SdfNode::Box { size, radius } => {
                self.u8(1);
                self.p3(*size);
                self.f(*radius);
            }
            SdfNode::Cylinder { radius, top_radius, height } => {
                self.u8(2);
                self.f(*radius);
                self.f(*top_radius);
                self.f(*height);
            }
            SdfNode::Torus { radius, tube } => {
                self.u8(3);
                self.f(*radius);
                self.f(*tube);
            }
            SdfNode::Capsule { a, b, radius } => {
                self.u8(4);
                self.p3(*a);
                self.p3(*b);
                self.f(*radius);
            }
            SdfNode::Ellipsoid { radii } => {
                self.u8(5);
                self.p3(*radii);
            }
            SdfNode::Gyroid { period, wall } => {
                self.u8(6);
                self.f(*period);
                self.f(*wall);
            }
            SdfNode::Combine { op, blend, of } => {
                self.u8(7);
                self.u8(*op as u8);
                self.f(*blend);
                self.u64(of.len() as u64);
                of.iter().for_each(|c| self.node(c));
            }
            SdfNode::Shell { thickness, of } => {
                self.u8(8);
                self.f(*thickness);
                self.node(of);
            }
            SdfNode::Round { radius, of } => {
                self.u8(9);
                self.f(*radius);
                self.node(of);
            }
            SdfNode::Transform { at, rotate, scale, of } => {
                self.u8(10);
                self.p3(*at);
                self.p3(*rotate);
                self.f(*scale);
                self.node(of);
            }
            SdfNode::Kernel(k) => {
                self.u8(11);
                self.u64(k.key);
            }
            SdfNode::Mesh(m) => {
                self.u8(12);
                self.u64(m.key());
            }
            SdfNode::Material { slot, within, of } => {
                self.u8(13);
                self.u8(*slot);
                match within {
                    Some(w) => {
                        self.u8(1);
                        self.node(w);
                    }
                    None => self.u8(0),
                }
                self.node(of);
            }
            SdfNode::Instance { places, of } => {
                self.u8(14);
                self.u64(places.len() as u64);
                places.iter().for_each(|p| self.place(p));
                self.node(of);
            }
            SdfNode::Bone { id, rest, of } => {
                self.u8(15);
                self.u64(*id as u64);
                self.place(rest);
                self.node(of);
            }
            SdfNode::Deform { deform, lipschitz, of } => {
                self.u8(16);
                match *deform {
                    SdfDeform::Twist { rate } => {
                        self.u8(0);
                        self.f(rate);
                    }
                    SdfDeform::Bend { rate } => {
                        self.u8(1);
                        self.f(rate);
                    }
                    SdfDeform::Taper { rate } => {
                        self.u8(2);
                        self.f(rate);
                    }
                    SdfDeform::Mirror { axis } => {
                        self.u8(3);
                        self.u8(axis);
                    }
                }
                self.f(*lipschitz);
                self.node(of);
            }
            SdfNode::Id { id, of } => {
                self.u8(17);
                self.u64(*id as u64);
                self.node(of);
            }
        }
    }
}

impl SdfShape {
    /// The canonical encoding (each kernel by its key).
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(MAGIC.to_vec());
        o.p3(self.min);
        o.p3(self.max);
        o.node(&self.root);
        o.0
    }

    /// The canonical identity: the encoding's hash.
    pub fn key(&self) -> [u8; 32] {
        makepad_core_util::sha256::sha256(&self.encode())
    }

    /// The shape as a field.
    pub fn field(&self) -> Arc<dyn Sdf3 + Send + Sync> {
        Arc::new(Eval(self.root.clone()))
    }
}

struct Eval(SdfNode);

impl Sdf3 for Eval {
    fn distance(&self, p: Vec3d) -> f64 {
        distance(&self.0, p)
    }
    fn distance_many(&self, ps: &[Vec3d], out: &mut [f64]) {
        distance_many(&self.0, ps, out)
    }
}

/// [`distance`] at many points: a kernel answers its points in one call.
pub fn distance_many(n: &SdfNode, ps: &[Vec3d], out: &mut [f64]) {
    match n {
        SdfNode::Kernel(k) => k.field.distance_many(ps, out),
        SdfNode::Combine { op, blend, of } => {
            distance_many(&of[0], ps, out);
            let mut e = vec![0.0; ps.len()];
            for c in &of[1..] {
                distance_many(c, ps, &mut e);
                for (d, x) in out.iter_mut().zip(&e) {
                    *d = match op {
                        SdfOp::Union => smin(*d, *x, *blend),
                        SdfOp::Intersection => -smin(-*d, -*x, *blend),
                        SdfOp::Difference => -smin(-*d, *x, *blend),
                    };
                }
            }
        }
        SdfNode::Shell { thickness, of } => {
            distance_many(of, ps, out);
            out.iter_mut().for_each(|d| *d = d.abs() - thickness);
        }
        SdfNode::Round { radius, of } => {
            distance_many(of, ps, out);
            out.iter_mut().for_each(|d| *d -= radius);
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let r = rot(*rotate);
            let q: Vec<Vec3d> = ps
                .iter()
                .map(|p| {
                    let d = *p - Vec3d::new(at[0], at[1], at[2]);
                    Vec3d::new(r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z, r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z, r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z) * (1.0 / scale)
                })
                .collect();
            distance_many(of, &q, out);
            out.iter_mut().for_each(|d| *d *= scale);
        }
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => distance_many(of, ps, out),
        SdfNode::Instance { places, of } => {
            out.iter_mut().for_each(|d| *d = f64::INFINITY);
            let mut e = vec![0.0; ps.len()];
            for pl in places {
                let q: Vec<Vec3d> = ps.iter().map(|p| v3(pl.into([p.x, p.y, p.z]))).collect();
                distance_many(of, &q, &mut e);
                for (d, x) in out.iter_mut().zip(&e) {
                    *d = d.min(x * pl.scale);
                }
            }
        }
        SdfNode::Deform { deform, lipschitz, of } => {
            let q: Vec<Vec3d> = ps.iter().map(|p| deform.warp(*p)).collect();
            distance_many(of, &q, out);
            out.iter_mut().for_each(|d| *d /= lipschitz);
        }
        other => {
            for (p, o) in ps.iter().zip(out.iter_mut()) {
                *o = distance(other, *p);
            }
        }
    }
}

fn v3(p: P3) -> Vec3d {
    Vec3d::new(p[0], p[1], p[2])
}

/// The distance of one node at `p` and the material slot there:
/// - Material: the innermost wins (0 outside every Material); one with a
///   `within` region gives its slot only inside the region (distance <= 0)
///   and its distance is always its child's;
/// - union and smooth union: the nearer operand's (a tie keeps the earlier);
/// - difference: the base's (a cut surface takes the base material);
/// - intersection: the farther operand's (a tie keeps the earlier);
/// - Instance: the nearest copy's; every other node: its child's.
pub fn distance_slot(n: &SdfNode, p: Vec3d) -> (f64, u8) {
    slot_in(n, p, 0)
}

fn slot_in(n: &SdfNode, p: Vec3d, inherit: u8) -> (f64, u8) {
    match n {
        SdfNode::Material { slot, within, of } => {
            let here = match within {
                Some(w) if distance(w, p) > 0.0 => inherit,
                _ => *slot,
            };
            slot_in(of, p, here)
        }
        SdfNode::Combine { op, blend, of } => {
            let (mut d, mut slot) = slot_in(&of[0], p, inherit);
            for c in &of[1..] {
                let (e, s) = slot_in(c, p, inherit);
                match op {
                    SdfOp::Union => {
                        if e < d {
                            slot = s;
                        }
                        d = smin(d, e, *blend);
                    }
                    SdfOp::Intersection => {
                        if e > d {
                            slot = s;
                        }
                        d = -smin(-d, -e, *blend);
                    }
                    SdfOp::Difference => d = -smin(-d, e, *blend),
                }
            }
            (d, slot)
        }
        SdfNode::Shell { thickness, of } => {
            let (d, s) = slot_in(of, p, inherit);
            (d.abs() - thickness, s)
        }
        SdfNode::Round { radius, of } => {
            let (d, s) = slot_in(of, p, inherit);
            (d - radius, s)
        }
        SdfNode::Transform { at, rotate, scale, of } => {
            let (d, s) = slot_in(of, transform_into(*at, *rotate, *scale, p), inherit);
            (d * scale, s)
        }
        SdfNode::Instance { places, of } => places.iter().fold((f64::INFINITY, inherit), |acc, pl| {
            let (d, s) = slot_in(of, v3(pl.into([p.x, p.y, p.z])), inherit);
            if d * pl.scale < acc.0 {
                (d * pl.scale, s)
            } else {
                acc
            }
        }),
        SdfNode::Deform { deform, lipschitz, of } => {
            let (d, s) = slot_in(of, deform.warp(p), inherit);
            (d / lipschitz, s)
        }
        SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => slot_in(of, p, inherit),
        leaf => (distance(leaf, p), inherit),
    }
}

fn transform_into(at: P3, rotate: P3, scale: f64, p: Vec3d) -> Vec3d {
    let r = rot(rotate);
    let d = p - Vec3d::new(at[0], at[1], at[2]);
    // The inverse rotation is the transpose.
    Vec3d::new(r[0][0] * d.x + r[1][0] * d.y + r[2][0] * d.z, r[0][1] * d.x + r[1][1] * d.y + r[2][1] * d.z, r[0][2] * d.x + r[1][2] * d.y + r[2][2] * d.z) * (1.0 / scale)
}

fn smin(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn rot(deg: P3) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let m = |a: [[f64; 3]; 3], b: [[f64; 3]; 3]| -> [[f64; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    m(rz, m(ry, rx))
}

/// The distance of one node at `p`.
pub fn distance(n: &SdfNode, p: Vec3d) -> f64 {
    match n {
        SdfNode::Sphere { radius } => p.length() - radius,
        SdfNode::Box { size, radius } => {
            let q = [p.x.abs() - size[0] * 0.5 + radius, p.y.abs() - size[1] * 0.5 + radius, p.z.abs() - size[2] * 0.5 + radius];
            Vec3d::new(q[0].max(0.0), q[1].max(0.0), q[2].max(0.0)).length() + q[0].max(q[1].max(q[2])).min(0.0) - radius
        }
        SdfNode::Cylinder { radius, top_radius, height } => {
            let hh = height * 0.5;
            let qx = (p.x * p.x + p.z * p.z).sqrt();
            if radius == top_radius {
                let (dx, dy) = (qx - radius, p.y.abs() - hh);
                return (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt() + dx.max(dy).min(0.0);
            }
            let (r1, r2, qy) = (*radius, *top_radius, p.y);
            let (k2x, k2y) = (r2 - r1, 2.0 * hh);
            let cax = qx - qx.min(if qy < 0.0 { r1 } else { r2 });
            let cay = qy.abs() - hh;
            let t = (((r2 - qx) * k2x + (hh - qy) * k2y) / (k2x * k2x + k2y * k2y)).clamp(0.0, 1.0);
            let (cbx, cby) = (qx - r2 + k2x * t, qy - hh + k2y * t);
            let s = if cbx < 0.0 && cay < 0.0 { -1.0 } else { 1.0 };
            s * (cax * cax + cay * cay).min(cbx * cbx + cby * cby).sqrt()
        }
        SdfNode::Torus { radius, tube } => {
            let q = (p.x * p.x + p.z * p.z).sqrt() - radius;
            (q * q + p.y * p.y).sqrt() - tube
        }
        SdfNode::Capsule { a, b, radius } => {
            let (a, b) = (Vec3d::new(a[0], a[1], a[2]), Vec3d::new(b[0], b[1], b[2]));
            let (pa, ba) = (p - a, b - a);
            let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
            (pa - ba * h).length() - radius
        }
        SdfNode::Ellipsoid { radii: r } => {
            let k0 = Vec3d::new(p.x / r[0], p.y / r[1], p.z / r[2]).length();
            let k1 = Vec3d::new(p.x / (r[0] * r[0]), p.y / (r[1] * r[1]), p.z / (r[2] * r[2])).length();
            k0 * (k0 - 1.0) / k1.max(1e-30)
        }
        SdfNode::Gyroid { period, wall } => {
            let k = std::f64::consts::TAU / period;
            let g = (p.x * k).sin() * (p.y * k).cos() + (p.y * k).sin() * (p.z * k).cos() + (p.z * k).sin() * (p.x * k).cos();
            g.abs() / k - wall * 0.5
        }
        SdfNode::Combine { op, blend, of } => {
            let mut d = distance(&of[0], p);
            for c in &of[1..] {
                let e = distance(c, p);
                d = match op {
                    SdfOp::Union => smin(d, e, *blend),
                    SdfOp::Intersection => -smin(-d, -e, *blend),
                    SdfOp::Difference => -smin(-d, e, *blend),
                };
            }
            d
        }
        SdfNode::Shell { thickness, of } => distance(of, p).abs() - thickness,
        SdfNode::Round { radius, of } => distance(of, p) - radius,
        SdfNode::Transform { at, rotate, scale, of } => distance(of, transform_into(*at, *rotate, *scale, p)) * scale,
        SdfNode::Kernel(k) => k.field.distance(p),
        SdfNode::Mesh(m) => m.signed64(p),
        SdfNode::Material { of, .. } | SdfNode::Bone { of, .. } | SdfNode::Id { of, .. } => distance(of, p),
        SdfNode::Instance { places, of } => places.iter().map(|pl| distance(of, v3(pl.into([p.x, p.y, p.z]))) * pl.scale).fold(f64::INFINITY, f64::min),
        SdfNode::Deform { deform, lipschitz, of } => distance(of, deform.warp(p)) / lipschitz,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bounds::node_bounds;

    fn ball(r: f64, at: P3) -> SdfNode {
        SdfNode::Transform { at, rotate: [0.0; 3], scale: 1.0, of: Box::new(SdfNode::Sphere { radius: r }) }
    }

    /// Points on a lattice through `b`, grown a little.
    fn lattice(b: &crate::bounds::Bounds, n: usize) -> Vec<Vec3d> {
        let mut out = Vec::new();
        let g = |i: usize, k: usize| b.0[k] - 1.0 + (b.1[k] - b.0[k] + 2.0) * i as f64 / (n - 1) as f64;
        for x in 0..n {
            for y in 0..n {
                for z in 0..n {
                    out.push(Vec3d::new(g(x, 0), g(y, 1), g(z, 2)));
                }
            }
        }
        out
    }

    fn nodes() -> Vec<SdfNode> {
        let bar = SdfNode::Box { size: [4.0, 20.0, 8.0], radius: 0.5 };
        vec![
            SdfNode::Instance { places: vec![SdfPlace { at: [10.0, 0.0, 0.0], rotate: quat_from_degrees([0.0, 45.0, 0.0]), scale: 2.0 }, SdfPlace::IDENTITY], of: Box::new(bar.clone()) },
            SdfNode::deform(SdfDeform::Twist { rate: 0.1 }, bar.clone()),
            SdfNode::deform(SdfDeform::Bend { rate: 0.05 }, SdfNode::Box { size: [30.0, 4.0, 4.0], radius: 0.0 }),
            SdfNode::deform(SdfDeform::Taper { rate: -0.03 }, bar.clone()),
            SdfNode::deform(SdfDeform::Mirror { axis: 0 }, ball(3.0, [4.0, 1.0, 0.0])),
            SdfNode::Material { slot: 2, within: None, of: Box::new(SdfNode::Bone { id: 7, rest: SdfPlace::IDENTITY, of: Box::new(SdfNode::Id { id: part_id("arm"), of: Box::new(bar) }) }) },
        ]
    }

    #[test]
    fn quaternion_turns_like_a_transform() {
        let deg = [30.0, -50.0, 110.0];
        let bar = SdfNode::Box { size: [4.0, 20.0, 8.0], radius: 0.0 };
        let t = SdfNode::Transform { at: [1.0, 2.0, 3.0], rotate: deg, scale: 1.5, of: Box::new(bar.clone()) };
        let i = SdfNode::Instance { places: vec![SdfPlace { at: [1.0, 2.0, 3.0], rotate: quat_from_degrees(deg), scale: 1.5 }], of: Box::new(bar) };
        for p in lattice(&node_bounds(&t).unwrap(), 7) {
            assert!((distance(&t, p) - distance(&i, p)).abs() < 1e-9);
        }
    }

    #[test]
    fn new_nodes_are_bounded_and_lipschitz() {
        for n in nodes() {
            let b = node_bounds(&n).unwrap();
            let ps = lattice(&b, 15);
            let mut many = vec![0.0; ps.len()];
            distance_many(&n, &ps, &mut many);
            for (k, p) in ps.iter().enumerate() {
                let d = distance(&n, *p);
                assert!((d - many[k]).abs() < 1e-9);
                if d <= 0.0 {
                    assert!((0..3).all(|i| [p.x, p.y, p.z][i] >= b.0[i] - 1e-9 && [p.x, p.y, p.z][i] <= b.1[i] + 1e-9), "{n:?} solid at {p:?} outside {b:?}");
                }
                // Never more than the true distance's change between neighbours.
                if k + 1 < ps.len() {
                    let q = ps[k + 1];
                    assert!((d - distance(&n, q)).abs() <= (*p - q).length() * (1.0 + 1e-6), "{n:?} at {p:?}");
                }
            }
        }
    }

    #[test]
    fn mirror_folds_the_positive_half() {
        let n = SdfNode::deform(SdfDeform::Mirror { axis: 0 }, ball(3.0, [4.0, 0.0, 0.0]));
        assert!(distance(&n, Vec3d::new(-4.0, 0.0, 0.0)) < -2.9);
        assert!(distance(&n, Vec3d::new(4.0, 0.0, 0.0)) < -2.9);
    }

    #[test]
    fn slots_follow_the_rules() {
        let a = SdfNode::Material { slot: 1, within: None, of: Box::new(ball(5.0, [0.0; 3])) };
        let b = SdfNode::Material { slot: 2, within: None, of: Box::new(ball(5.0, [8.0, 0.0, 0.0])) };
        let at = |x: f64| Vec3d::new(x, 0.0, 0.0);
        let u = SdfNode::Combine { op: SdfOp::Union, blend: 2.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&u, at(-1.0)).1, 1);
        assert_eq!(distance_slot(&u, at(9.0)).1, 2);
        assert_eq!(distance_slot(&u, at(4.0)).1, 1, "a tie keeps the earlier");
        let d = SdfNode::Combine { op: SdfOp::Difference, blend: 0.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&d, at(3.5)).1, 1, "a cut takes the base's");
        let i = SdfNode::Combine { op: SdfOp::Intersection, blend: 0.0, of: vec![a.clone(), b.clone()] };
        assert_eq!(distance_slot(&i, at(1.0)).1, 2);
        // The innermost wins; outside every Material, 0.
        let outer = SdfNode::Material { slot: 3, within: None, of: Box::new(SdfNode::Combine { op: SdfOp::Union, blend: 0.0, of: vec![a, ball(2.0, [-20.0, 0.0, 0.0])] }) };
        assert_eq!(distance_slot(&outer, at(0.0)).1, 1);
        assert_eq!(distance_slot(&outer, at(-20.0)).1, 3);
        assert_eq!(distance_slot(&ball(1.0, [0.0; 3]), at(0.0)).1, 0);
        // The distance is the plain one.
        assert_eq!(distance_slot(&u, at(2.0)).0, distance(&u, at(2.0)));
    }

    #[test]
    fn a_material_within_a_region_paints_only_there() {
        let body = ball(5.0, [0.0; 3]);
        let region = ball(2.0, [5.0, 0.0, 0.0]);
        let n = SdfNode::Material { slot: 4, within: Some(Box::new(region)), of: Box::new(body.clone()) };
        for (x, slot) in [(5.0, 4), (-5.0, 0)] {
            let (d, s) = distance_slot(&n, Vec3d::new(x, 0.0, 0.0));
            assert_eq!(s, slot, "at x {x}");
            assert!((d - distance(&body, Vec3d::new(x, 0.0, 0.0))).abs() < 1e-12, "the distance is the body's");
        }
    }

    #[test]
    fn instance_keeps_its_source_key() {
        let bar = SdfNode::Box { size: [1.0; 3], radius: 0.0 };
        let one = SdfNode::Instance { places: vec![SdfPlace::IDENTITY], of: Box::new(bar.clone()) };
        let moved = SdfNode::Instance { places: vec![SdfPlace { at: [5.0, 0.0, 0.0], ..SdfPlace::IDENTITY }], of: Box::new(bar.clone()) };
        assert_ne!(one.key(), moved.key());
        let (SdfNode::Instance { of: a, .. }, SdfNode::Instance { of: b, .. }) = (&one, &moved) else { unreachable!() };
        assert_eq!(a.key(), b.key());
        assert_eq!(a.key(), bar.key());
        // Only the moved copy is dirty.
        let c = crate::bounds::changed(&one, &moved).unwrap();
        assert!(c.0[0] >= -0.5 - 1e-9 && c.1[0] <= 5.5 + 1e-9);
    }
}
